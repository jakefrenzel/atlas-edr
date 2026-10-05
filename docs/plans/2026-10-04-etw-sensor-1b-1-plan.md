# Sub-project 1b-1 — Schema Additions and `atlas-buffer` Implementation Plan

> **Status:** Approved 2026-10-04, after an independent review (Review Log at the end); the user chose D1 A and D2 A. **For agentic workers:** steps use checkbox (`- [ ]`) syntax for tracking. Nothing in this plan needs elevation, the VM, or a kernel driver.

**Goal:** Add the sub-project 1 additions to the `atlas.events.v1` schema (sensor spec §10), and build `atlas-buffer`, the agent's on-disk segment log (§8), with its tests, fuzz target and CI wiring (§12.1). Plans 1b-2 to 1b-4 build on both.

**Architecture:**
- **Schema** (`atlas-proto`, `atlas-schema`). All changes are additive within `atlas.events.v1`:
  - File System Activity gains `Open` (14).
  - Registry events gain `path_unresolved`, and Value Set gains `data_read_after`, `data_unavailable` and `raw_type`.
  - Two new classes: Event Log Activity (OCSF 1008) and Sensor Health (an Atlas extension class).

  They follow the 0a pattern: proto wire types, Rust domain types, infallible `From`, and a validating `from_wire` that reports exact field paths.
- **`atlas-buffer`** (new crate, portable, no `unsafe`, records are opaque bytes):
  - `record`: pure framing and scanning over byte slices, which is what recovery and the fuzz target use.
  - `Writer`: the single writer. `append` only queues in memory. `tick(now)` does all the I/O: a write plus flush once per second, and after an I/O error a retry every 10 s while records wait in a bounded backlog.
  - `Reader`: iterates after a `Cursor`, and is safe beside a live writer in another process.
  - Four overflow policies at the size cap: rolling retention (the sub-project 1 default), head + tail, drop-oldest and drop-newest.

  Time is passed in, so every behaviour is tested without sleeping. A test-only hook injects I/O failures.

**Tech stack:** Rust 1.97 (edition 2024). Existing: prost 0.14, protox 0.9, proptest 1.11, prost-reflect 0.16, libfuzzer-sys 0.4. New: `crc32c` 0.6 (CRC32C with hardware acceleration, MIT/Apache-2.0) and `tempfile` 3.27 (tests only). CI tools: `buf` 1.73, `actionlint` 1.7.

**Spec:** `docs/specs/2026-10-01-etw-sensor-design.md`, revision 3. Section numbers (§) refer to it unless marked "0a §", which means `docs/specs/2026-09-24-event-schema-design.md`.

**Verification note (2026-10-04, host, Windows build 26200, unelevated):** every code block below was compiled and run in a scratch worktree of `main` (dae2ae6), after the independent review's fixes were applied (see the Review Log at the end).
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean.
- `cargo test --workspace`: 127 tests pass, including the property tests. The symlink tests and the Windows share-mode test ran rather than skipped, because Developer Mode is on.
- The fuzz target's body was also run as a single-threaded property test: 800 random and mutated segments, plus the edge cases (empty, torn magic, zero-filled, foreign). Every assertion held.
- `buf lint` passes (buf 1.73.0), and so does `buf breaking` against `main`'s protos.
- Regenerating the golden fixtures changed none of the 17 existing ones (the only diff was line endings), so 0a events encode exactly as before.
- `actionlint` passes on both workflows. The jq that selects fuzz targets for a PR was run against four changed-file lists.
- The fuzz crate passes `cargo check`. **Not run yet:** `cargo fuzz` (needs nightly on Linux; it first runs in CI on the build PR) and the Linux build of the ~15 non-Windows lines in `fsx.rs` (first compiled by the CI Linux job).
- Throughput (release, 250-byte records, flush every 10 000): the writer took 1.95 M records/s, so the host's ~10 000 events/s costs about 0.5% of one core. A reader took 330 k records/s.

## Global Constraints

- **Branches.** This plan is reviewed on `docs/1b-1-plan`. The build runs on a new branch, `feat/1b-1-schema-buffer`, created from `main` after this plan merges. Never commit to `main`.
- **Additive schema only** (0a §7): no field is renumbered, renamed or removed. `buf breaking` (category `FILE`) must pass against `main`. Every message 0a accepts stays valid.
- **0a validation rules hold** for the new classes: an unset enum or oneof is `Missing`, an unknown enum is `UnknownEnum`, oversize is `TooLarge`, a structural violation is `Malformed`, and field paths use domain names. A class or activity from a newer schema reads as `kind` / `activity: Missing`; `activity` is checked before any class field.
- **The buffer stays opaque:** `atlas-buffer` depends on neither `atlas-schema` nor the `windows` crate. Event times for gap reports come from a function the caller passes in (`RecordTime`).
- **Formatting and lint:** the repo's `rustfmt.toml` (`max_width = 120`, `use_small_heuristics = "Max"`). Each commit passes `cargo fmt --all --check`, except where a step says otherwise. Clippy with `-D warnings` must pass at the end of Tasks 3, 6 and 8.
- **Shell:** commands are given for PowerShell 7. `cargo` commands are the same in bash.
- **Line endings:** `core.autocrlf` is on. Regenerated fixtures may show as modified with no content change; `git diff --ignore-cr-at-eol` must be empty for the existing ones.
- Every commit message ends with the attribution lines the session's system reminder specifies (currently `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`).

## Decisions (chosen 2026-10-04: D1 A, D2 A)

The user chose the recommended option for both. The alternatives are kept for the record.

### D1, Sensor Health wire shape

Spec §10.3 says Sensor Health carries the §9.3 counters (optional `u64`), the interval, and gap reports. It leaves open how they are laid out and what a counter's value means.

- **(A) Recommended: typed fields, grouped.** One `optional uint64` per §9.3 counter, in four groups (`loss`, `quality`, `housekeeping`, `resources`), plus an optional `gap`.
  - Counters are **deltas** over `[interval_start, Event.time]`, so the server can sum them, and a lost report loses only its own interval. Gauges (stuck helpers, negative-cache size, working set) are sampled at the end of the interval.
  - Absent means "not measured" (for example, seeding off); `0` means measured and none occurred.
  - Per-class counts (`actor_dropped`, `actor_unresolved`) are lists of `{class_uid, count}`, at most 32 entries.
  - A fifth group, `buffer`, reports the buffer's own health (§8.3, "recovery is reported in Sensor Health"): write errors, recoveries, whether writes are failing, rejected records, segments that could not be deleted, what startup recovery found, corrupt segments, and disk use.
  - Pros: self-documenting, validated, and queryable as columns. The §13 / DoD 4 loss check reads fixed fields.
  - Cons: if plans 1b-3/1b-4 find a counter wrong, it becomes `reserved` and a new field is added. That costs one field number, not a breaking change.
- (B) A fixed core (interval, gap, resources, the four §13 loss counters) plus `map<string, uint64>` for the rest. Flexible while plans 1b-3/1b-4 are written, but the names are unvalidated and undocumented in the schema, and rules have to key on strings.
- (C) Defer both new classes to plan 1b-4, where the watchdog that fills them is written. This splits the §10 schema work over two plans and two schema PRs, and leaves DoD item 3 half done until then.

### D2, Atlas extension UID (§10.3)

OCSF's registry (`ocsf-schema/extensions.md`, read 2026-10-04) lists native extensions 1–3 (Linux, Windows, macOS) and vendor extensions 985–999 (999 is "Development"). Vendors are numbered downward from 999.
- **(A) Recommended: 500.** Far from the vendor range, and not registered. `class_uid` = 500 × 100000 + 6 × 1000 + 1 = **50006001**, so `type_uid` is 5000600101 (above `u32`; `OcsfIds::type_uid` is already `u64`). If Atlas becomes a product, register it, and if that brings a different number, add a v2 class.
- (B) 999 ("Development"). It is registered, but any other development extension may use it too.
- (C) Register Atlas with OCSF now. This is a public PR to the OCSF repo, early for a home project.

## Deliberate clarifications of the spec (applied to the spec text in Task 9)

1. **Segment header.** Every segment starts with an 8-byte header, `ATLSEG01` (a magic number plus a format version), and records follow it (refines §8.1). Recovery and readers check it. This allows format evolution and refuses a file that is not ours.
   - A newest segment whose header is torn is rewritten as an empty segment. Torn means a prefix of the magic followed by nothing or by zeros: NTFS can keep a new file's size after a power cut while its unwritten bytes read as zeros.
   - A newest segment with any other wrong header is kept, skipped by readers and reported (`Recovery::foreign_segment`).
   - Segment numbers never wrap: a file name or cursor at `u64::MAX` is refused with `InvalidData`. Sealed segments that are symlinks or junctions are refused at open, like the newest.
2. **The writer always starts a new segment when it opens** (refines §8.2). It never appends to a segment from before the restart.
   - Why: a reader can see records that are written but not yet flushed, so a transport can ack a position that a power cut then cuts away.
   - If the writer appended to that segment after recovery, new records would land behind the ack and be skipped.
   - Cost: one extra segment file per restart.
   - Segments behind the cursor are deleted at open where possible. One that cannot be deleted is kept and counted.
3. **The cursor file lives in `buffer\`** (`buffer\cursor`), not beside it as §11.1 lists. One directory belongs to the crate, and segment listing ignores every name that is not `NNNN….seg`.
4. **What a reader treats as corruption** (refines §8.5). In the **newest** segment, any record that is not whole and valid means "not written yet": the reader waits. Only in a **sealed** segment (one with a newer segment after it) is a bad record corruption: the reader skips the rest of the segment and counts it (`ReaderStats::corrupt_segments`). A reader can see a record half-written, and what NTFS returns for bytes not yet written is not guaranteed to be valid, so a CRC failure in the newest segment cannot be told apart from a write in progress.
5. **Writer API** (makes §8.1 / §8.3 concrete).
   - `append` only queues in memory. `tick(now)` writes and flushes when `flush_interval` (1 s) has passed, or as soon as half the backlog is used. An idle tick does no I/O; in particular, no empty segment is created and nothing is flushed.
   - After an I/O error, `tick` retries every `retry_interval` (10 s). On retry, the active segment is cut back to its last good length (removing a torn write) before the queued records are written.
   - The backlog default is 32 MiB: over 2 minutes at the host's average rate, about 2.5 s at the 52 000/s peak. Beyond it, `append` drops and counts (`Stats::backlog_drops`).
   - **One thread owns the `Writer`** (the agent's buffer writer thread, [6] in §3.2). It is fed by a bounded channel and calls `tick` between messages. `tick` can block on the disk (a flush, or reading a segment of up to 16 MiB for a gap report), and meanwhile the channel absorbs incoming events. `append` is only "non-blocking" in the sense that it does no I/O itself.
6. **Overflow mechanics** (makes §8.4 concrete). Room is made when a new segment is started: whole sealed segments are deleted until a new full segment fits under the cap.
   - A segment that cannot be deleted is skipped and counted (`Stats::delete_failures`), and the next candidate goes instead. This happens when another process holds it open without delete sharing (an editor, `Get-Content`). If no candidate can be deleted, the new segment goes over the cap. Writing never stalls on a delete, and an eviction is counted only once its delete succeeds.
   - The cap can also be exceeded by one record larger than a segment, which gets a segment of its own.
   - Head + tail pins the oldest segments that together hold at least 25% of the bytes on disk, and deletes the first segment after them. The first segment is therefore never deleted.
   - Drop-newest deletes nothing. Records that do not fit are dropped and collected in an open gap, which `take_gaps` closes, so each periodic Sensor Health report includes current drops.
   - A gap's time range comes from a caller-supplied `RecordTime` function, so the buffer stays opaque. Plan 1b-4 passes one that decodes the event. The agent merges several gaps in one interval (sum of counts, min and max of times), because a report carries one gap.
   - Config is validated: cap ≥ 4 × segment size, and the backlog must hold one maximum-size record.
7. **No OCSF exporter exists yet**, so §10's "updates the OCSF exporter (`ocsf.rs`)" is reworded. `ocsf.rs` holds only the derived OCSF ids, and it gains the new classes and activities. How the new fields map into the `atlas` extension namespace is documented in `schema-reference.md` for the exporter (a later adapter, decision log 2026-09-24).
8. **Validation details** the spec left open (§10.2, §10.4):
   - `log_name` "required" means non-empty, because proto3 cannot tell empty from absent. `log_provider` must be non-empty for Disable.
   - The new error paths are `reg_value.raw_type` (Malformed: in 0–11, or set together with `type`), `reg_value.data_unavailable` (Malformed: data present or truncated), `log_name` / `log_provider` (Missing / TooLarge), `loss.actor_dropped` / `quality.actor_unresolved` (TooLarge, over 32 entries), `gap.last_time` (Malformed: before `first_time`) and `gap` (Malformed: only one time present).
   - The domain type for a value's type is `RegType::{Known(RegValueType), Raw(u32)}`, so the sensor cannot set both `type` and `raw_type`. `RegType::from_raw` picks the variant. A hand-built `Raw(n)` with n ≤ 11 still encodes, and is rejected on decode.
9. **Sensor Health semantics** per D1 and D2, including the `buffer` group (refines §10.3).

## Review Focus

These inputs would slip past a plain round trip, so each has a pinned test:
1. **Old tests that relied on unused tag numbers.** 0a's version-skew tests use the first *unused* tag of a message as "a field from a newer schema". Sub-project 1 takes three of them:
   - `Event` 17 (now Event Log Activity);
   - `RegistryKeyActivity` / `RegistryValueActivity` 6 (now `path_unresolved`, a varint, so the old length-delimited probe would read as `Malformed`);
   - `FileSystemActivity` 9 (now `Open`).

   Task 3 moves the probes to 19, 7 and 10, and adds the two new classes to the "activity before class fields" test.
2. **0a messages still decode.** All 17 existing golden fixtures are byte-identical after regeneration, and a Value Set with neither `type` nor `raw_type` is still `reg_value.type: Missing`. See Task 3 and `raw_type_must_be_above_reg_qword_and_alone`.
3. **A torn write at any byte** recovers exactly the whole records before it, and writing continues after them. See `a_segment_torn_at_every_byte_recovers_its_whole_records`, which cuts at every offset, and the property tests on the pure scan.
4. **A length field claiming 4 GiB** is rejected from the 8-byte header, before any allocation. See `bad_length_or_crc_is_invalid`, and the reader's `read_at`.
5. **A reader beside a live writer.** A half-written tail is waited for, not flagged. A segment deleted while the reader holds it open is read to its end; on Windows that needs `FILE_SHARE_DELETE`. See `tests/concurrent.rs`.
6. **Head + tail never deletes the pinned head**, and every lost record is in a gap. See `overflow_invariants` (a property test over all four policies, which also compares the writer's byte count with the files on disk), and `head_tail_pins_at_least_a_quarter_of_the_bytes` (a 16-segment cap, so the head is several segments).
7. **I/O failure mid-write** leaves torn bytes. The retry removes them and writes the backlog in order, including when the failure comes right after a rotation. See the `writer` unit tests, in particular `a_failure_right_after_a_rotation_is_retried_in_order`.
8. **Reparse points:** a segment (newest or sealed) or a buffer directory that is a symlink is refused (`InvalidData`).
9. **A segment that cannot be deleted** (held open without delete sharing) neither stalls the writer nor is counted twice; the next segment goes instead. See `a_segment_that_cannot_be_deleted_is_skipped_and_counted_once` (Windows).
10. **An ack ahead of a power cut** does not hide records written after the restart. See `an_ack_ahead_of_a_power_cut_does_not_hide_new_records`.
11. **A zero-filled newest segment** (NTFS after a power cut) is repaired as a torn header, not reported as foreign. See `a_zero_filled_newest_segment_is_a_torn_header_not_foreign`.

## File Structure

```
Cargo.toml                                   + crc32c, tempfile in [workspace.dependencies]
crates/atlas-proto/
  build.rs                                   + event_log.proto, sensor_health.proto
  proto/atlas/events/v1/
    file.proto                               + FileOpen (activity 9 → OCSF 14)
    registry.proto                           + path_unresolved; Value Set data_read_after, data_unavailable, raw_type
    event_log.proto                          NEW  EventLogActivity (OCSF 1008)
    sensor_health.proto                      NEW  SensorHealth (Atlas extension class 50006001), 5 counter groups + gap
    event.proto                              + oneof kind: event_log = 17, sensor_health = 18
crates/atlas-schema/
  src/limits.rs                              + EVENT_LOG_NAME_MAX, CLASS_COUNTS_MAX
  src/ocsf.rs                                + ATLAS_EXTENSION_UID, SENSOR_HEALTH_CLASS_UID, new ids
  src/event.rs, src/lib.rs, src/classes/mod.rs   wiring
  src/classes/file.rs                        + FileAction::Open
  src/classes/registry.rs                    + RegType, path_unresolved, read-after flags
  src/classes/event_log.rs                   NEW
  src/classes/sensor_health.rs               NEW
  tests/common/mod.rs, validation.rs, ocsf_ids.rs   samples, strategies, rule table
  tests/fixtures/*.json                      + 8 new golden fixtures
crates/atlas-buffer/                         NEW crate
  Cargo.toml
  src/lib.rs                                 Config, Overflow, Gap, Stats, RecordTime
  src/record.rs                              framing, parse, scan (pure)
  src/fsx.rs                                 segment names, share-delete opens, reparse-point refusal
  src/cursor.rs                              Cursor, atomic cursor file
  src/writer.rs                              Writer, Recovery, Dropped
  src/reader.rs                              Reader, Record, ReaderStats
  tests/common/mod.rs, log.rs, recovery.rs, concurrent.rs
  fuzz/                                      own workspace: buffer_recover target
.github/workflows/fuzz.yml                   matrix over (crate, target), PR selection
.github/workflows/ci.yml                     + buffer fuzz crate check and audit
docs/schema-reference.md, docs/specs/…, docs/architecture-overview.md   Task 9
```

## Interfaces for later plans

- **Plan 1b-3/1b-4 build events with:**
  - `FileAction::Open`;
  - `RegistryKeyActivity.path_unresolved`;
  - `RegistryValueActivity { path_unresolved, action: Set { value_type: RegType, data, data_truncated, data_read_after, data_unavailable } }`. `RegType::from_raw(event.Type)` picks `Known` or `Raw`; a `Raw` type means `data_unavailable` (§7.5).
  - `classes::event_log::{EventLogActivity, EventLogAction::{Stop, Restart, Disable}}`;
  - `classes::sensor_health::{SensorHealthActivity, SensorHealthAction::Report(Box<HealthReport>), HealthReport, SensorLoss, SensorQuality, SensorHousekeeping, SensorResources, SensorGap, ClassCount}`;
  - `atlas_schema::{SENSOR_HEALTH_CLASS_UID, ATLAS_EXTENSION_UID}`, `limits::{EVENT_LOG_NAME_MAX, CLASS_COUNTS_MAX}`.
- **Plan 1b-4 runs the buffer with:**
  - `Writer::open(Config::new(dir), record_time)` returns `(Writer, Recovery)`. `Recovery` holds the truncated bytes, whether the cursor was reset, and any foreign segment.
  - Then `append(&bytes)`, which returns `Err(Dropped::{Rejected, BacklogFull})`.
  - One thread owns the `Writer` and receives encoded events over a bounded channel. Between messages, and at least every 100 ms, it calls `tick(Instant::now())`. `close()` runs on a clean stop. `tick` can block on the disk (clarification 5).
  - Also: `stats()`, `take_gaps()`, `is_failing()`, `disk_bytes()`. `Writer` is `Send`.
- **Sensor Health mapping (plan 1b-4):**

  | Source | Sensor Health field |
  |---|---|
  | `Stats::retention_evictions` | `housekeeping.retention_evictions` |
  | `Stats::backlog_drops` | `loss.buffer_backlog_drops` |
  | `take_gaps()`, merged | `gap` |
  | Replay records failing `decode_event` | `quality.buffer_invalid_records` |
  | `ReaderStats::corrupt_segments` | `buffer.corrupt_segments` |
  | `Stats::{write_errors, recoveries, rejected, delete_failures}`, `is_failing()`, `disk_bytes()` | `buffer.{write_errors, recoveries, rejected, delete_failures, failing, disk_bytes}` |
  | `Recovery` (first report only) | `buffer.{truncated_bytes, cursor_reset, foreign_segments}` |
- **`dump` (1b-4) and the transport (sub-project 2):**
  - `Reader::open(dir, cursor)` then `next_record()`, which returns `Ok(None)` when caught up (poll again). Each `Record` carries `at` and `next` cursors.
  - The transport acks with `Writer::ack(record.next)`. Sub-project 2 switches `Config::overflow` to `Overflow::HeadTail`.

---

### Task 1: Wire contract (protos)

**Files:**
- Modify: `crates/atlas-proto/proto/atlas/events/v1/file.proto`, `registry.proto`, `event.proto`, `crates/atlas-proto/build.rs`
- Create: `crates/atlas-proto/proto/atlas/events/v1/event_log.proto`, `sensor_health.proto`

**Interfaces:**
- Produces the generated types: `wire::FileOpen`; the new fields on `wire::RegistryKeyActivity`, `wire::RegistryValueActivity` and `wire::RegistryValueSet`; `wire::{EventLogActivity, EventLogStop, EventLogRestart, EventLogDisable, SensorHealth, SensorHealthReport, SensorLoss, SensorQuality, SensorHousekeeping, SensorResources, SensorGap, ClassCount}`; and `wire::event::Kind::{EventLog, SensorHealth}`.

`atlas-schema` does not compile from the end of this task until Task 2 is done (new fields and oneof variants), so this task does not commit.

- [ ] **Step 1: Create the branch**

```powershell
git switch main
git pull
git switch -c feat/1b-1-schema-buffer
```

- [ ] **Step 2: Add `Open` to File System Activity**

`crates/atlas-proto/proto/atlas/events/v1/file.proto`:
```diff
--- a/crates/atlas-proto/proto/atlas/events/v1/file.proto
+++ b/crates/atlas-proto/proto/atlas/events/v1/file.proto
@@ -16,6 +16,7 @@ message FileSystemActivity {
     FileDelete delete = 6;                // activity_id 4
     FileRename rename = 7;                // activity_id 5
     FileSetAttributes set_attributes = 8; // activity_id 6
+    FileOpen open = 9;                    // activity_id 14
   }
 }
 
@@ -30,3 +31,6 @@ message FileRename {
 }
 
 message FileSetAttributes {}
+
+// A file handle was opened (OCSF: "a request to create a file handle"). Access intent, not proof of a read.
+message FileOpen {}
```

- [ ] **Step 3: Add the registry fields**

`crates/atlas-proto/proto/atlas/events/v1/registry.proto`:
```diff
--- a/crates/atlas-proto/proto/atlas/events/v1/registry.proto
+++ b/crates/atlas-proto/proto/atlas/events/v1/registry.proto
@@ -14,6 +14,8 @@ message RegistryKeyActivity {
     RegistryKeyDelete delete = 4; // activity_id 4
     RegistryKeyRename rename = 5; // activity_id 5
   }
+  // `path` holds only what the sensor saw (a relative name, or empty), not a full path.
+  bool path_unresolved = 6;
 }
 
 message RegistryKeyCreate {}
@@ -34,15 +36,23 @@ message RegistryValueActivity {
     RegistryValueSet set = 4;       // activity_id 2
     RegistryValueDelete delete = 5; // activity_id 4
   }
+  // `key_path` holds only what the sensor saw (a relative name, or empty), not a full path.
+  bool path_unresolved = 6;
 }
 
 message RegistryValueSet {
   // Windows REG_* constant (REG_NONE = 0 .. REG_QWORD = 11). Not an OCSF type_id.
   // `optional` because REG_NONE is 0: without presence an unset type would
-  // silently read as REG_NONE. Absent is rejected as Missing.
+  // silently read as REG_NONE. Absent is rejected as Missing, unless `raw_type` is set.
   optional uint32 type = 1;
   bytes data = 2;
   bool data_truncated = 3;
+  // `data` was read from the registry after the event, not captured with it.
+  bool data_read_after = 4;
+  // No data was obtained: `data` is then empty and `data_truncated` false.
+  bool data_unavailable = 5;
+  // The value's type when it is above REG_QWORD (11); `type` is then absent.
+  optional uint32 raw_type = 6;
 }
 
 message RegistryValueDelete {}
```

- [ ] **Step 4: Create `event_log.proto`**

`crates/atlas-proto/proto/atlas/events/v1/event_log.proto`:
```protobuf
// OCSF Event Log Activity (class_uid 1008): tampering seen on the agent's own ETW sessions.
syntax = "proto3";

package atlas.events.v1;

import "atlas/events/v1/objects.proto";

message EventLogActivity {
  // Optional: the watchdog sees the effect, not who caused it.
  ProcessRef actor = 1;
  // OCSF `log_name`: the ETW session name. Required.
  string log_name = 2;
  // OCSF `log_provider`: the ETW provider name. Required for Disable.
  string log_provider = 3;
  // OCSF `status_code`: the Win32 error or NTSTATUS that revealed the change.
  optional uint32 status_code = 4;
  oneof activity {
    EventLogStop stop = 5;       // activity_id 7
    EventLogRestart restart = 6; // activity_id 8
    EventLogDisable disable = 7; // activity_id 10
  }
}

// The session was found stopped, or replaced by another session with its name.
message EventLogStop {}

// The agent recreated the session.
message EventLogRestart {}

// A provider was disabled or changed in the session, or its canary went silent.
message EventLogDisable {}
```

- [ ] **Step 5: Create `sensor_health.proto`**

`crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto`:
```protobuf
// Atlas extension class Sensor Health (class_uid 50006001, category 6 Application Activity):
// the agent's own loss, quality, housekeeping and resource figures.
//
// Counters count occurrences during the interval [interval_start, Event.time]. Gauges are
// sampled at the end of the interval. An absent field was not measured (for example, seeding
// is off); 0 means measured and none occurred.
syntax = "proto3";

package atlas.events.v1;

message SensorHealth {
  // Start of the interval, in nanoseconds since the Unix epoch, UTC.
  int64 interval_start = 1;
  oneof activity {
    SensorHealthReport report = 2; // activity_id 1
  }
}

message SensorHealthReport {
  SensorLoss loss = 1;
  SensorQuality quality = 2;
  SensorHousekeeping housekeeping = 3;
  SensorResources resources = 4;
  // Set when buffered events were deleted before delivery (overflow).
  SensorGap gap = 5;
  SensorBuffer buffer = 6;
}

// A count for one event class.
message ClassCount {
  // OCSF class_uid.
  uint32 class_uid = 1;
  uint64 count = 2;
}

// Events that never reached the buffer.
message SensorLoss {
  // ETW events lost, per session (Atlas-Sensor, Atlas-Process).
  optional uint64 sensor_session_events_lost = 1;
  optional uint64 process_session_events_lost = 2;
  // ETW real-time buffers lost, per session.
  optional uint64 sensor_session_buffers_lost = 3;
  optional uint64 process_session_buffers_lost = 4;
  // Events dropped because an internal queue was full.
  optional uint64 kernel_queue_drops = 5;
  optional uint64 user_queue_drops = 6;
  // DNS-Client events over the per-process rate limit.
  optional uint64 dns_rate_limit_drops = 7;
  // Events dropped because no process uid could be computed, per class.
  repeated ClassCount actor_dropped = 8;
  // Events dropped because the buffer's in-memory backlog was full (disk full or I/O errors).
  optional uint64 buffer_backlog_drops = 9;
}

// Events emitted with less than full information, or handled outside the usual path.
message SensorQuality {
  optional uint64 late_arrivals = 1;
  optional uint64 parse_errors = 2;
  optional uint64 unknown_version = 3;
  optional uint64 launch_join_miss = 4;
  // Events emitted with an unresolved actor (empty path and name), per class.
  repeated ClassCount actor_unresolved = 5;
  optional uint64 unknown_file_object = 6;
  optional uint64 registry_unresolved = 7;
  optional uint64 value_read_failed = 8;
  optional uint64 early_read_redone = 9;
  optional uint64 reg_type_unusual = 10;
  optional uint64 file_op_late_failure = 11;
  optional uint64 writes_after_cleanup = 12;
  optional uint64 file_object_replaced = 13;
  // Invalid records found when the buffer was replayed.
  optional uint64 buffer_invalid_records = 14;
  optional uint64 enrichment_misses = 15;
  optional uint64 enrichment_errors = 16;
}

// Evictions from bounded structures, and expected drops.
message SensorHousekeeping {
  optional uint64 process_cache_evictions = 1;
  optional uint64 file_map_evictions = 2;
  optional uint64 key_map_evictions = 3;
  optional uint64 early_key_map_evictions = 4;
  optional uint64 flow_table_evictions = 5;
  optional uint64 hash_cache_evictions = 6;
  // Whole buffer segments deleted by rolling retention (expected while there is no transport).
  optional uint64 retention_evictions = 7;
  // Failed file creates, deletes and renames that were dropped.
  optional uint64 file_op_failed = 8;
  optional uint64 pending_overflow = 9;
  // Handle-table seeding.
  optional bool seeding_enabled = 10;
  optional uint64 seeder_handles_named = 11;
  optional uint64 seeder_handles_failed = 12;
  optional uint64 seeder_handles_timed_out = 13;
  optional uint64 seeder_table_reads = 14;
  optional uint64 seeder_deferred_rereads = 15;
  // Gauges.
  optional uint64 seeder_stuck_helpers = 16;
  optional uint64 seeder_negative_cache_size = 17;
}

// The agent's own resource use.
message SensorResources {
  // Process CPU time (user + kernel) during the interval, in nanoseconds.
  optional uint64 cpu_time = 1;
  // Working set at the end of the interval, in bytes (a gauge).
  optional uint64 working_set = 2;
}

// The agent's on-disk buffer (sensor spec §8). Records lost from it are counted in
// `loss.buffer_backlog_drops`, `quality.buffer_invalid_records` and `gap`.
message SensorBuffer {
  // Write attempts that failed (disk full, I/O errors), and successful retries after them.
  optional uint64 write_errors = 1;
  optional uint64 recoveries = 2;
  // Gauge: writes were failing at the end of the interval.
  optional bool failing = 3;
  // Records refused because they were empty or over 256 KiB (an agent defect).
  optional uint64 rejected = 4;
  // Segments that could not be deleted because another process held them open.
  optional uint64 delete_failures = 5;
  // Found at startup, reported once in the first report: torn bytes cut from the
  // newest segment, a damaged cursor file, a newest segment that was not ours.
  optional uint64 truncated_bytes = 6;
  optional bool cursor_reset = 7;
  optional uint64 foreign_segments = 8;
  // Sealed segments whose remainder a reader skipped as corrupt.
  optional uint64 corrupt_segments = 9;
  // Gauge: bytes in all segments.
  optional uint64 disk_bytes = 10;
}

// Buffered events deleted before delivery.
message SensorGap {
  // Event-time range of the deleted events, in nanoseconds since the Unix epoch, UTC.
  // Absent when none of the deleted records could be decoded.
  optional int64 first_time = 1;
  optional int64 last_time = 2;
  uint64 events = 3;
}
```

- [ ] **Step 6: Add both classes to the envelope and the build**

`crates/atlas-proto/proto/atlas/events/v1/event.proto`:
```diff
--- a/crates/atlas-proto/proto/atlas/events/v1/event.proto
+++ b/crates/atlas-proto/proto/atlas/events/v1/event.proto
@@ -5,11 +5,13 @@ syntax = "proto3";
 package atlas.events.v1;
 
 import "atlas/events/v1/dns.proto";
+import "atlas/events/v1/event_log.proto";
 import "atlas/events/v1/file.proto";
 import "atlas/events/v1/module.proto";
 import "atlas/events/v1/network.proto";
 import "atlas/events/v1/process.proto";
 import "atlas/events/v1/registry.proto";
+import "atlas/events/v1/sensor_health.proto";
 
 enum Sensor {
   SENSOR_UNSPECIFIED = 0;
@@ -39,5 +41,7 @@ message Event {
     RegistryKeyActivity registry_key = 14;
     RegistryValueActivity registry_value = 15;
     DnsActivity dns = 16;
+    EventLogActivity event_log = 17;
+    SensorHealth sensor_health = 18;
   }
 }
```

`crates/atlas-proto/build.rs`:
```diff
--- a/crates/atlas-proto/build.rs
+++ b/crates/atlas-proto/build.rs
@@ -9,6 +9,8 @@ const PROTO_FILES: &[&str] = &[
     "atlas/events/v1/file.proto",
     "atlas/events/v1/registry.proto",
     "atlas/events/v1/dns.proto",
+    "atlas/events/v1/event_log.proto",
+    "atlas/events/v1/sensor_health.proto",
     "atlas/events/v1/event.proto",
 ];
```

- [ ] **Step 7: Build and check the contract**

```powershell
cargo build -p atlas-proto
```
Expected: `Finished`. If `buf` is installed, run it as well (with Docker: `docker run --rm -v "${PWD}:/w" -w /w bufbuild/buf:1.73.0 lint crates/atlas-proto/proto`):
```powershell
buf lint crates/atlas-proto/proto
buf breaking crates/atlas-proto/proto --against '.git#branch=main,subdir=crates/atlas-proto/proto'
```
Expected: no output from either. Otherwise the CI `proto` job runs both on the PR.

### Task 2: Domain types

**Files:**
- Modify: `crates/atlas-schema/src/limits.rs`, `ocsf.rs`, `event.rs`, `lib.rs`, `classes/mod.rs`, `classes/file.rs`, `classes/registry.rs`
- Create: `crates/atlas-schema/src/classes/event_log.rs`, `classes/sensor_health.rs`

**Interfaces:**
- Consumes: Task 1's wire types; 0a's `convert::{require, bounded, err}`, `ProcessRef::{from_wire, required}` and `objects::test_support`.
- Produces: the types listed under "Interfaces for later plans", and `EventKind::{EventLog, SensorHealth}` with their `ocsf_ids()`.

The integration tests (`tests/`) do not compile until Task 3. This task verifies with `--lib`.

- [ ] **Step 1: Limits, module list and envelope wiring**

`crates/atlas-schema/src/limits.rs`:
```diff
--- a/crates/atlas-schema/src/limits.rs
+++ b/crates/atlas-schema/src/limits.rs
@@ -21,6 +21,10 @@ pub const USER_UID_MAX: usize = 256;
 pub const USER_NAME_MAX: usize = 1024;
 /// `file.signature.signer`.
 pub const SIGNER_MAX: usize = 1024;
+/// `log_name` and `log_provider` (Event Log Activity).
+pub const EVENT_LOG_NAME_MAX: usize = 256;
+/// Entries in each per-class counter list of a Sensor Health report.
+pub const CLASS_COUNTS_MAX: usize = 32;
 /// A whole encoded event, checked before protobuf decoding.
 pub const EVENT_MAX: usize = 256 * 1024;
```

`crates/atlas-schema/src/classes/mod.rs`:
```diff
--- a/crates/atlas-schema/src/classes/mod.rs
+++ b/crates/atlas-schema/src/classes/mod.rs
@@ -1,9 +1,12 @@
-//! The seven v1 event classes (spec section 5). Each module holds the domain
-//! types for one OCSF class plus their wire conversions.
+//! The event classes: the seven 0a classes (spec section 5) and the two added
+//! by sub-project 1 (sensor spec §10). Each module holds the domain types for
+//! one class plus their wire conversions.
 
 pub mod dns;
+pub mod event_log;
 pub mod file;
 pub mod module;
 pub mod network;
 pub mod process;
 pub mod registry;
+pub mod sensor_health;
```

`crates/atlas-schema/src/event.rs`:
```diff
--- a/crates/atlas-schema/src/event.rs
+++ b/crates/atlas-schema/src/event.rs
@@ -5,11 +5,13 @@ use atlas_proto::v1::event::Kind as W;
 use uuid::Uuid;
 
 use crate::classes::dns::DnsActivity;
+use crate::classes::event_log::EventLogActivity;
 use crate::classes::file::FileSystemActivity;
 use crate::classes::module::ModuleActivity;
 use crate::classes::network::NetworkActivity;
 use crate::classes::process::ProcessActivity;
 use crate::classes::registry::{RegistryKeyActivity, RegistryValueActivity};
+use crate::classes::sensor_health::SensorHealthActivity;
 use crate::convert::{Result, err, fixed, require, wire_enum};
 use crate::error::{SchemaError, SchemaErrorKind};
 use crate::ids::{BootId, DeviceUid, EventId};
@@ -51,6 +53,8 @@ pub enum EventKind {
     RegistryKey(RegistryKeyActivity),
     RegistryValue(RegistryValueActivity),
     Dns(DnsActivity),
+    EventLog(EventLogActivity),
+    SensorHealth(SensorHealthActivity),
 }
 
 impl From<Event> for wire::Event {
@@ -67,6 +71,8 @@ impl From<Event> for wire::Event {
             EventKind::RegistryKey(a) => W::RegistryKey(a.into()),
             EventKind::RegistryValue(a) => W::RegistryValue(a.into()),
             EventKind::Dns(a) => W::Dns(a.into()),
+            EventKind::EventLog(a) => W::EventLog(a.into()),
+            EventKind::SensorHealth(a) => W::SensorHealth(a.into()),
         };
         Self {
             event_id: v.meta.event_id.as_bytes().to_vec(),
@@ -112,6 +118,8 @@ impl TryFrom<wire::Event> for Event {
             W::RegistryKey(a) => EventKind::RegistryKey(RegistryKeyActivity::from_wire(a)?),
             W::RegistryValue(a) => EventKind::RegistryValue(RegistryValueActivity::from_wire(a)?),
             W::Dns(a) => EventKind::Dns(DnsActivity::from_wire(a)?),
+            W::EventLog(a) => EventKind::EventLog(EventLogActivity::from_wire(a)?),
+            W::SensorHealth(a) => EventKind::SensorHealth(SensorHealthActivity::from_wire(a)?),
         };
         Ok(Self { meta: EventMeta { event_id, time: w.time, sensor }, device, kind })
     }
```

`crates/atlas-schema/src/lib.rs`:
```diff
--- a/crates/atlas-schema/src/lib.rs
+++ b/crates/atlas-schema/src/lib.rs
@@ -20,7 +20,7 @@ pub use error::{SchemaError, SchemaErrorKind};
 pub use event::{Device, Event, EventKind, EventMeta, Sensor};
 pub use ids::{BootId, DeviceUid, EventId, ProcessUid, process_uid};
 pub use objects::{File, Hashes, Integrity, NetworkEndpoint, Process, ProcessRef, Signature, SignatureStatus, User};
-pub use ocsf::OcsfIds;
+pub use ocsf::{ATLAS_EXTENSION_UID, OcsfIds, SENSOR_HEALTH_CLASS_UID};
 
 /// Generated wire types, re-exported for transport code.
 pub use atlas_proto::v1 as wire;
```

- [ ] **Step 2: OCSF ids**

`crates/atlas-schema/src/ocsf.rs`:
```diff
--- a/crates/atlas-schema/src/ocsf.rs
+++ b/crates/atlas-schema/src/ocsf.rs
@@ -2,11 +2,13 @@
 //! They are never stored, so they cannot disagree with the event.
 
 use crate::classes::dns::DnsAction;
+use crate::classes::event_log::EventLogAction;
 use crate::classes::file::FileAction;
 use crate::classes::module::ModuleAction;
 use crate::classes::network::NetworkAction;
 use crate::classes::process::ProcessActivity;
 use crate::classes::registry::{RegistryKeyAction, RegistryValueAction};
+use crate::classes::sensor_health::SensorHealthAction;
 use crate::event::EventKind;
 
 #[derive(Debug, Clone, Copy, PartialEq, Eq)]
@@ -25,6 +27,14 @@ impl OcsfIds {
 
 const SYSTEM: u32 = 1;
 const NETWORK: u32 = 4;
+const APPLICATION: u32 = 6;
+
+/// Atlas's OCSF extension uid. Outside OCSF's registered extensions (1–3 and
+/// 985–999 in the registry at github.com/ocsf/ocsf-schema `extensions.md`, 2026-10-04).
+pub const ATLAS_EXTENSION_UID: u32 = 500;
+
+/// Sensor Health: `extension_uid × 100000 + category_uid × 1000 + 1` (OCSF's extension class rule).
+pub const SENSOR_HEALTH_CLASS_UID: u32 = ATLAS_EXTENSION_UID * 100_000 + APPLICATION * 1000 + 1;
 
 impl EventKind {
     pub fn ocsf_ids(&self) -> OcsfIds {
@@ -62,6 +72,7 @@ impl EventKind {
                     FileAction::Delete => 4,
                     FileAction::Rename { .. } => 5,
                     FileAction::SetAttributes => 6,
+                    FileAction::Open => 14,
                 },
             ),
             EventKind::RegistryKey(a) => (
@@ -88,6 +99,22 @@ impl EventKind {
                     DnsAction::Response { .. } => 2,
                 },
             ),
+            EventKind::EventLog(a) => (
+                SYSTEM,
+                1008,
+                match a.action {
+                    EventLogAction::Stop => 7,
+                    EventLogAction::Restart => 8,
+                    EventLogAction::Disable => 10,
+                },
+            ),
+            EventKind::SensorHealth(a) => (
+                APPLICATION,
+                SENSOR_HEALTH_CLASS_UID,
+                match a.action {
+                    SensorHealthAction::Report(_) => 1,
+                },
+            ),
         };
         OcsfIds { category_uid, class_uid, activity_id }
     }
```

- [ ] **Step 3: File `Open`**

`crates/atlas-schema/src/classes/file.rs`:
```diff
--- a/crates/atlas-schema/src/classes/file.rs
+++ b/crates/atlas-schema/src/classes/file.rs
@@ -1,4 +1,4 @@
-//! File System Activity (OCSF 1001), spec section 5.5.
+//! File System Activity (OCSF 1001), spec section 5.5, plus `Open` (sensor spec §10.1).
 
 use atlas_proto::v1 as wire;
 use atlas_proto::v1::file_system_activity::Activity as W;
@@ -20,8 +20,12 @@ pub enum FileAction {
     Read,
     Update,
     Delete,
-    Rename { file_result: File },
+    Rename {
+        file_result: File,
+    },
     SetAttributes,
+    /// A handle was opened: access intent, not proof of a read.
+    Open,
 }
 
 impl From<FileSystemActivity> for wire::FileSystemActivity {
@@ -33,6 +37,7 @@ impl From<FileSystemActivity> for wire::FileSystemActivity {
             FileAction::Delete => W::Delete(wire::FileDelete {}),
             FileAction::Rename { file_result } => W::Rename(wire::FileRename { file_result: Some(file_result.into()) }),
             FileAction::SetAttributes => W::SetAttributes(wire::FileSetAttributes {}),
+            FileAction::Open => W::Open(wire::FileOpen {}),
         };
         Self { actor: Some(v.actor.into()), file: Some(v.file.into()), activity: Some(activity) }
     }
@@ -52,6 +57,7 @@ impl FileSystemActivity {
                 W::Delete(_) => FileAction::Delete,
                 W::Rename(r) => FileAction::Rename { file_result: File::required(r.file_result, "", "file_result")? },
                 W::SetAttributes(_) => FileAction::SetAttributes,
+                W::Open(_) => FileAction::Open,
             },
         })
     }
@@ -76,6 +82,7 @@ mod tests {
             FileAction::Delete,
             FileAction::Rename { file_result: file("C:\\b.txt") },
             FileAction::SetAttributes,
+            FileAction::Open,
         ];
         for action in actions {
             let a = activity(action);
```

- [ ] **Step 4: Write the registry tests**

Replace the `tests` module at the bottom of `crates/atlas-schema/src/classes/registry.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::test_support::proc_ref;

    fn key(action: RegistryKeyAction) -> RegistryKeyActivity {
        RegistryKeyActivity { actor: proc_ref(), path: "HKLM\\SOFTWARE\\New".into(), path_unresolved: false, action }
    }

    fn set(value_type: RegType, data: Vec<u8>) -> RegistryValueActivity {
        RegistryValueActivity {
            actor: proc_ref(),
            key_path: "HKCU\\Software\\Run".into(),
            name: "x".into(),
            path_unresolved: false,
            action: RegistryValueAction::Set {
                value_type,
                data,
                data_truncated: false,
                data_read_after: false,
                data_unavailable: false,
            },
        }
    }

    fn dword(data: Vec<u8>) -> RegistryValueActivity {
        set(RegType::Known(RegValueType::Dword), data)
    }

    fn wire_set(w: &mut wire::RegistryValueActivity) -> &mut wire::RegistryValueSet {
        let Some(WV::Set(s)) = w.activity.as_mut() else { unreachable!() };
        s
    }

    fn rejected(w: wire::RegistryValueActivity) -> (String, SchemaErrorKind) {
        let e = RegistryValueActivity::from_wire(w).unwrap_err();
        (e.field_path, e.kind)
    }

    #[test]
    fn key_actions_round_trip() {
        for action in [
            RegistryKeyAction::Create,
            RegistryKeyAction::Delete,
            RegistryKeyAction::Rename { prev_path: "HKLM\\SOFTWARE\\Old".into() },
        ] {
            let a = key(action);
            assert_eq!(RegistryKeyActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn value_actions_round_trip() {
        let delete = RegistryValueActivity { action: RegistryValueAction::Delete, ..dword(vec![]) };
        for a in [dword(vec![1, 0, 0, 0]), delete] {
            assert_eq!(RegistryValueActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn reg_value_type_raw_values_match_windows_constants() {
        for raw in 0..=11 {
            assert_eq!(RegValueType::from_raw(raw).unwrap() as u32, raw);
        }
        assert_eq!(RegValueType::from_raw(12), None);
    }

    #[test]
    fn reg_type_from_raw_splits_at_reg_qword() {
        assert_eq!(RegType::from_raw(11), RegType::Known(RegValueType::Qword));
        assert_eq!(RegType::from_raw(12), RegType::Raw(12));
        assert_eq!(RegType::from_raw(u32::MAX), RegType::Raw(u32::MAX));
    }

    #[test]
    fn unknown_value_type_and_oversized_data_are_rejected() {
        let mut w = wire::RegistryValueActivity::from(dword(vec![]));
        wire_set(&mut w).r#type = Some(12);
        assert_eq!(rejected(w), ("reg_value.type".into(), SchemaErrorKind::UnknownEnum));

        let w = dword(vec![0; REG_DATA_MAX + 1]).into();
        assert_eq!(rejected(w), ("reg_value.data".into(), SchemaErrorKind::TooLarge));
    }

    #[test]
    fn sub_project_1_fields_round_trip() {
        let mut unresolved_key = key(RegistryKeyAction::Create);
        unresolved_key.path = "Software\\Relative".into();
        unresolved_key.path_unresolved = true;
        assert_eq!(RegistryKeyActivity::from_wire(unresolved_key.clone().into()).unwrap(), unresolved_key);

        let read_after = RegistryValueActivity {
            action: RegistryValueAction::Set {
                value_type: RegType::Known(RegValueType::Sz),
                data: vec![b'a', 0, 0, 0],
                data_truncated: false,
                data_read_after: true,
                data_unavailable: false,
            },
            ..dword(vec![])
        };
        let unavailable = RegistryValueActivity {
            path_unresolved: true,
            action: RegistryValueAction::Set {
                value_type: RegType::Raw(0x2000_0000),
                data: vec![],
                data_truncated: false,
                data_read_after: false,
                data_unavailable: true,
            },
            ..dword(vec![])
        };
        for a in [read_after, unavailable] {
            assert_eq!(RegistryValueActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn raw_type_travels_alone_on_the_wire() {
        let mut w = wire::RegistryValueActivity::from(set(RegType::Raw(12), vec![]));
        let s = wire_set(&mut w);
        assert_eq!((s.r#type, s.raw_type), (None, Some(12)));
    }

    #[test]
    fn raw_type_must_be_above_reg_qword_and_alone() {
        // In the known range: the sensor should have used `type`.
        let w = set(RegType::Raw(11), vec![]).into();
        assert_eq!(rejected(w), ("reg_value.raw_type".into(), SchemaErrorKind::Malformed));

        // Both set.
        let mut w = wire::RegistryValueActivity::from(dword(vec![]));
        wire_set(&mut w).raw_type = Some(12);
        assert_eq!(rejected(w), ("reg_value.raw_type".into(), SchemaErrorKind::Malformed));

        // Neither set: still 0a's Missing.
        let mut w = wire::RegistryValueActivity::from(dword(vec![]));
        wire_set(&mut w).r#type = None;
        assert_eq!(rejected(w), ("reg_value.type".into(), SchemaErrorKind::Missing));
    }

    #[test]
    fn data_unavailable_requires_no_data() {
        let mut w = wire::RegistryValueActivity::from(dword(vec![1, 0, 0, 0]));
        wire_set(&mut w).data_unavailable = true;
        assert_eq!(rejected(w), ("reg_value.data_unavailable".into(), SchemaErrorKind::Malformed));

        let mut w = wire::RegistryValueActivity::from(dword(vec![]));
        let s = wire_set(&mut w);
        (s.data_unavailable, s.data_truncated) = (true, true);
        assert_eq!(rejected(w), ("reg_value.data_unavailable".into(), SchemaErrorKind::Malformed));
    }
}
```

- [ ] **Step 5: Implement the registry changes**

Replace everything above the `tests` module with:

```rust
//! Registry Key Activity (OCSF 201001) and Registry Value Activity (OCSF 201002),
//! spec sections 5.6–5.7, plus the sub-project 1 fields (sensor spec §10.4).

use atlas_proto::v1 as wire;
use atlas_proto::v1::registry_key_activity::Activity as WK;
use atlas_proto::v1::registry_value_activity::Activity as WV;

use crate::convert::{Result, bounded, err, require};
use crate::error::SchemaErrorKind;
use crate::limits::{PATH_MAX, REG_DATA_MAX};
use crate::objects::ProcessRef;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryKeyActivity {
    pub actor: ProcessRef,
    /// `reg_key.path`. For `Rename`: the new path.
    pub path: String,
    /// `path` holds only what the sensor saw (a relative name, or empty), not a full path.
    pub path_unresolved: bool,
    pub action: RegistryKeyAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryKeyAction {
    Create,
    Delete,
    /// `prev_path` is `prev_reg_key.path`, the original path.
    Rename {
        prev_path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryValueActivity {
    pub actor: ProcessRef,
    /// `reg_value.path`: the containing key.
    pub key_path: String,
    /// `reg_value.name`; empty means the default value of the key.
    pub name: String,
    /// `key_path` holds only what the sensor saw (a relative name, or empty), not a full path.
    pub path_unresolved: bool,
    pub action: RegistryValueAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryValueAction {
    Set {
        value_type: RegType,
        data: Vec<u8>,
        data_truncated: bool,
        /// `data` was read from the registry after the event, not captured with it.
        data_read_after: bool,
        /// No data was obtained. Requires empty `data` and `data_truncated == false`.
        data_unavailable: bool,
    },
    Delete,
}

/// The type of a set value: a `REG_*` constant 0–11, or a raw number above
/// `REG_QWORD` (sensor spec §7.5), which travels as `raw_type` on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegType {
    Known(RegValueType),
    /// Must be greater than 11; anything else is rejected on decode.
    Raw(u32),
}

/// Windows `REG_*` value types. Discriminants are the Windows constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RegValueType {
    None = 0,
    Sz = 1,
    ExpandSz = 2,
    Binary = 3,
    Dword = 4,
    DwordBigEndian = 5,
    Link = 6,
    MultiSz = 7,
    ResourceList = 8,
    FullResourceDescriptor = 9,
    ResourceRequirementsList = 10,
    Qword = 11,
}

impl RegValueType {
    pub fn from_raw(raw: u32) -> Option<Self> {
        use RegValueType::*;
        Some(match raw {
            0 => None,
            1 => Sz,
            2 => ExpandSz,
            3 => Binary,
            4 => Dword,
            5 => DwordBigEndian,
            6 => Link,
            7 => MultiSz,
            8 => ResourceList,
            9 => FullResourceDescriptor,
            10 => ResourceRequirementsList,
            11 => Qword,
            _ => return Option::None,
        })
    }
}

impl RegType {
    /// The sensor's mapping from the raw type in an event.
    pub fn from_raw(raw: u32) -> Self {
        match RegValueType::from_raw(raw) {
            Some(t) => Self::Known(t),
            None => Self::Raw(raw),
        }
    }
}

impl From<RegistryKeyActivity> for wire::RegistryKeyActivity {
    fn from(v: RegistryKeyActivity) -> Self {
        let activity = match v.action {
            RegistryKeyAction::Create => WK::Create(wire::RegistryKeyCreate {}),
            RegistryKeyAction::Delete => WK::Delete(wire::RegistryKeyDelete {}),
            RegistryKeyAction::Rename { prev_path } => WK::Rename(wire::RegistryKeyRename { prev_path }),
        };
        Self { actor: Some(v.actor.into()), path: v.path, path_unresolved: v.path_unresolved, activity: Some(activity) }
    }
}

impl From<RegistryValueActivity> for wire::RegistryValueActivity {
    fn from(v: RegistryValueActivity) -> Self {
        let activity = match v.action {
            RegistryValueAction::Set { value_type, data, data_truncated, data_read_after, data_unavailable } => {
                let (r#type, raw_type) = match value_type {
                    RegType::Known(t) => (Some(t as u32), None),
                    RegType::Raw(raw) => (None, Some(raw)),
                };
                WV::Set(wire::RegistryValueSet {
                    r#type,
                    data,
                    data_truncated,
                    data_read_after,
                    data_unavailable,
                    raw_type,
                })
            }
            RegistryValueAction::Delete => WV::Delete(wire::RegistryValueDelete {}),
        };
        Self {
            actor: Some(v.actor.into()),
            key_path: v.key_path,
            name: v.name,
            path_unresolved: v.path_unresolved,
            activity: Some(activity),
        }
    }
}

impl RegistryKeyActivity {
    pub(crate) fn from_wire(w: wire::RegistryKeyActivity) -> Result<Self> {
        // Activity first: an unknown (newer) activity must read as `activity: Missing`.
        let activity = require(w.activity, "", "activity")?;
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            path: bounded(w.path, PATH_MAX, "", "reg_key.path")?,
            path_unresolved: w.path_unresolved,
            action: match activity {
                WK::Create(_) => RegistryKeyAction::Create,
                WK::Delete(_) => RegistryKeyAction::Delete,
                WK::Rename(r) => {
                    RegistryKeyAction::Rename { prev_path: bounded(r.prev_path, PATH_MAX, "", "prev_reg_key.path")? }
                }
            },
        })
    }
}

impl RegistryValueActivity {
    pub(crate) fn from_wire(w: wire::RegistryValueActivity) -> Result<Self> {
        // Activity first: an unknown (newer) activity must read as `activity: Missing`.
        let activity = require(w.activity, "", "activity")?;
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            key_path: bounded(w.key_path, PATH_MAX, "", "reg_value.path")?,
            name: bounded(w.name, PATH_MAX, "", "reg_value.name")?,
            path_unresolved: w.path_unresolved,
            action: match activity {
                WV::Set(s) => set_from_wire(s)?,
                WV::Delete(_) => RegistryValueAction::Delete,
            },
        })
    }
}

fn set_from_wire(s: wire::RegistryValueSet) -> Result<RegistryValueAction> {
    // Exactly one of `type` (0–11) and `raw_type` (> 11). Both absent keeps 0a's
    // `reg_value.type: Missing`, so every message 0a accepts is still accepted.
    let value_type = match (s.r#type, s.raw_type) {
        (Some(raw), None) => match RegValueType::from_raw(raw) {
            Some(t) => RegType::Known(t),
            None => return err("", "reg_value.type", SchemaErrorKind::UnknownEnum),
        },
        (None, Some(raw)) if RegValueType::from_raw(raw).is_none() => RegType::Raw(raw),
        (None, None) => return err("", "reg_value.type", SchemaErrorKind::Missing),
        // A raw type in the known range, or both fields set.
        _ => return err("", "reg_value.raw_type", SchemaErrorKind::Malformed),
    };
    if s.data.len() > REG_DATA_MAX {
        return err("", "reg_value.data", SchemaErrorKind::TooLarge);
    }
    if s.data_unavailable && (!s.data.is_empty() || s.data_truncated) {
        return err("", "reg_value.data_unavailable", SchemaErrorKind::Malformed);
    }
    Ok(RegistryValueAction::Set {
        value_type,
        data: s.data,
        data_truncated: s.data_truncated,
        data_read_after: s.data_read_after,
        data_unavailable: s.data_unavailable,
    })
}
```

- [ ] **Step 6: Write the Event Log Activity tests**

Create `crates/atlas-schema/src/classes/event_log.rs` containing only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::test_support::proc_ref;

    fn activity(action: EventLogAction) -> EventLogActivity {
        EventLogActivity {
            actor: None,
            log_name: "Atlas-Sensor".into(),
            log_provider: "Microsoft-Windows-Kernel-File".into(),
            status_code: Some(4201),
            action,
        }
    }

    fn rejected(w: wire::EventLogActivity) -> (String, SchemaErrorKind) {
        let e = EventLogActivity::from_wire(w).unwrap_err();
        (e.field_path, e.kind)
    }

    #[test]
    fn every_action_round_trips_with_and_without_actor() {
        for action in [EventLogAction::Stop, EventLogAction::Restart, EventLogAction::Disable] {
            let a = activity(action);
            assert_eq!(EventLogActivity::from_wire(a.clone().into()).unwrap(), a);
            let a = EventLogActivity { actor: Some(proc_ref()), status_code: None, ..a };
            assert_eq!(EventLogActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn log_name_is_required_and_bounded() {
        let w = wire::EventLogActivity { log_name: String::new(), ..activity(EventLogAction::Stop).into() };
        assert_eq!(rejected(w), ("log_name".into(), SchemaErrorKind::Missing));
        let w = wire::EventLogActivity {
            log_name: "a".repeat(EVENT_LOG_NAME_MAX + 1),
            ..activity(EventLogAction::Stop).into()
        };
        assert_eq!(rejected(w), ("log_name".into(), SchemaErrorKind::TooLarge));
    }

    #[test]
    fn provider_is_required_only_for_disable() {
        let w = wire::EventLogActivity { log_provider: String::new(), ..activity(EventLogAction::Disable).into() };
        assert_eq!(rejected(w), ("log_provider".into(), SchemaErrorKind::Missing));
        for action in [EventLogAction::Stop, EventLogAction::Restart] {
            let a = EventLogActivity { log_provider: String::new(), ..activity(action) };
            assert_eq!(EventLogActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn a_present_actor_is_validated() {
        let mut w = wire::EventLogActivity::from(EventLogActivity {
            actor: Some(proc_ref()),
            ..activity(EventLogAction::Stop)
        });
        w.actor.as_mut().unwrap().uid.pop();
        assert_eq!(rejected(w), ("actor.process.uid".into(), SchemaErrorKind::Malformed));
    }
}
```

- [ ] **Step 7: Implement Event Log Activity**

Add above the test module:

```rust
//! Event Log Activity (OCSF 1008), sensor spec §10.2: tampering the watchdog
//! sees on the agent's own ETW sessions.

use atlas_proto::v1 as wire;
use atlas_proto::v1::event_log_activity::Activity as W;

use crate::convert::{Result, bounded, err, require};
use crate::error::SchemaErrorKind;
use crate::limits::EVENT_LOG_NAME_MAX;
use crate::objects::ProcessRef;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLogActivity {
    /// Optional: the watchdog sees the effect, not who caused it.
    pub actor: Option<ProcessRef>,
    /// `log_name`: the ETW session name. Never empty.
    pub log_name: String,
    /// `log_provider`: the ETW provider name. Never empty for `Disable`.
    pub log_provider: String,
    /// `status_code`: the Win32 error or NTSTATUS that revealed the change.
    pub status_code: Option<u32>,
    pub action: EventLogAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventLogAction {
    /// The session was found stopped, or replaced by another session with its name.
    Stop,
    /// The agent recreated the session.
    Restart,
    /// A provider was disabled or changed in the session, or its canary went silent.
    Disable,
}

impl From<EventLogActivity> for wire::EventLogActivity {
    fn from(v: EventLogActivity) -> Self {
        let activity = match v.action {
            EventLogAction::Stop => W::Stop(wire::EventLogStop {}),
            EventLogAction::Restart => W::Restart(wire::EventLogRestart {}),
            EventLogAction::Disable => W::Disable(wire::EventLogDisable {}),
        };
        Self {
            actor: v.actor.map(Into::into),
            log_name: v.log_name,
            log_provider: v.log_provider,
            status_code: v.status_code,
            activity: Some(activity),
        }
    }
}

impl EventLogActivity {
    pub(crate) fn from_wire(w: wire::EventLogActivity) -> Result<Self> {
        // Activity first: an unknown (newer) activity must read as `activity: Missing`.
        let activity = require(w.activity, "", "activity")?;
        let actor = match w.actor {
            Some(a) => Some(ProcessRef::from_wire(a, "actor.process")?),
            None => None,
        };
        let log_name = bounded(w.log_name, EVENT_LOG_NAME_MAX, "", "log_name")?;
        if log_name.is_empty() {
            return err("", "log_name", SchemaErrorKind::Missing);
        }
        let log_provider = bounded(w.log_provider, EVENT_LOG_NAME_MAX, "", "log_provider")?;
        let action = match activity {
            W::Stop(_) => EventLogAction::Stop,
            W::Restart(_) => EventLogAction::Restart,
            W::Disable(_) => EventLogAction::Disable,
        };
        if action == EventLogAction::Disable && log_provider.is_empty() {
            return err("", "log_provider", SchemaErrorKind::Missing);
        }
        Ok(Self { actor, log_name, log_provider, status_code: w.status_code, action })
    }
}
```

- [ ] **Step 8: Write the Sensor Health tests**

Create `crates/atlas-schema/src/classes/sensor_health.rs` containing only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn report(r: HealthReport) -> SensorHealthActivity {
        SensorHealthActivity {
            interval_start: 1_790_000_000_000_000_000,
            action: SensorHealthAction::Report(Box::new(r)),
        }
    }

    fn rejected(w: wire::SensorHealth) -> (String, SchemaErrorKind) {
        let e = SensorHealthActivity::from_wire(w).unwrap_err();
        (e.field_path, e.kind)
    }

    fn wire_report(a: SensorHealthActivity) -> (wire::SensorHealth, wire::SensorHealthReport) {
        let w = wire::SensorHealth::from(a);
        let Some(W::Report(r)) = w.activity.clone() else { unreachable!() };
        (w, r)
    }

    #[test]
    fn empty_report_round_trips_and_sends_no_groups() {
        let a = report(HealthReport::default());
        assert_eq!(SensorHealthActivity::from_wire(a.clone().into()).unwrap(), a);
        let (_, r) = wire_report(a);
        assert_eq!(r, wire::SensorHealthReport::default());
    }

    #[test]
    fn full_report_round_trips() {
        let a = report(HealthReport {
            loss: SensorLoss {
                kernel_queue_drops: Some(0),
                actor_dropped: vec![ClassCount { class_uid: 4001, count: 3 }],
                ..Default::default()
            },
            quality: SensorQuality {
                late_arrivals: Some(12),
                actor_unresolved: vec![ClassCount { class_uid: 1001, count: 1 }],
                ..Default::default()
            },
            housekeeping: SensorHousekeeping {
                seeding_enabled: Some(false),
                retention_evictions: Some(2),
                ..Default::default()
            },
            resources: SensorResources { cpu_time: Some(250_000_000), working_set: Some(40 << 20) },
            gap: Some(SensorGap { first_time: Some(1), last_time: Some(2), events: 9 }),
            buffer: SensorBuffer { failing: Some(true), write_errors: Some(3), ..Default::default() },
        });
        assert_eq!(SensorHealthActivity::from_wire(a.clone().into()).unwrap(), a);
    }

    #[test]
    fn class_count_lists_are_bounded() {
        let many = vec![ClassCount { class_uid: 1001, count: 1 }; CLASS_COUNTS_MAX + 1];
        let a = report(HealthReport {
            quality: SensorQuality { actor_unresolved: many, ..Default::default() },
            ..Default::default()
        });
        assert_eq!(rejected(a.into()), ("quality.actor_unresolved".into(), SchemaErrorKind::TooLarge));

        let at_limit = vec![ClassCount { class_uid: 1001, count: 1 }; CLASS_COUNTS_MAX];
        let a = report(HealthReport {
            loss: SensorLoss { actor_dropped: at_limit, ..Default::default() },
            ..Default::default()
        });
        assert_eq!(SensorHealthActivity::from_wire(a.clone().into()).unwrap(), a);
    }

    #[test]
    fn gap_times_are_both_or_neither_and_ordered() {
        let gap = |first_time, last_time| {
            report(HealthReport { gap: Some(SensorGap { first_time, last_time, events: 1 }), ..Default::default() })
        };
        assert_eq!(rejected(gap(Some(5), Some(4)).into()), ("gap.last_time".into(), SchemaErrorKind::Malformed));
        assert_eq!(rejected(gap(Some(5), None).into()), ("gap".into(), SchemaErrorKind::Malformed));
        assert_eq!(rejected(gap(None, Some(5)).into()), ("gap".into(), SchemaErrorKind::Malformed));
        for ok in [gap(None, None), gap(Some(5), Some(5))] {
            assert_eq!(SensorHealthActivity::from_wire(ok.clone().into()).unwrap(), ok);
        }
    }
}
```

- [ ] **Step 9: Implement Sensor Health**

Add above the test module. `counter_group!` writes each group's struct and both conversions from one field list, so a field cannot be missing from either direction. `GroupField` copies scalars and validates the per-class lists.

```rust
//! Sensor Health (Atlas extension class, sensor spec §10.3): the agent's own
//! loss, quality, housekeeping and resource figures.
//!
//! Counters count occurrences during `[interval_start, meta.time]`; gauges are
//! sampled at the end of the interval. `None` means not measured; `Some(0)`
//! means measured and none occurred.

use atlas_proto::v1 as wire;
use atlas_proto::v1::sensor_health::Activity as W;

use crate::convert::{Result, err, require};
use crate::error::SchemaErrorKind;
use crate::limits::CLASS_COUNTS_MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorHealthActivity {
    /// Start of the interval: nanoseconds since the Unix epoch, UTC.
    pub interval_start: i64,
    pub action: SensorHealthAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensorHealthAction {
    /// Boxed: a report is much larger than any other event body.
    Report(Box<HealthReport>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HealthReport {
    pub loss: SensorLoss,
    pub quality: SensorQuality,
    pub housekeeping: SensorHousekeeping,
    pub resources: SensorResources,
    /// Set when buffered events were deleted before delivery (overflow).
    pub gap: Option<SensorGap>,
    pub buffer: SensorBuffer,
}

/// A count for one event class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassCount {
    /// OCSF `class_uid`.
    pub class_uid: u32,
    pub count: u64,
}

/// Buffered events deleted before delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorGap {
    /// Event-time range of the deleted events (ns since the Unix epoch, UTC).
    /// Both `None` when none of the deleted records could be decoded.
    pub first_time: Option<i64>,
    pub last_time: Option<i64>,
    pub events: u64,
}

/// Converts one field of a counter group. Identity for scalars; validated for lists.
trait GroupField: Sized {
    type Wire;
    fn into_wire(self) -> Self::Wire;
    fn from_wire(w: Self::Wire, path: &str, field: &str) -> Result<Self>;
}

impl GroupField for Option<u64> {
    type Wire = Self;
    fn into_wire(self) -> Self {
        self
    }
    fn from_wire(w: Self, _: &str, _: &str) -> Result<Self> {
        Ok(w)
    }
}

impl GroupField for Option<bool> {
    type Wire = Self;
    fn into_wire(self) -> Self {
        self
    }
    fn from_wire(w: Self, _: &str, _: &str) -> Result<Self> {
        Ok(w)
    }
}

impl GroupField for Vec<ClassCount> {
    type Wire = Vec<wire::ClassCount>;
    fn into_wire(self) -> Self::Wire {
        self.into_iter().map(|c| wire::ClassCount { class_uid: c.class_uid, count: c.count }).collect()
    }
    fn from_wire(w: Self::Wire, path: &str, field: &str) -> Result<Self> {
        if w.len() > CLASS_COUNTS_MAX {
            return err(path, field, SchemaErrorKind::TooLarge);
        }
        Ok(w.into_iter().map(|c| ClassCount { class_uid: c.class_uid, count: c.count }).collect())
    }
}

/// A group of counters: the domain struct, and conversions that copy (or, for
/// lists, validate) each field. `path` is the group's field path for errors.
macro_rules! counter_group {
    ($(#[$doc:meta])* $name:ident, $path:literal { $($(#[$fdoc:meta])* $field:ident: $ty:ty,)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Default, PartialEq, Eq)]
        pub struct $name {
            $($(#[$fdoc])* pub $field: $ty,)*
        }

        impl From<$name> for wire::$name {
            fn from(v: $name) -> Self {
                Self { $($field: GroupField::into_wire(v.$field),)* }
            }
        }

        impl $name {
            fn from_wire(w: Option<wire::$name>) -> Result<Self> {
                let Some(w) = w else { return Ok(Self::default()) };
                Ok(Self { $($field: GroupField::from_wire(w.$field, $path, stringify!($field))?,)* })
            }

            /// `None` for an all-absent group, so empty groups cost nothing on the wire.
            fn into_wire(self) -> Option<wire::$name> {
                (self != Self::default()).then(|| self.into())
            }
        }
    };
}

counter_group!(
    /// Events that never reached the buffer.
    SensorLoss, "loss" {
        /// ETW events lost by the `Atlas-Sensor` session.
        sensor_session_events_lost: Option<u64>,
        /// ETW events lost by the `Atlas-Process` session.
        process_session_events_lost: Option<u64>,
        sensor_session_buffers_lost: Option<u64>,
        process_session_buffers_lost: Option<u64>,
        kernel_queue_drops: Option<u64>,
        user_queue_drops: Option<u64>,
        dns_rate_limit_drops: Option<u64>,
        /// Dropped because no process uid could be computed, per class.
        actor_dropped: Vec<ClassCount>,
        /// Dropped because the buffer's in-memory backlog was full.
        buffer_backlog_drops: Option<u64>,
    }
);

counter_group!(
    /// Events emitted with less than full information, or handled outside the usual path.
    SensorQuality, "quality" {
        late_arrivals: Option<u64>,
        parse_errors: Option<u64>,
        unknown_version: Option<u64>,
        launch_join_miss: Option<u64>,
        /// Emitted with an unresolved actor (empty path and name), per class.
        actor_unresolved: Vec<ClassCount>,
        unknown_file_object: Option<u64>,
        registry_unresolved: Option<u64>,
        value_read_failed: Option<u64>,
        early_read_redone: Option<u64>,
        reg_type_unusual: Option<u64>,
        file_op_late_failure: Option<u64>,
        writes_after_cleanup: Option<u64>,
        file_object_replaced: Option<u64>,
        /// Invalid records found when the buffer was replayed.
        buffer_invalid_records: Option<u64>,
        enrichment_misses: Option<u64>,
        enrichment_errors: Option<u64>,
    }
);

counter_group!(
    /// Evictions from bounded structures, and expected drops.
    SensorHousekeeping, "housekeeping" {
        process_cache_evictions: Option<u64>,
        file_map_evictions: Option<u64>,
        key_map_evictions: Option<u64>,
        early_key_map_evictions: Option<u64>,
        flow_table_evictions: Option<u64>,
        hash_cache_evictions: Option<u64>,
        /// Whole buffer segments deleted by rolling retention (expected without a transport).
        retention_evictions: Option<u64>,
        /// Failed file creates, deletes and renames that were dropped.
        file_op_failed: Option<u64>,
        pending_overflow: Option<u64>,
        seeding_enabled: Option<bool>,
        seeder_handles_named: Option<u64>,
        seeder_handles_failed: Option<u64>,
        seeder_handles_timed_out: Option<u64>,
        seeder_table_reads: Option<u64>,
        seeder_deferred_rereads: Option<u64>,
        /// Gauge.
        seeder_stuck_helpers: Option<u64>,
        /// Gauge.
        seeder_negative_cache_size: Option<u64>,
    }
);

counter_group!(
    /// The agent's own resource use.
    SensorResources, "resources" {
        /// Process CPU time (user + kernel) during the interval, in nanoseconds.
        cpu_time: Option<u64>,
        /// Working set at the end of the interval, in bytes (a gauge).
        working_set: Option<u64>,
    }
);

counter_group!(
    /// The agent's on-disk buffer (sensor spec §8).
    SensorBuffer, "buffer" {
        write_errors: Option<u64>,
        recoveries: Option<u64>,
        /// Gauge: writes were failing at the end of the interval.
        failing: Option<bool>,
        /// Records refused: empty or over 256 KiB (an agent defect).
        rejected: Option<u64>,
        /// Segments another process held open, so they could not be deleted.
        delete_failures: Option<u64>,
        /// Found at startup; reported once, in the first report.
        truncated_bytes: Option<u64>,
        cursor_reset: Option<bool>,
        foreign_segments: Option<u64>,
        /// Sealed segments whose remainder a reader skipped as corrupt.
        corrupt_segments: Option<u64>,
        /// Gauge: bytes in all segments.
        disk_bytes: Option<u64>,
    }
);

impl From<SensorGap> for wire::SensorGap {
    fn from(v: SensorGap) -> Self {
        Self { first_time: v.first_time, last_time: v.last_time, events: v.events }
    }
}

impl SensorGap {
    fn from_wire(w: wire::SensorGap) -> Result<Self> {
        match (w.first_time, w.last_time) {
            (Some(first), Some(last)) if first > last => return err("gap", "last_time", SchemaErrorKind::Malformed),
            (Some(_), None) | (None, Some(_)) => return err("", "gap", SchemaErrorKind::Malformed),
            _ => {}
        }
        Ok(Self { first_time: w.first_time, last_time: w.last_time, events: w.events })
    }
}

impl From<SensorHealthActivity> for wire::SensorHealth {
    fn from(v: SensorHealthActivity) -> Self {
        let activity = match v.action {
            SensorHealthAction::Report(r) => W::Report(wire::SensorHealthReport {
                loss: r.loss.into_wire(),
                quality: r.quality.into_wire(),
                housekeeping: r.housekeeping.into_wire(),
                resources: r.resources.into_wire(),
                gap: r.gap.map(Into::into),
                buffer: r.buffer.into_wire(),
            }),
        };
        Self { interval_start: v.interval_start, activity: Some(activity) }
    }
}

impl SensorHealthActivity {
    pub(crate) fn from_wire(w: wire::SensorHealth) -> Result<Self> {
        // Activity first: an unknown (newer) activity must read as `activity: Missing`.
        let action = match require(w.activity, "", "activity")? {
            W::Report(r) => SensorHealthAction::Report(Box::new(HealthReport {
                loss: SensorLoss::from_wire(r.loss)?,
                quality: SensorQuality::from_wire(r.quality)?,
                housekeeping: SensorHousekeeping::from_wire(r.housekeeping)?,
                resources: SensorResources::from_wire(r.resources)?,
                gap: match r.gap {
                    Some(g) => Some(SensorGap::from_wire(g)?),
                    None => None,
                },
                buffer: SensorBuffer::from_wire(r.buffer)?,
            })),
        };
        Ok(Self { interval_start: w.interval_start, action })
    }
}
```

- [ ] **Step 10: Run the unit tests**

```powershell
cargo test -p atlas-schema --lib
```
Expected: `test result: ok. 48 passed` (35 from 0a; 13 new or extended).

### Task 3: Integration tests, golden fixtures, commit

**Files:**
- Modify: `crates/atlas-schema/tests/common/mod.rs` (replace), `tests/validation.rs`, `tests/ocsf_ids.rs`
- Create: `crates/atlas-schema/tests/fixtures/{file_open, registry_key_create_unresolved, registry_value_set_read_after, registry_value_set_unavailable, event_log_stop, event_log_restart, event_log_disable, sensor_health_report}.json` (generated)

**Interfaces:**
- Consumes: Task 2.
- Produces: one named sample per new class, activity and validation-relevant variant, and arbitrary-event strategies that cover the new classes. The round-trip and hostile-input property tests and the codec benchmark pick these up unchanged.

- [ ] **Step 1: Replace `tests/common/mod.rs`**

The changes from 0a are: the new fields on the existing samples, eight new samples, `arb_reg_type`, `arb_health_report`, and Event Log and Sensor Health in `arb_kind`. Generated events respect the new cross-field rules: unavailable data is empty and not truncated, both gap times or neither, and non-empty log names.

`crates/atlas-schema/tests/common/mod.rs`:
```rust
//! Shared test data: one named sample per class/activity, and proptest
//! strategies that generate arbitrary *valid* domain events.

#![allow(dead_code)] // each test binary uses a different subset

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use atlas_schema::classes::dns::{DnsAction, DnsActivity, DnsAnswer};
use atlas_schema::classes::event_log::{EventLogAction, EventLogActivity};
use atlas_schema::classes::file::{FileAction, FileSystemActivity};
use atlas_schema::classes::module::{ModuleAction, ModuleActivity};
use atlas_schema::classes::network::{NetworkAction, NetworkActivity, NetworkDirection, NetworkProtocol};
use atlas_schema::classes::process::ProcessActivity;
use atlas_schema::classes::registry::{
    RegType, RegValueType, RegistryKeyAction, RegistryKeyActivity, RegistryValueAction, RegistryValueActivity,
};
use atlas_schema::classes::sensor_health::{
    ClassCount, HealthReport, SensorBuffer, SensorGap, SensorHealthAction, SensorHealthActivity, SensorHousekeeping,
    SensorLoss, SensorQuality, SensorResources,
};
use atlas_schema::*;
use proptest::prelude::*;

// ---------------------------------------------------------------- samples

pub fn fixed_event_id() -> EventId {
    // 0192... is a valid UUIDv7 (version nibble 7, RFC 9562 variant).
    EventId::from_uuid(uuid::Uuid::from_bytes([
        0x01, 0x92, 0x2a, 0x6e, 0x5b, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    ]))
    .expect("valid v7")
}

pub fn device() -> Device {
    Device { uid: DeviceUid::from_bytes([0xd0; 16]), boot_id: BootId::from_bytes([0xb0; 16]) }
}

pub fn file(path: &str) -> File {
    let name = path.rsplit('\\').next().unwrap_or(path).to_owned();
    File { path: path.to_owned(), name, hashes: None, signature: None }
}

pub fn user() -> User {
    User { uid: "S-1-5-21-1000-1000-1000-1001".into(), name: "DESK-01\\jake".into() }
}

pub fn proc_ref(start_key: u64, path: &str, pid: u32) -> ProcessRef {
    let d = device();
    ProcessRef { uid: process_uid(&d.uid, &d.boot_id, start_key), pid, file: file(path), user: Some(user()) }
}

pub fn actor() -> ProcessRef {
    proc_ref(7788, "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", 7788)
}

pub fn event(kind: EventKind) -> Event {
    Event {
        meta: EventMeta { event_id: fixed_event_id(), time: 1_790_000_000_123_456_789, sensor: Sensor::Etw },
        device: device(),
        kind,
    }
}

fn file_event(action: FileAction) -> Event {
    event(EventKind::File(FileSystemActivity { actor: actor(), file: file("C:\\Users\\jake\\a.txt"), action }))
}

fn net_event(action: NetworkAction) -> Event {
    event(EventKind::Network(NetworkActivity {
        actor: actor(),
        src_endpoint: NetworkEndpoint { ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)), port: 50123 },
        dst_endpoint: NetworkEndpoint { ip: IpAddr::V6(Ipv6Addr::LOCALHOST), port: 443 },
        protocol: NetworkProtocol::Tcp,
        direction: NetworkDirection::Outbound,
        action,
    }))
}

fn reg_key_event(action: RegistryKeyAction) -> Event {
    event(EventKind::RegistryKey(RegistryKeyActivity {
        actor: actor(),
        path: "HKLM\\SOFTWARE\\Atlas\\New".into(),
        path_unresolved: false,
        action,
    }))
}

fn reg_value_event(action: RegistryValueAction) -> Event {
    event(EventKind::RegistryValue(RegistryValueActivity {
        actor: actor(),
        key_path: "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run".into(),
        name: "Updater".into(),
        path_unresolved: false,
        action,
    }))
}

fn event_log_event(action: EventLogAction, log_provider: &str) -> Event {
    event(EventKind::EventLog(EventLogActivity {
        actor: None,
        log_name: "Atlas-Sensor".into(),
        log_provider: log_provider.into(),
        status_code: None,
        action,
    }))
}

/// One valid event per (class, activity), named `<class>_<activity>`, plus
/// named variants for fields that change validation (`*_read_after`, …).
pub fn samples() -> Vec<(&'static str, Event)> {
    let launch = ProcessActivity::Launch {
        actor: proc_ref(4120, "C:\\Program Files\\Microsoft Office\\root\\Office16\\WINWORD.EXE", 4120),
        process: Process {
            uid: actor().uid,
            pid: 7788,
            file: File {
                hashes: Some(Hashes { sha256: Some([0xab; 32]) }),
                signature: Some(Signature { signer: Some("Microsoft Windows".into()), status: SignatureStatus::Valid }),
                ..file("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe")
            },
            user: Some(user()),
            cmd_line: "powershell.exe -enc SQBFAFgA".into(),
            cmd_line_truncated: false,
            created_time: 1_790_000_000_123_000_000,
            integrity: Some(Integrity::Medium),
            parent_process: Some(proc_ref(
                4120,
                "C:\\Program Files\\Microsoft Office\\root\\Office16\\WINWORD.EXE",
                4120,
            )),
        },
    };
    vec![
        ("process_launch", event(EventKind::Process(launch))),
        (
            "process_terminate",
            event(EventKind::Process(ProcessActivity::Terminate { process: actor(), exit_code: Some(0) })),
        ),
        (
            "module_load",
            event(EventKind::Module(ModuleActivity {
                actor: actor(),
                action: ModuleAction::Load {
                    file: file("C:\\Windows\\System32\\amsi.dll"),
                    base_address: 0x7ffb_1234_0000,
                },
            })),
        ),
        ("network_open", net_event(NetworkAction::Open)),
        ("network_close", net_event(NetworkAction::Close { bytes_in: Some(5120), bytes_out: Some(812) })),
        ("file_create", file_event(FileAction::Create)),
        ("file_read", file_event(FileAction::Read)),
        ("file_update", file_event(FileAction::Update)),
        ("file_delete", file_event(FileAction::Delete)),
        ("file_rename", file_event(FileAction::Rename { file_result: file("C:\\Users\\jake\\a.txt.locked") })),
        ("file_set_attributes", file_event(FileAction::SetAttributes)),
        ("file_open", file_event(FileAction::Open)),
        ("registry_key_create", reg_key_event(RegistryKeyAction::Create)),
        ("registry_key_delete", reg_key_event(RegistryKeyAction::Delete)),
        (
            "registry_key_rename",
            reg_key_event(RegistryKeyAction::Rename { prev_path: "HKLM\\SOFTWARE\\Atlas\\Old".into() }),
        ),
        (
            "registry_key_create_unresolved",
            event(EventKind::RegistryKey(RegistryKeyActivity {
                actor: actor(),
                path: "Atlas\\New".into(),
                path_unresolved: true,
                action: RegistryKeyAction::Create,
            })),
        ),
        (
            "registry_value_set",
            reg_value_event(RegistryValueAction::Set {
                value_type: RegType::Known(RegValueType::Sz),
                data: "C:\\Users\\Public\\u.exe\0".encode_utf16().flat_map(u16::to_le_bytes).collect(),
                data_truncated: false,
                data_read_after: false,
                data_unavailable: false,
            }),
        ),
        (
            "registry_value_set_read_after",
            reg_value_event(RegistryValueAction::Set {
                value_type: RegType::Known(RegValueType::Dword),
                data: vec![1, 0, 0, 0],
                data_truncated: false,
                data_read_after: true,
                data_unavailable: false,
            }),
        ),
        (
            "registry_value_set_unavailable",
            event(EventKind::RegistryValue(RegistryValueActivity {
                actor: actor(),
                key_path: "Software\\Microsoft\\Windows\\CurrentVersion\\Run".into(),
                name: "Updater".into(),
                path_unresolved: true,
                action: RegistryValueAction::Set {
                    value_type: RegType::Raw(0x0020_0000),
                    data: vec![],
                    data_truncated: false,
                    data_read_after: false,
                    data_unavailable: true,
                },
            })),
        ),
        ("registry_value_delete", reg_value_event(RegistryValueAction::Delete)),
        (
            "dns_response",
            event(EventKind::Dns(DnsActivity {
                actor: actor(),
                hostname: "example.com".into(),
                query_type: 28,
                action: DnsAction::Response {
                    rcode: Some(0),
                    platform_status: Some(0),
                    answers: vec![DnsAnswer { rr_type: 28, data: "2606:2800:21f:cb07:6820:80da:af6b:8b2c".into() }],
                },
            })),
        ),
        ("event_log_stop", event_log_event(EventLogAction::Stop, "")),
        ("event_log_restart", event_log_event(EventLogAction::Restart, "")),
        ("event_log_disable", event_log_event(EventLogAction::Disable, "Microsoft-Windows-Kernel-Registry")),
        (
            "sensor_health_report",
            event(EventKind::SensorHealth(SensorHealthActivity {
                interval_start: 1_790_000_000_000_000_000,
                action: SensorHealthAction::Report(Box::new(HealthReport {
                    loss: SensorLoss {
                        sensor_session_events_lost: Some(0),
                        kernel_queue_drops: Some(0),
                        actor_dropped: vec![ClassCount { class_uid: 4001, count: 2 }],
                        ..Default::default()
                    },
                    quality: SensorQuality {
                        late_arrivals: Some(17),
                        actor_unresolved: vec![ClassCount { class_uid: 1001, count: 3 }],
                        ..Default::default()
                    },
                    housekeeping: SensorHousekeeping {
                        retention_evictions: Some(1),
                        seeding_enabled: Some(true),
                        seeder_handles_named: Some(14_139),
                        ..Default::default()
                    },
                    resources: SensorResources { cpu_time: Some(480_000_000), working_set: Some(41_943_040) },
                    gap: Some(SensorGap {
                        first_time: Some(1_789_999_000_000_000_000),
                        last_time: Some(1_789_999_100_000_000_000),
                        events: 52_000,
                    }),
                    buffer: SensorBuffer {
                        write_errors: Some(0),
                        failing: Some(false),
                        disk_bytes: Some(734_003_200),
                        ..Default::default()
                    },
                })),
            })),
        ),
    ]
}

// ------------------------------------------------------------- strategies

pub fn arb_string(max_chars: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..=max_chars).prop_map(String::from_iter)
}

fn arb_uid<T: 'static + std::fmt::Debug>(f: fn([u8; 16]) -> T) -> impl Strategy<Value = T> {
    any::<[u8; 16]>().prop_map(f)
}

pub fn arb_event_id() -> impl Strategy<Value = EventId> {
    (0u64..(1 << 48), any::<[u8; 10]>()).prop_map(|(ms, rand)| {
        EventId::from_uuid(uuid::Builder::from_unix_timestamp_millis(ms, &rand).into_uuid()).expect("v7")
    })
}

pub fn arb_user() -> BoxedStrategy<User> {
    (arb_string(20), arb_string(20)).prop_map(|(uid, name)| User { uid, name }).boxed()
}

pub fn arb_file() -> BoxedStrategy<File> {
    let status =
        prop_oneof![Just(SignatureStatus::Valid), Just(SignatureStatus::Invalid), Just(SignatureStatus::Unsigned)];
    (
        arb_string(40),
        arb_string(20),
        prop::option::of(prop::option::of(any::<[u8; 32]>()).prop_map(|sha256| Hashes { sha256 })),
        prop::option::of(
            (prop::option::of(arb_string(20)), status).prop_map(|(signer, status)| Signature { signer, status }),
        ),
    )
        .prop_map(|(path, name, hashes, signature)| File { path, name, hashes, signature })
        .boxed()
}

pub fn arb_process_ref() -> BoxedStrategy<ProcessRef> {
    (arb_uid(ProcessUid::from_bytes), any::<u32>(), arb_file(), prop::option::of(arb_user()))
        .prop_map(|(uid, pid, file, user)| ProcessRef { uid, pid, file, user })
        .boxed()
}

pub fn arb_integrity() -> impl Strategy<Value = Integrity> {
    prop_oneof![
        Just(Integrity::Untrusted),
        Just(Integrity::Low),
        Just(Integrity::Medium),
        Just(Integrity::High),
        Just(Integrity::System),
        Just(Integrity::Protected),
    ]
}

pub fn arb_process() -> BoxedStrategy<Process> {
    (
        arb_process_ref(),
        arb_string(60),
        any::<bool>(),
        any::<i64>(),
        prop::option::of(arb_integrity()),
        prop::option::of(arb_process_ref()),
    )
        .prop_map(|(r, cmd_line, cmd_line_truncated, created_time, integrity, parent_process)| Process {
            uid: r.uid,
            pid: r.pid,
            file: r.file,
            user: r.user,
            cmd_line,
            cmd_line_truncated,
            created_time,
            integrity,
            parent_process,
        })
        .boxed()
}

pub fn arb_endpoint() -> BoxedStrategy<NetworkEndpoint> {
    let ip = prop_oneof![
        any::<[u8; 4]>().prop_map(|o| IpAddr::V4(Ipv4Addr::from(o))),
        any::<[u8; 16]>().prop_map(|o| IpAddr::V6(Ipv6Addr::from(o))),
    ];
    (ip, any::<u16>()).prop_map(|(ip, port)| NetworkEndpoint { ip, port }).boxed()
}

fn arb_reg_type() -> impl Strategy<Value = RegType> {
    prop_oneof![
        (0u32..=11).prop_map(|raw| RegType::Known(RegValueType::from_raw(raw).expect("0..=11 are valid"))),
        (12u32..).prop_map(RegType::Raw),
    ]
}

fn arb_class_counts() -> impl Strategy<Value = Vec<ClassCount>> {
    let entry = (any::<u32>(), any::<u64>()).prop_map(|(class_uid, count)| ClassCount { class_uid, count });
    prop::collection::vec(entry, 0..4)
}

/// A counter: absent, or any value.
fn counter() -> impl Strategy<Value = Option<u64>> {
    any::<Option<u64>>()
}

fn arb_health_report() -> BoxedStrategy<HealthReport> {
    let loss =
        (counter(), counter(), counter(), counter(), counter(), counter(), counter(), arb_class_counts(), counter())
            .prop_map(|f| SensorLoss {
                sensor_session_events_lost: f.0,
                process_session_events_lost: f.1,
                sensor_session_buffers_lost: f.2,
                process_session_buffers_lost: f.3,
                kernel_queue_drops: f.4,
                user_queue_drops: f.5,
                dns_rate_limit_drops: f.6,
                actor_dropped: f.7,
                buffer_backlog_drops: f.8,
            });
    let quality = (
        (counter(), counter(), counter(), counter(), arb_class_counts(), counter(), counter(), counter()),
        (counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
    )
        .prop_map(|(a, b)| SensorQuality {
            late_arrivals: a.0,
            parse_errors: a.1,
            unknown_version: a.2,
            launch_join_miss: a.3,
            actor_unresolved: a.4,
            unknown_file_object: a.5,
            registry_unresolved: a.6,
            value_read_failed: a.7,
            early_read_redone: b.0,
            reg_type_unusual: b.1,
            file_op_late_failure: b.2,
            writes_after_cleanup: b.3,
            file_object_replaced: b.4,
            buffer_invalid_records: b.5,
            enrichment_misses: b.6,
            enrichment_errors: b.7,
        });
    let housekeeping = (
        (counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
        (any::<Option<bool>>(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
    )
        .prop_map(|(a, b)| SensorHousekeeping {
            process_cache_evictions: a.0,
            file_map_evictions: a.1,
            key_map_evictions: a.2,
            early_key_map_evictions: a.3,
            flow_table_evictions: a.4,
            hash_cache_evictions: a.5,
            retention_evictions: a.6,
            file_op_failed: a.7,
            pending_overflow: a.8,
            seeding_enabled: b.0,
            seeder_handles_named: b.1,
            seeder_handles_failed: b.2,
            seeder_handles_timed_out: b.3,
            seeder_table_reads: b.4,
            seeder_deferred_rereads: b.5,
            seeder_stuck_helpers: b.6,
            seeder_negative_cache_size: b.7,
        });
    let resources =
        (counter(), counter()).prop_map(|(cpu_time, working_set)| SensorResources { cpu_time, working_set });
    // Gap times are both present (and ordered) or both absent.
    let times = prop::option::of((any::<i64>(), any::<i64>())).prop_map(|t| match t {
        Some((a, b)) => (Some(a.min(b)), Some(a.max(b))),
        None => (None, None),
    });
    let gap = prop::option::of((times, any::<u64>()).prop_map(|((first_time, last_time), events)| SensorGap {
        first_time,
        last_time,
        events,
    }));
    let buffer = (
        (counter(), counter(), any::<Option<bool>>(), counter(), counter()),
        (counter(), any::<Option<bool>>(), counter(), counter(), counter()),
    )
        .prop_map(|(a, b)| SensorBuffer {
            write_errors: a.0,
            recoveries: a.1,
            failing: a.2,
            rejected: a.3,
            delete_failures: a.4,
            truncated_bytes: b.0,
            cursor_reset: b.1,
            foreign_segments: b.2,
            corrupt_segments: b.3,
            disk_bytes: b.4,
        });
    (loss, quality, housekeeping, resources, gap, buffer)
        .prop_map(|(loss, quality, housekeeping, resources, gap, buffer)| HealthReport {
            loss,
            quality,
            housekeeping,
            resources,
            gap,
            buffer,
        })
        .boxed()
}

pub fn arb_kind() -> BoxedStrategy<EventKind> {
    let process = prop_oneof![
        (arb_process_ref(), arb_process()).prop_map(|(actor, process)| ProcessActivity::Launch { actor, process }),
        (arb_process_ref(), any::<Option<i32>>())
            .prop_map(|(process, exit_code)| ProcessActivity::Terminate { process, exit_code }),
    ]
    .prop_map(EventKind::Process);

    let module = (arb_process_ref(), arb_file(), any::<u64>()).prop_map(|(actor, file, base_address)| {
        EventKind::Module(ModuleActivity { actor, action: ModuleAction::Load { file, base_address } })
    });

    let net_action = prop_oneof![
        Just(NetworkAction::Open),
        (any::<Option<u64>>(), any::<Option<u64>>())
            .prop_map(|(bytes_in, bytes_out)| NetworkAction::Close { bytes_in, bytes_out }),
    ];
    let protocol = prop_oneof![Just(NetworkProtocol::Tcp), Just(NetworkProtocol::Udp)];
    let direction = prop_oneof![Just(NetworkDirection::Inbound), Just(NetworkDirection::Outbound)];
    let network = (arb_process_ref(), arb_endpoint(), arb_endpoint(), protocol, direction, net_action).prop_map(
        |(actor, src_endpoint, dst_endpoint, protocol, direction, action)| {
            EventKind::Network(NetworkActivity { actor, src_endpoint, dst_endpoint, protocol, direction, action })
        },
    );

    let file_action = prop_oneof![
        Just(FileAction::Create),
        Just(FileAction::Read),
        Just(FileAction::Update),
        Just(FileAction::Delete),
        arb_file().prop_map(|file_result| FileAction::Rename { file_result }),
        Just(FileAction::SetAttributes),
        Just(FileAction::Open),
    ];
    let file = (arb_process_ref(), arb_file(), file_action)
        .prop_map(|(actor, file, action)| EventKind::File(FileSystemActivity { actor, file, action }));

    let key_action = prop_oneof![
        Just(RegistryKeyAction::Create),
        Just(RegistryKeyAction::Delete),
        arb_string(40).prop_map(|prev_path| RegistryKeyAction::Rename { prev_path }),
    ];
    let reg_key = (arb_process_ref(), arb_string(40), any::<bool>(), key_action).prop_map(
        |(actor, path, path_unresolved, action)| {
            EventKind::RegistryKey(RegistryKeyActivity { actor, path, path_unresolved, action })
        },
    );

    // (data, data_truncated, data_unavailable): unavailable requires no data and no truncation.
    let value_data = prop_oneof![
        (prop::collection::vec(any::<u8>(), 0..64), any::<bool>())
            .prop_map(|(data, truncated)| (data, truncated, false)),
        Just((vec![], false, true)),
    ];
    let value_action = prop_oneof![
        (arb_reg_type(), value_data, any::<bool>()).prop_map(
            |(value_type, (data, data_truncated, data_unavailable), data_read_after)| RegistryValueAction::Set {
                value_type,
                data,
                data_truncated,
                data_read_after,
                data_unavailable,
            }
        ),
        Just(RegistryValueAction::Delete),
    ];
    let reg_value = (arb_process_ref(), arb_string(40), arb_string(20), any::<bool>(), value_action).prop_map(
        |(actor, key_path, name, path_unresolved, action)| {
            EventKind::RegistryValue(RegistryValueActivity { actor, key_path, name, path_unresolved, action })
        },
    );

    let answer = (any::<u16>(), arb_string(30)).prop_map(|(rr_type, data)| DnsAnswer { rr_type, data });
    let dns = (
        arb_process_ref(),
        arb_string(30),
        any::<u16>(),
        any::<Option<u16>>(),
        any::<Option<u32>>(),
        prop::collection::vec(answer, 0..5),
    )
        .prop_map(|(actor, hostname, query_type, rcode, platform_status, answers)| {
            EventKind::Dns(DnsActivity {
                actor,
                hostname,
                query_type,
                action: DnsAction::Response { rcode, platform_status, answers },
            })
        });

    // `log_name` is never empty, and neither is `log_provider` for Disable.
    let log_action =
        prop_oneof![Just(EventLogAction::Stop), Just(EventLogAction::Restart), Just(EventLogAction::Disable)];
    let event_log =
        (prop::option::of(arb_process_ref()), "[A-Za-z-]{1,20}", "[A-Za-z-]{1,20}", any::<Option<u32>>(), log_action)
            .prop_map(|(actor, log_name, log_provider, status_code, action)| {
                EventKind::EventLog(EventLogActivity { actor, log_name, log_provider, status_code, action })
            });

    let sensor_health = (any::<i64>(), arb_health_report()).prop_map(|(interval_start, report)| {
        EventKind::SensorHealth(SensorHealthActivity {
            interval_start,
            action: SensorHealthAction::Report(Box::new(report)),
        })
    });

    prop_oneof![
        process.boxed(),
        module.boxed(),
        network.boxed(),
        file.boxed(),
        reg_key.boxed(),
        reg_value.boxed(),
        dns.boxed(),
        event_log.boxed(),
        sensor_health.boxed(),
    ]
    .boxed()
}

pub fn arb_event() -> BoxedStrategy<Event> {
    let sensor = prop_oneof![Just(Sensor::Etw), Just(Sensor::Driver)];
    (arb_event_id(), any::<i64>(), sensor, arb_uid(DeviceUid::from_bytes), arb_uid(BootId::from_bytes), arb_kind())
        .prop_map(|(event_id, time, sensor, uid, boot_id, kind)| Event {
            meta: EventMeta { event_id, time, sensor },
            device: Device { uid, boot_id },
            kind,
        })
        .boxed()
}
```

- [ ] **Step 2: Expected OCSF ids**

`crates/atlas-schema/tests/ocsf_ids.rs`:
```diff
--- a/crates/atlas-schema/tests/ocsf_ids.rs
+++ b/crates/atlas-schema/tests/ocsf_ids.rs
@@ -15,12 +15,21 @@ const EXPECTED: &[(&str, u32, u32, u32)] = &[
     ("file_delete", 1, 1001, 4),
     ("file_rename", 1, 1001, 5),
     ("file_set_attributes", 1, 1001, 6),
+    ("file_open", 1, 1001, 14),
     ("registry_key_create", 1, 201001, 1),
     ("registry_key_delete", 1, 201001, 4),
     ("registry_key_rename", 1, 201001, 5),
+    ("registry_key_create_unresolved", 1, 201001, 1),
     ("registry_value_set", 1, 201002, 2),
+    ("registry_value_set_read_after", 1, 201002, 2),
+    ("registry_value_set_unavailable", 1, 201002, 2),
     ("registry_value_delete", 1, 201002, 4),
     ("dns_response", 4, 4003, 2),
+    ("event_log_stop", 1, 1008, 7),
+    ("event_log_restart", 1, 1008, 8),
+    ("event_log_disable", 1, 1008, 10),
+    // Atlas extension 500, category 6 (Application Activity), class 1 (sensor spec §10.3).
+    ("sensor_health_report", 6, 50_006_001, 1),
 ];
 
 #[test]
@@ -41,4 +50,13 @@ fn type_uid_examples() {
     let ids = |name: &str| common::samples().into_iter().find(|(n, _)| *n == name).unwrap().1.kind.ocsf_ids();
     assert_eq!(ids("process_launch").type_uid(), 100_701);
     assert_eq!(ids("registry_value_set").type_uid(), 20_100_202);
+    assert_eq!(ids("event_log_disable").type_uid(), 100_810);
+    // Above u32::MAX: `type_uid` is a u64.
+    assert_eq!(ids("sensor_health_report").type_uid(), 5_000_600_101);
+}
+
+#[test]
+fn sensor_health_class_uid_follows_the_ocsf_extension_rule() {
+    // extension_uid × 100000 + category_uid × 1000 + n
+    assert_eq!(atlas_schema::SENSOR_HEALTH_CLASS_UID, atlas_schema::ATLAS_EXTENSION_UID * 100_000 + 6 * 1000 + 1);
 }
```

- [ ] **Step 3: The validation table, and the moved version-skew probes**

`crates/atlas-schema/tests/validation.rs`:
```diff
--- a/crates/atlas-schema/tests/validation.rs
+++ b/crates/atlas-schema/tests/validation.rs
@@ -86,6 +86,21 @@ fn dns_response(w: &mut wire::Event) -> &mut wire::DnsResponse {
     }
 }
 
+fn event_log(w: &mut wire::Event) -> &mut wire::EventLogActivity {
+    match w.kind.as_mut() {
+        Some(Kind::EventLog(e)) => e,
+        _ => panic!("not event log"),
+    }
+}
+
+fn health_report(w: &mut wire::Event) -> &mut wire::SensorHealthReport {
+    use wire::sensor_health::Activity;
+    match w.kind.as_mut() {
+        Some(Kind::SensorHealth(wire::SensorHealth { activity: Some(Activity::Report(r)), .. })) => r,
+        _ => panic!("not a sensor health report"),
+    }
+}
+
 fn long(n: usize) -> String {
     "a".repeat(n)
 }
@@ -250,6 +265,64 @@ const CASES: &[(&str, Mutation, &str, K)] = &[
     ),
     ("registry_value_set", |w| reg_value_set(w).r#type = Some(12), "reg_value.type", K::UnknownEnum),
     ("registry_value_set", |w| reg_value_set(w).data = vec![0; REG_DATA_MAX + 1], "reg_value.data", K::TooLarge),
+    // registry value: sub-project 1 fields (sensor spec §10.4)
+    ("registry_value_set", |w| reg_value_set(w).raw_type = Some(12), "reg_value.raw_type", K::Malformed),
+    ("registry_value_set_unavailable", |w| reg_value_set(w).raw_type = Some(11), "reg_value.raw_type", K::Malformed),
+    ("registry_value_set", |w| reg_value_set(w).data_unavailable = true, "reg_value.data_unavailable", K::Malformed),
+    (
+        "registry_value_set_unavailable",
+        |w| reg_value_set(w).data_truncated = true,
+        "reg_value.data_unavailable",
+        K::Malformed,
+    ),
+    ("registry_value_set_unavailable", |w| reg_value_set(w).raw_type = None, "reg_value.type", K::Missing),
+    // event log
+    ("event_log_stop", |w| event_log(w).activity = None, "activity", K::Missing),
+    ("event_log_stop", |w| event_log(w).log_name.clear(), "log_name", K::Missing),
+    ("event_log_stop", |w| event_log(w).log_name = long(EVENT_LOG_NAME_MAX + 1), "log_name", K::TooLarge),
+    ("event_log_disable", |w| event_log(w).log_provider.clear(), "log_provider", K::Missing),
+    ("event_log_restart", |w| event_log(w).log_provider = long(EVENT_LOG_NAME_MAX + 1), "log_provider", K::TooLarge),
+    (
+        "event_log_stop",
+        |w| event_log(w).actor = Some(wire::ProcessRef { uid: vec![0; 3], ..Default::default() }),
+        "actor.process.uid",
+        K::Malformed,
+    ),
+    // sensor health
+    (
+        "sensor_health_report",
+        |w| match w.kind.as_mut() {
+            Some(Kind::SensorHealth(h)) => h.activity = None,
+            _ => unreachable!(),
+        },
+        "activity",
+        K::Missing,
+    ),
+    (
+        "sensor_health_report",
+        |w| {
+            let q = health_report(w).quality.as_mut().unwrap();
+            q.actor_unresolved = vec![wire::ClassCount { class_uid: 1001, count: 1 }; CLASS_COUNTS_MAX + 1];
+        },
+        "quality.actor_unresolved",
+        K::TooLarge,
+    ),
+    (
+        "sensor_health_report",
+        |w| {
+            let l = health_report(w).loss.as_mut().unwrap();
+            l.actor_dropped = vec![wire::ClassCount { class_uid: 4001, count: 1 }; CLASS_COUNTS_MAX + 1];
+        },
+        "loss.actor_dropped",
+        K::TooLarge,
+    ),
+    (
+        "sensor_health_report",
+        |w| health_report(w).gap.as_mut().unwrap().first_time = Some(i64::MAX),
+        "gap.last_time",
+        K::Malformed,
+    ),
+    ("sensor_health_report", |w| health_report(w).gap.as_mut().unwrap().last_time = None, "gap", K::Malformed),
     // dns
     ("dns_response", |w| dns_activity(w).hostname = long(DNS_HOSTNAME_MAX + 1), "query.hostname", K::TooLarge),
     ("dns_response", |w| dns_activity(w).query_type = 65_536, "query.type", K::Malformed),
@@ -298,6 +371,16 @@ const AT_LIMIT: &[(&str, Mutation)] = &[
         dns_response(w).answers = vec![a; DNS_ANSWERS_MAX];
     }),
     ("network_open", |w| network(w).src_endpoint.as_mut().unwrap().port = 65_535),
+    ("event_log_disable", |w| {
+        let e = event_log(w);
+        (e.log_name, e.log_provider) = (long(EVENT_LOG_NAME_MAX), long(EVENT_LOG_NAME_MAX));
+    }),
+    ("sensor_health_report", |w| {
+        let q = health_report(w).quality.as_mut().unwrap();
+        q.actor_unresolved = vec![wire::ClassCount { class_uid: 1001, count: 1 }; CLASS_COUNTS_MAX];
+    }),
+    ("registry_value_set_unavailable", |w| reg_value_set(w).raw_type = Some(12)),
+    ("registry_value_set_unavailable", |w| reg_value_set(w).raw_type = Some(u32::MAX)),
 ];
 
 #[test]
@@ -322,6 +405,14 @@ fn optional_fields_may_be_absent() {
     f.hashes = None;
     f.signature = None;
     check(w).expect("optional fields are optional");
+
+    // Event Log Activity: no actor and no status code (the samples already omit both).
+    check(wire_sample("event_log_stop")).expect("actor and status_code are optional");
+
+    // Sensor Health: every group and the gap may be absent.
+    let mut w = wire_sample("sensor_health_report");
+    *health_report(&mut w) = wire::SensorHealthReport::default();
+    check(w).expect("every Sensor Health field is optional");
 }
 
 #[test]
@@ -358,12 +449,13 @@ fn invalid_utf8_in_a_string_is_malformed() {
 
 #[test]
 fn class_from_a_newer_schema_is_rejected_as_missing_kind() {
-    // A newer agent might send oneof field 17 (a class this build does not know).
+    // A newer agent might send oneof field 19 (a class this build does not know;
+    // 17 and 18 are Event Log Activity and Sensor Health since sub-project 1).
     // prost keeps it as an unknown field, so `kind` is absent.
     let mut w = wire_sample("process_launch");
     w.kind = None;
     let mut bytes = w.encode_to_vec();
-    bytes.extend_from_slice(&[0x8a, 0x01, 0x00]); // field 17, wire type 2, length 0
+    bytes.extend(field(19, &[]));
     let err = decode_event(&bytes).unwrap_err();
     assert_eq!((err.field_path.as_str(), err.kind), ("kind", K::Missing));
 }
@@ -394,14 +486,16 @@ fn field(tag: u32, inner: &[u8]) -> Vec<u8> {
 fn unknown_activity_is_reported_before_class_fields() {
     // A newer activity may omit fields today's activities require (e.g. a future
     // Network Listen has no dst_endpoint). Version skew must read as `activity: Missing`,
-    // not as a missing class field. (sample, Event oneof tag, first unused activity tag)
+    // not as a missing class field. (sample, Event oneof tag, first unused field tag)
     let cases: &[(&str, u32, u32)] = &[
         ("module_load", 11, 3),
         ("network_open", 12, 8),
-        ("file_create", 13, 9),
-        ("registry_key_create", 14, 6),
-        ("registry_value_set", 15, 6),
+        ("file_create", 13, 10),
+        ("registry_key_create", 14, 7),
+        ("registry_value_set", 15, 7),
         ("dns_response", 16, 5),
+        ("event_log_stop", 17, 8),
+        ("sensor_health_report", 18, 3),
     ];
     for &(sample, event_tag, unknown_activity_tag) in cases {
         let mut w = wire_sample(sample);
@@ -430,6 +524,15 @@ fn unknown_activity_is_reported_before_class_fields() {
                 (c.activity, c.actor) = (None, None);
                 c.encode_to_vec()
             }
+            Kind::EventLog(mut c) => {
+                // `log_name` is required: the unknown activity must still be reported first.
+                (c.activity, c.log_name) = (None, String::new());
+                c.encode_to_vec()
+            }
+            Kind::SensorHealth(mut c) => {
+                c.activity = None;
+                c.encode_to_vec()
+            }
             Kind::Process(_) => unreachable!("process has no class-level fields"),
         };
         inner.extend(field(unknown_activity_tag, &[]));
```

- [ ] **Step 4: Run the tests; the golden test fails on the missing fixtures**

```powershell
cargo test -p atlas-schema
```
Expected: everything passes except `fixtures_match_samples`, which panics with `missing …\file_open.json; run with ATLAS_UPDATE_FIXTURES=1` (or the first missing name).

- [ ] **Step 5: Generate the fixtures and review them**

```powershell
$env:ATLAS_UPDATE_FIXTURES = '1'; cargo test -p atlas-schema --test golden; Remove-Item Env:ATLAS_UPDATE_FIXTURES
git diff --ignore-cr-at-eol --stat -- crates/atlas-schema/tests/fixtures
git status --short -- crates/atlas-schema/tests/fixtures
```
Expected: `git diff --ignore-cr-at-eol` prints nothing (no existing fixture changed), and `git status` lists 8 new files. Read each new file. For example, `registry_value_set_unavailable.json` must contain `"pathUnresolved": true`, `"dataUnavailable": true` and `"rawType": 2097152`, and no `type` or `data`. `sensor_health_report.json` must contain `"intervalStart"` and all five groups.

- [ ] **Step 6: Full verification, then commit**

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git add crates/atlas-proto crates/atlas-schema
git commit -m "feat(schema): sub-project 1 additions (File Open, registry flags, Event Log Activity, Sensor Health)"
```
Expected: all clean and green.

### Task 4: `atlas-buffer` crate and record framing

**Files:**
- Modify: `Cargo.toml` (workspace dependencies)
- Create: `crates/atlas-buffer/Cargo.toml`, `src/lib.rs` (minimal for now), `src/record.rs`

**Interfaces:**
- Produces: `record::{RECORD_HEADER (8), MAX_PAYLOAD (256 KiB), Frame::{Record { payload_start, next }, Incomplete, Invalid}, parse(&[u8], usize) -> Frame, Scan { valid_end, records }, scan(&[u8], usize) -> Scan}`, and the crate-private `encode(&[u8], &mut Vec<u8>)`.

- [ ] **Step 1: Workspace dependencies and crate manifest**

`Cargo.toml`:
```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -11,6 +11,7 @@ publish = false
 [workspace.dependencies]
 atlas-proto = { path = "crates/atlas-proto" }
 blake3 = "1.8"
+crc32c = "0.6"
 criterion = "0.8"
 prost = "0.14"
 prost-build = "0.14"
@@ -18,5 +19,6 @@ prost-reflect = { version = "0.16", features = ["serde"] }
 proptest = "1.11"
 protox = "0.9"
 serde_json = "1"
+tempfile = "3.27"
 thiserror = "2"
 uuid = { version = "1.26", features = ["v7"] }
```

`crates/atlas-buffer/Cargo.toml`:
```toml
[package]
name = "atlas-buffer"
version = "0.1.0"
description = "The agent's on-disk event buffer: an append-only segment log of opaque records."
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
crc32c.workspace = true

[dev-dependencies]
proptest.workspace = true
tempfile.workspace = true
```

Create `crates/atlas-buffer/src/lib.rs` (Task 6 replaces it):
```rust
//! The agent's on-disk event buffer (sensor spec §8).

pub mod record;
```

- [ ] **Step 2: Write the framing tests**

`crates/atlas-buffer/src/record.rs`, test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn framed(payloads: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in payloads {
            encode(p, &mut out);
        }
        out
    }

    #[test]
    fn encode_then_parse_round_trips() {
        let data = framed(&[b"abc", b"de"]);
        assert_eq!(parse(&data, 0), Frame::Record { payload_start: 8, next: 11 });
        assert_eq!(parse(&data, 11), Frame::Record { payload_start: 19, next: 21 });
        assert_eq!(parse(&data, 21), Frame::Incomplete);
        assert_eq!(scan(&data, 0), Scan { valid_end: 21, records: vec![(8, 11), (19, 21)] });
    }

    #[test]
    fn short_header_or_payload_is_incomplete() {
        let data = framed(&[b"abcdef"]);
        for cut in 0..data.len() {
            assert_eq!(parse(&data[..cut], 0), Frame::Incomplete, "cut at {cut}");
        }
    }

    #[test]
    fn bad_length_or_crc_is_invalid() {
        let mut zero_len = framed(&[b"x"]);
        zero_len[..4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse(&zero_len, 0), Frame::Invalid);

        // Rejected from the header alone: the 4 GiB "payload" is never looked for.
        let mut huge = framed(&[b"x"]);
        huge[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse(&huge, 0), Frame::Invalid);

        let mut over = vec![0u8; RECORD_HEADER];
        over[..4].copy_from_slice(&((MAX_PAYLOAD + 1) as u32).to_le_bytes());
        assert_eq!(parse(&over, 0), Frame::Invalid);

        let mut flipped = framed(&[b"abc"]);
        flipped[9] ^= 1;
        assert_eq!(parse(&flipped, 0), Frame::Invalid);
    }

    #[test]
    fn scan_stops_at_the_first_bad_frame() {
        let mut data = framed(&[b"one", b"two", b"three"]);
        data[20] ^= 0xff; // inside "two"'s payload (bytes 19..22)
        assert_eq!(scan(&data, 0), Scan { valid_end: 11, records: vec![(8, 11)] });
    }

    #[test]
    fn offsets_past_the_end_are_incomplete_not_a_panic() {
        assert_eq!(parse(&[], usize::MAX), Frame::Incomplete);
        assert_eq!(scan(&[1, 2, 3], 99), Scan { valid_end: 3, records: vec![] });
    }
}
```

- [ ] **Step 3: Run them; expect a compile failure**

```powershell
cargo test -p atlas-buffer
```
Expected: errors like `cannot find function 'encode' in this scope`.

- [ ] **Step 4: Implement the framing**

Add above the test module:

```rust
//! Record framing (sensor spec §8.1): `[u32 LE length][u32 LE CRC32C of payload][payload]`.
//!
//! Pure functions over byte slices, so recovery logic can be fuzzed without a file system.

/// Bytes before each payload: length + CRC.
pub const RECORD_HEADER: usize = 8;

/// Largest payload accepted: the 0a encoded-event limit (256 KiB). Checked
/// before anything is allocated or read.
pub const MAX_PAYLOAD: usize = 256 * 1024;

/// Appends one framed record to `out`. The caller has checked `1..=MAX_PAYLOAD`.
pub(crate) fn encode(payload: &[u8], out: &mut Vec<u8>) {
    debug_assert!(!payload.is_empty() && payload.len() <= MAX_PAYLOAD);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32c::crc32c(payload).to_le_bytes());
    out.extend_from_slice(payload);
}

/// What sits at an offset in a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// A whole, valid record: the payload is `data[payload_start..next]`.
    Record { payload_start: usize, next: usize },
    /// The record extends past the end of the data: not written yet, or torn.
    Incomplete,
    /// The length is 0 or above `MAX_PAYLOAD`, or the CRC does not match.
    Invalid,
}

/// Parses the frame starting at `at`. Never panics, never allocates.
pub fn parse(data: &[u8], at: usize) -> Frame {
    let Some(header) = data.get(at..).and_then(|rest| rest.get(..RECORD_HEADER)) else {
        return Frame::Incomplete;
    };
    let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let crc = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    if len == 0 || len > MAX_PAYLOAD {
        return Frame::Invalid;
    }
    let payload_start = at + RECORD_HEADER;
    let Some(payload) = data.get(payload_start..payload_start + len) else {
        return Frame::Incomplete;
    };
    if crc32c::crc32c(payload) != crc {
        return Frame::Invalid;
    }
    Frame::Record { payload_start, next: payload_start + len }
}

/// The result of scanning a segment's records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// End of the last valid record: everything after it is torn or corrupt.
    pub valid_end: usize,
    /// `(payload_start, payload_end)` of every valid record, in order.
    pub records: Vec<(usize, usize)>,
}

/// Scans records from `start` until the first frame that is not a whole valid
/// record (sensor spec §8.2).
pub fn scan(data: &[u8], start: usize) -> Scan {
    let mut at = start.min(data.len());
    let mut records = Vec::new();
    while let Frame::Record { payload_start, next } = parse(data, at) {
        records.push((payload_start, next));
        at = next;
    }
    Scan { valid_end: at, records }
}
```

- [ ] **Step 5: Run the tests and commit**

```powershell
cargo test -p atlas-buffer
git add Cargo.toml Cargo.lock crates/atlas-buffer
git commit -m "feat(buffer): crate skeleton and CRC32C record framing"
```
Expected: `5 passed`.

### Task 5: File-system helpers and the ack cursor

**Files:**
- Create: `crates/atlas-buffer/src/fsx.rs`, `src/cursor.rs`
- Modify: `crates/atlas-buffer/src/lib.rs`

**Interfaces:**
- Produces:
  - `fsx` (crate-private): `SEGMENT_MAGIC`, `SEGMENT_HEADER`, `segment_path`, `list_segments`, `open`, `open_read`, `open_rw`, `prepare_dir`;
  - `cursor`: `Cursor { segment, offset }` (public, ordered); crate-private `load(dir) -> Loaded::{Missing, Valid, Corrupt}` and `store(dir, cursor)`.

On Windows every open shares read, write and delete, and uses `FILE_FLAG_OPEN_REPARSE_POINT`, then checks the handle's attributes. The check and the use are therefore the same handle, with no time-of-check/time-of-use gap. The constants are the documented Win32 values, so no `windows` crate dependency and no `unsafe` are needed.

- [ ] **Step 1: Register the modules**

Append to `crates/atlas-buffer/src/lib.rs`:
```rust
mod cursor;
mod fsx;

pub use cursor::Cursor;
pub use fsx::{SEGMENT_HEADER, SEGMENT_MAGIC};
```

- [ ] **Step 2: Write `fsx.rs`**

The unit tests are at the bottom of the file.

`crates/atlas-buffer/src/fsx.rs`:
```rust
//! File-system helpers: segment naming, and opening files the way the agent's
//! data directory requires (sensor spec §8.5, §11.1).
//!
//! - Every handle shares read, write and delete, so a reader (`dump --follow`)
//!   never blocks the writer, and retention can delete a segment a reader holds.
//! - Nothing inside the buffer directory may be a reparse point (symlink or
//!   junction): on Windows files are opened with `FILE_FLAG_OPEN_REPARSE_POINT`
//!   and the handle's attributes are checked, so the check and the use are the
//!   same handle.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Every segment starts with this 8-byte header: a magic number and a format version.
pub const SEGMENT_MAGIC: [u8; 8] = *b"ATLSEG01";
pub const SEGMENT_HEADER: u64 = SEGMENT_MAGIC.len() as u64;

const SEGMENT_EXT: &str = "seg";

/// `00000000000000000042.seg`: zero-padded so names sort like numbers.
pub fn segment_path(dir: &Path, seq: u64) -> PathBuf {
    dir.join(format!("{seq:020}.{SEGMENT_EXT}"))
}

fn parse_segment_name(name: &str) -> Option<u64> {
    let digits = name.strip_suffix(".seg")?;
    if digits.len() != 20 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Sequence numbers of the segments in `dir`, ascending. Other names are ignored.
pub fn list_segments(dir: &Path) -> io::Result<Vec<u64>> {
    let mut seqs = Vec::new();
    for entry in fs::read_dir(dir)? {
        if let Some(seq) = entry?.file_name().to_str().and_then(parse_segment_name) {
            seqs.push(seq);
        }
    }
    seqs.sort_unstable();
    Ok(seqs)
}

fn invalid(what: &str, path: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{what}: {}", path.display()))
}

#[cfg(windows)]
mod imp {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

    use super::*;

    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const FILE_SHARE_DELETE: u32 = 0x4;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    pub fn open(path: &Path, opts: &mut OpenOptions) -> io::Result<File> {
        let file = opts
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let meta = file.metadata()?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !meta.is_file() {
            return Err(invalid("not a regular file", path));
        }
        Ok(file)
    }

    pub fn is_reparse_point(meta: &fs::Metadata) -> bool {
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    /// Non-Windows builds are for CI only; the check here is best effort.
    pub fn open(path: &Path, opts: &mut OpenOptions) -> io::Result<File> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(invalid("not a regular file", path)),
            _ => {}
        }
        let file = opts.open(path)?;
        if !file.metadata()?.is_file() {
            return Err(invalid("not a regular file", path));
        }
        Ok(file)
    }

    pub fn is_reparse_point(meta: &fs::Metadata) -> bool {
        meta.file_type().is_symlink()
    }
}

/// Opens a file inside the buffer directory (see the module docs).
pub fn open(path: &Path, opts: &mut OpenOptions) -> io::Result<File> {
    imp::open(path, opts)
}

pub fn open_read(path: &Path) -> io::Result<File> {
    open(path, OpenOptions::new().read(true))
}

/// Read-write, created if missing, never truncated.
pub fn open_rw(path: &Path) -> io::Result<File> {
    open(path, OpenOptions::new().read(true).write(true).create(true).truncate(false))
}

/// The size of a segment, without following a symlink or junction: one is refused.
pub fn segment_len(path: &Path) -> io::Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    if imp::is_reparse_point(&meta) || !meta.is_file() {
        return Err(invalid("not a regular file", path));
    }
    Ok(meta.len())
}

/// Creates the buffer directory if needed (its parent must exist) and checks
/// that it is a real directory, not a symlink or junction.
pub fn prepare_dir(dir: &Path) -> io::Result<()> {
    match fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = fs::symlink_metadata(dir)?;
    if imp::is_reparse_point(&meta) || !meta.is_dir() {
        return Err(invalid("buffer directory is not a plain directory", dir));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_names_round_trip_and_sort_numerically() {
        let dir = Path::new("buf");
        let name = segment_path(dir, 42).file_name().unwrap().to_str().unwrap().to_owned();
        assert_eq!(name, "00000000000000000042.seg");
        assert_eq!(parse_segment_name(&name), Some(42));
        assert!(segment_path(dir, 9).file_name() < segment_path(dir, 10).file_name());
    }

    #[test]
    fn other_names_are_not_segments() {
        for name in ["cursor", "cursor.tmp", "42.seg", "0000000000000000004x.seg", "00000000000000000042.seg.tmp"] {
            assert_eq!(parse_segment_name(name), None, "{name}");
        }
    }

    #[test]
    fn list_ignores_other_files() {
        let tmp = tempfile::tempdir().unwrap();
        for seq in [3, 1, 2] {
            File::create(segment_path(tmp.path(), seq)).unwrap();
        }
        File::create(tmp.path().join("cursor")).unwrap();
        assert_eq!(list_segments(tmp.path()).unwrap(), vec![1, 2, 3]);
    }
}
```

- [ ] **Step 3: Write `cursor.rs`**

The unit tests are at the bottom of the file.

`crates/atlas-buffer/src/cursor.rs`:
```rust
//! The ack cursor (sensor spec §8.5): a position in the log, persisted
//! atomically as `[u64 LE segment][u64 LE offset][u32 LE CRC32C of both]`.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

use crate::fsx;

const CURSOR_FILE: &str = "cursor";
const CURSOR_TMP: &str = "cursor.tmp";
const CURSOR_LEN: usize = 20;

/// A position in the log: the next record to read is at `offset` in segment `segment`.
/// `Cursor::default()` is before every record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cursor {
    pub segment: u64,
    pub offset: u64,
}

fn encode(c: Cursor) -> [u8; CURSOR_LEN] {
    let mut out = [0u8; CURSOR_LEN];
    out[..8].copy_from_slice(&c.segment.to_le_bytes());
    out[8..16].copy_from_slice(&c.offset.to_le_bytes());
    let crc = crc32c::crc32c(&out[..16]);
    out[16..].copy_from_slice(&crc.to_le_bytes());
    out
}

fn decode(bytes: &[u8]) -> Option<Cursor> {
    let bytes: &[u8; CURSOR_LEN] = bytes.try_into().ok()?;
    let crc = u32::from_le_bytes(bytes[16..].try_into().ok()?);
    if crc32c::crc32c(&bytes[..16]) != crc {
        return None;
    }
    Some(Cursor {
        segment: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        offset: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
    })
}

/// What was found in the cursor file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Loaded {
    Missing,
    Valid(Cursor),
    /// Wrong size or bad CRC: delivery restarts from the oldest record (at-least-once).
    Corrupt,
}

pub(crate) fn load(dir: &Path) -> io::Result<Loaded> {
    let path = dir.join(CURSOR_FILE);
    let file = match fsx::open_read(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Loaded::Missing),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::with_capacity(CURSOR_LEN);
    // One byte more than a valid file, so an oversized file is detected without reading it all.
    file.take(CURSOR_LEN as u64 + 1).read_to_end(&mut bytes)?;
    Ok(decode(&bytes).map_or(Loaded::Corrupt, Loaded::Valid))
}

/// Writes the cursor to a temporary file, flushes it, then renames it over the old one.
pub(crate) fn store(dir: &Path, cursor: Cursor) -> io::Result<()> {
    let tmp = dir.join(CURSOR_TMP);
    {
        let mut file = fsx::open(&tmp, OpenOptions::new().write(true).create(true).truncate(true))?;
        file.write_all(&encode(cursor))?;
        file.sync_all()?;
    }
    fs::rename(&tmp, dir.join(CURSOR_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_then_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(tmp.path()).unwrap(), Loaded::Missing);
        for c in [Cursor { segment: 1, offset: 8 }, Cursor { segment: u64::MAX, offset: 1 << 40 }] {
            store(tmp.path(), c).unwrap();
            assert_eq!(load(tmp.path()).unwrap(), Loaded::Valid(c));
        }
        assert!(!tmp.path().join(CURSOR_TMP).exists());
    }

    #[test]
    fn damaged_files_are_corrupt() {
        let tmp = tempfile::tempdir().unwrap();
        let good = encode(Cursor { segment: 3, offset: 99 });
        let mut flipped = good;
        flipped[0] ^= 1;
        let mut long = good.to_vec();
        long.push(0);
        for bytes in [&good[..19], &flipped[..], &long[..], &[][..]] {
            fs::write(tmp.path().join(CURSOR_FILE), bytes).unwrap();
            assert_eq!(load(tmp.path()).unwrap(), Loaded::Corrupt, "{} bytes", bytes.len());
        }
    }

    #[test]
    fn cursors_order_by_segment_then_offset() {
        assert!(Cursor { segment: 1, offset: 900 } < Cursor { segment: 2, offset: 8 });
        assert!(Cursor { segment: 2, offset: 8 } < Cursor { segment: 2, offset: 9 });
    }
}
```

- [ ] **Step 4: Run the tests and commit**

```powershell
cargo test -p atlas-buffer
git add crates/atlas-buffer
git commit -m "feat(buffer): segment files, reparse-point refusal, atomic ack cursor"
```
Expected: `11 passed`. `dead_code` warnings for the helpers Task 6 uses (`open_rw`, `prepare_dir`, …) are expected until then.

### Task 6: Writer and Reader

**Files:**
- Modify: `crates/atlas-buffer/src/lib.rs` (replace)
- Create: `crates/atlas-buffer/src/writer.rs`, `src/reader.rs`, `tests/common/mod.rs`, `tests/log.rs`

**Interfaces:**
- Consumes: Tasks 4–5.
- Produces: `Config::new(dir)` (16 MiB segments, 1 GiB cap, `Overflow::Retention`, 1 s flush, 10 s retry, 32 MiB backlog); `Overflow::{Retention, HeadTail, DropOldest, DropNewest}`; `Gap`; `Stats`; `RecordTime`; `Writer`, `Recovery`, `Dropped`; `Reader`, `Record`, `ReaderStats`. Signatures are under "Interfaces for later plans".

- [ ] **Step 1: Write the behaviour tests**

`crates/atlas-buffer/tests/common/mod.rs`:

`crates/atlas-buffer/tests/common/mod.rs`:
```rust
//! Shared helpers. Records carry their "event time" in the first 8 bytes.

#![allow(dead_code)] // each test binary uses a different subset

use std::path::Path;
use std::time::Instant;

use atlas_buffer::{Config, Cursor, Reader, Writer};

/// Framed size of every `rec`: 8-byte header + 32-byte payload.
pub const FRAMED: u64 = 40;

pub fn time_of(payload: &[u8]) -> Option<i64> {
    Some(i64::from_le_bytes(payload.get(..8)?.try_into().ok()?))
}

/// A 32-byte record whose time is `t`.
pub fn rec(t: i64) -> Vec<u8> {
    let mut p = t.to_le_bytes().to_vec();
    p.extend_from_slice(&[0x5a; 24]);
    p
}

/// Small segments so tests rotate quickly: 8-byte header + 6 records = 248 ≤ 256.
pub fn cfg(dir: &Path) -> Config {
    Config { segment_bytes: 256, cap_bytes: 1024, ..Config::new(dir) }
}

pub fn open(cfg: Config) -> Writer {
    Writer::open(cfg, time_of).expect("open").0
}

/// Appends and writes `times` in one tick.
pub fn write(w: &mut Writer, times: impl IntoIterator<Item = i64>) {
    for t in times {
        w.append(&rec(t)).expect("append");
    }
    flush(w);
}

/// Forces a flush now, whatever the interval.
pub fn flush(w: &mut Writer) {
    // Each call is a new `Instant` far enough apart for the 1 s interval.
    thread_local!(static CLOCK: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) });
    let now = CLOCK.with(|c| {
        let next = c.get().map_or_else(Instant::now, |t| t + std::time::Duration::from_secs(2));
        c.set(Some(next));
        next
    });
    w.tick(now).expect("tick");
}

/// The times of every record a fresh reader sees from `from`.
pub fn read_times(dir: &Path, from: Cursor) -> Vec<i64> {
    let mut reader = Reader::open(dir, from);
    std::iter::from_fn(|| reader.next_record().expect("read")).map(|r| time_of(&r.payload).unwrap()).collect()
}

/// The total size of the segment files actually on disk.
pub fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "seg"))
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum()
}

pub fn segment_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "seg"))
        .count()
}
```

`crates/atlas-buffer/tests/log.rs`:

`crates/atlas-buffer/tests/log.rs`:
```rust
//! Writing, reading, rotation, acks and the overflow policies (sensor spec §8.1, §8.4, §8.5).

mod common;

use std::time::{Duration, Instant};

use atlas_buffer::{Config, Cursor, Gap, Overflow, Reader, SEGMENT_HEADER, Writer};
use common::*;
use proptest::prelude::*;

#[test]
fn records_come_back_in_order_across_rotations() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20);
    assert_eq!(read_times(tmp.path(), Cursor::default()), (0..20).collect::<Vec<_>>());
    // 6 records per 256-byte segment.
    assert_eq!(segment_count(tmp.path()), 4);
    assert_eq!(w.stats().records_written, 20);
    assert_eq!(w.disk_bytes(), 4 * SEGMENT_HEADER + 20 * FRAMED);
}

#[test]
fn nothing_reaches_disk_before_the_first_tick() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    w.append(&rec(1)).unwrap();
    assert_eq!(read_times(tmp.path(), Cursor::default()), Vec::<i64>::new());
    flush(&mut w);
    assert_eq!(read_times(tmp.path(), Cursor::default()), vec![1]);
}

#[test]
fn writes_wait_for_the_flush_interval() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    let t0 = Instant::now();
    w.append(&rec(1)).unwrap();
    w.tick(t0).unwrap();
    w.append(&rec(2)).unwrap();
    w.tick(t0 + Duration::from_millis(999)).unwrap();
    assert_eq!(read_times(tmp.path(), Cursor::default()), vec![1]);
    w.tick(t0 + Duration::from_secs(1)).unwrap();
    assert_eq!(read_times(tmp.path(), Cursor::default()), vec![1, 2]);
}

#[test]
fn a_crowded_backlog_is_written_before_the_interval() {
    let tmp = tempfile::tempdir().unwrap();
    let backlog_bytes = 300 * 1024;
    let mut w = open(Config { backlog_bytes, segment_bytes: 1 << 20, cap_bytes: 4 << 20, ..Config::new(tmp.path()) });
    let t0 = Instant::now();
    w.tick(t0).unwrap();
    w.append(&vec![1; backlog_bytes / 2]).unwrap();
    w.tick(t0 + Duration::from_millis(1)).unwrap();
    assert_eq!(w.stats().records_written, 1);
}

#[test]
fn close_writes_everything() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    for t in 0..3 {
        w.append(&rec(t)).unwrap();
    }
    w.close().unwrap();
    assert_eq!(read_times(tmp.path(), Cursor::default()), vec![0, 1, 2]);
}

#[test]
fn reopening_starts_a_new_segment() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..2);
    drop(w);
    let (mut w, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
    assert_eq!(recovery, atlas_buffer::Recovery::default());
    write(&mut w, 2..4);
    assert_eq!(segment_count(tmp.path()), 2);
    assert_eq!(read_times(tmp.path(), Cursor::default()), vec![0, 1, 2, 3]);
}

#[test]
fn opening_and_idling_creates_no_segment() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    flush(&mut w);
    drop(w);
    let _w = open(cfg(tmp.path()));
    assert_eq!(segment_count(tmp.path()), 0);
}

#[test]
fn a_reader_resumes_from_a_record_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..10);
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    let mut cursor = Cursor::default();
    for _ in 0..7 {
        cursor = reader.next_record().unwrap().unwrap().next;
    }
    assert_eq!(cursor, reader.position());
    assert_eq!(read_times(tmp.path(), cursor), vec![7, 8, 9]);
}

#[test]
fn ack_persists_the_cursor_and_deletes_delivered_segments() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20); // segments 1..=4
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    let cursor = (0..13).map(|_| reader.next_record().unwrap().unwrap().next).last().unwrap();
    assert_eq!(cursor.segment, 3);

    w.ack(cursor).unwrap();
    assert_eq!(segment_count(tmp.path()), 2, "segments 1 and 2 were wholly delivered");
    assert_eq!(w.acked(), Some(cursor));

    // An older cursor is ignored; one past the newest segment is refused.
    w.ack(Cursor { segment: 1, offset: 8 }).unwrap();
    assert_eq!(w.acked(), Some(cursor));
    assert!(w.ack(Cursor { segment: 99, offset: 8 }).is_err());

    drop(w);
    let w = open(cfg(tmp.path()));
    assert_eq!(w.acked(), Some(cursor));
    assert_eq!(read_times(tmp.path(), cursor), (13..20).collect::<Vec<_>>());
}

#[test]
fn a_damaged_cursor_restarts_delivery_from_the_oldest_record() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..3);
    w.ack(Cursor { segment: 1, offset: SEGMENT_HEADER + FRAMED }).unwrap();
    drop(w);
    std::fs::write(tmp.path().join("cursor"), b"garbage").unwrap();
    let (w, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
    assert!(recovery.cursor_reset);
    assert_eq!(w.acked(), None);
}

#[test]
fn segments_left_behind_the_cursor_are_deleted_at_open() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20);
    w.ack(Cursor { segment: 3, offset: SEGMENT_HEADER }).unwrap();
    drop(w);
    // Simulate a crash between writing the cursor and deleting segment 2.
    std::fs::copy(tmp.path().join("00000000000000000003.seg"), tmp.path().join("00000000000000000002.seg")).unwrap();
    let _w = open(cfg(tmp.path()));
    assert!(!tmp.path().join("00000000000000000002.seg").exists());
}

#[test]
fn new_segments_never_reuse_a_delivered_number() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20); // newest is segment 4
    w.ack(Cursor { segment: 4, offset: SEGMENT_HEADER + 2 * FRAMED }).unwrap();
    drop(w);
    // Every segment deleted out of band: the next one must sort after the cursor.
    for e in std::fs::read_dir(tmp.path()).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "seg") {
            std::fs::remove_file(p).unwrap();
        }
    }
    let mut w = open(cfg(tmp.path()));
    write(&mut w, [100]);
    assert!(tmp.path().join("00000000000000000005.seg").exists());
    assert_eq!(read_times(tmp.path(), w.acked().unwrap()), vec![100]);
}

// ---------------------------------------------------------------- overflow

/// Writes `n` records one tick at a time; returns the policy's gaps.
fn fill(w: &mut Writer, times: std::ops::Range<i64>) -> Vec<Gap> {
    let mut gaps = Vec::new();
    for t in times {
        write(w, [t]);
        gaps.extend(w.take_gaps());
    }
    gaps
}

#[test]
fn retention_deletes_the_oldest_segment_without_a_gap() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    let gaps = fill(&mut w, 0..60);
    assert!(gaps.is_empty());
    assert!(w.disk_bytes() <= 1024);
    assert!(w.stats().retention_evictions > 0);
    // What is left is the newest, contiguous history.
    let times = read_times(tmp.path(), Cursor::default());
    assert_eq!(times, (60 - times.len() as i64..60).collect::<Vec<_>>());
}

#[test]
fn drop_oldest_reports_each_deleted_segment_as_a_gap() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(Config { overflow: Overflow::DropOldest, ..cfg(tmp.path()) });
    // The 1024-byte cap holds 4 segments of 6 records (248 bytes each): starting the
    // 5th (record 24) and the 6th (record 30) each deletes the oldest.
    let gaps = fill(&mut w, 0..36);
    assert_eq!(
        gaps,
        vec![
            Gap { records: 6, first_time: Some(0), last_time: Some(5) },
            Gap { records: 6, first_time: Some(6), last_time: Some(11) },
        ]
    );
    assert_eq!(w.stats().overflow_evictions, 2);
    assert_eq!(read_times(tmp.path(), Cursor::default()), (12..36).collect::<Vec<_>>());
}

#[test]
fn head_tail_pins_at_least_a_quarter_of_the_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    // 16 segments under the cap, so the head is several segments, not just the first.
    let mut w = open(Config { overflow: Overflow::HeadTail, cap_bytes: 16 * 256, ..cfg(tmp.path()) });
    let gaps = fill(&mut w, 0..300);
    let kept = read_times(tmp.path(), Cursor::default());
    // The head is the oldest whole segments holding ≥ 25% of the bytes on disk.
    let head = kept.iter().zip(0..).take_while(|(t, i)| **t == *i).count();
    let head_bytes = head as u64 * FRAMED + head.div_ceil(6) as u64 * SEGMENT_HEADER;
    assert!(head_bytes * 4 >= w.disk_bytes(), "head {head} records, {head_bytes} of {} bytes", w.disk_bytes());
    assert!(head >= 18, "more than one segment is pinned (head = {head})");
    assert_eq!(kept.last(), Some(&299));
    assert_eq!(kept.len() as u64 + gaps.iter().map(|g| g.records).sum::<u64>(), 300);
}

#[test]
fn head_tail_keeps_the_oldest_records_and_the_newest() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(Config { overflow: Overflow::HeadTail, ..cfg(tmp.path()) });
    let gaps = fill(&mut w, 0..60);
    let kept = read_times(tmp.path(), Cursor::default());
    // The head (the first segment: 0..6) is pinned through the whole flood...
    assert_eq!(&kept[..6], &[0, 1, 2, 3, 4, 5]);
    // ...the newest records are there...
    assert_eq!(kept.last(), Some(&59));
    // ...and the middle went, each deleted segment reported as a gap.
    let dropped: u64 = gaps.iter().map(|g| g.records).sum();
    assert_eq!(kept.len() as u64 + dropped, 60);
    assert_eq!(gaps[0], Gap { records: 6, first_time: Some(6), last_time: Some(11) });
}

#[test]
fn drop_newest_keeps_the_disk_and_reports_the_dropped_records() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(Config { overflow: Overflow::DropNewest, ..cfg(tmp.path()) });
    // 4 segments (24 records) fit; every later record is dropped, one gap per tick.
    let gaps = fill(&mut w, 0..30);
    assert_eq!(read_times(tmp.path(), Cursor::default()), (0..24).collect::<Vec<_>>());
    assert_eq!(gaps.len(), 6);
    assert_eq!(gaps[0], Gap { records: 1, first_time: Some(24), last_time: Some(24) });
    assert_eq!(w.stats().overflow_drops, 6);

    // Delivering (acking) frees space, and writing resumes.
    let end = {
        let mut r = Reader::open(tmp.path(), Cursor::default());
        std::iter::from_fn(|| r.next_record().unwrap()).last().unwrap().next
    };
    w.ack(end).unwrap();
    write(&mut w, [1000]);
    assert_eq!(read_times(tmp.path(), end), vec![1000]);
}

#[test]
fn config_is_validated() {
    let tmp = tempfile::tempdir().unwrap();
    for bad in [
        Config { cap_bytes: 1000, ..cfg(tmp.path()) },
        Config { segment_bytes: SEGMENT_HEADER, ..cfg(tmp.path()) },
        Config { backlog_bytes: 1024, ..cfg(tmp.path()) },
    ] {
        assert!(Writer::open(bad, time_of).is_err());
    }
}

fn policy() -> impl Strategy<Value = Overflow> {
    prop_oneof![
        Just(Overflow::Retention),
        Just(Overflow::HeadTail),
        Just(Overflow::DropOldest),
        Just(Overflow::DropNewest)
    ]
}

/// A record whose time is `t`, padded to `size` bytes (at least 8).
fn sized(t: i64, size: usize) -> Vec<u8> {
    let mut p = t.to_le_bytes().to_vec();
    p.resize(size.max(8), 0x33);
    p
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Under any policy and record sizes: the disk stays under the cap, what is kept is
    /// in order, every lost record is accounted for in a gap (except under retention,
    /// which reports none), and head-plus-tail never deletes the pinned first segment.
    #[test]
    fn overflow_invariants(policy in policy(), sizes in prop::collection::vec(8usize..200, 1..120)) {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = open(Config { overflow: policy, ..cfg(tmp.path()) });
        let mut gaps = Vec::new();
        let mut first_segment = Vec::new();
        for (t, &size) in sizes.iter().enumerate() {
            w.append(&sized(t as i64, size)).unwrap();
            flush(&mut w);
            gaps.extend(w.take_gaps());
            prop_assert!(w.disk_bytes() <= 1024);
            prop_assert_eq!(dir_bytes(tmp.path()), w.disk_bytes(), "the writer's accounting matches the disk");
            if first_segment.is_empty() && tmp.path().join("00000000000000000002.seg").exists() {
                let mut r = Reader::open(tmp.path(), Cursor::default());
                first_segment = std::iter::from_fn(|| r.next_record().unwrap())
                    .take_while(|rec| rec.at.segment == 1)
                    .map(|rec| time_of(&rec.payload).unwrap())
                    .collect();
            }
        }
        let kept = read_times(tmp.path(), Cursor::default());
        prop_assert!(kept.windows(2).all(|p| p[0] < p[1]));
        let lost: u64 = gaps.iter().map(|g| g.records).sum();
        if policy == Overflow::Retention {
            prop_assert!(gaps.is_empty());
        } else {
            prop_assert_eq!(kept.len() as u64 + lost, sizes.len() as u64);
        }
        if policy == Overflow::HeadTail {
            prop_assert!(first_segment.iter().all(|t| kept.contains(t)), "the pinned head was deleted");
        }
    }
}
```

- [ ] **Step 2: Run them; expect a compile failure**

```powershell
cargo test -p atlas-buffer --test log
```
Expected: `unresolved imports atlas_buffer::Config, …`.

- [ ] **Step 3: Replace `lib.rs`**

`crates/atlas-buffer/src/lib.rs`:
```rust
//! The agent's on-disk event buffer (sensor spec §8): an append-only log of
//! numbered segment files holding opaque, CRC-checked records.
//!
//! - [`Writer`]: the single writer. [`Writer::append`] only queues in memory;
//!   [`Writer::tick`] does all I/O: it writes and flushes once per
//!   `flush_interval`, and after an I/O error retries once per `retry_interval`
//!   while records wait in a bounded backlog (§8.3).
//! - [`Reader`]: iterates records after a [`Cursor`]; safe to run in another
//!   process while the writer appends (§8.5).
//! - Overflow at the size cap follows an [`Overflow`] policy (§8.4).
//!
//! The crate knows nothing about events or Windows: records are byte strings.
//! Time is passed in (`tick(now)`), so every behaviour is testable without sleeping.

mod cursor;
mod fsx;
mod reader;
pub mod record;
mod writer;

use std::io;
use std::path::PathBuf;
use std::time::Duration;

pub use cursor::Cursor;
pub use fsx::{SEGMENT_HEADER, SEGMENT_MAGIC};
pub use reader::{Reader, ReaderStats, Record};
pub use writer::{Dropped, Recovery, Writer};

/// Extracts an event time (ns since the Unix epoch) from a record, for gap reports.
/// The agent passes a function that decodes the event; the buffer stays opaque.
pub type RecordTime = fn(&[u8]) -> Option<i64>;

/// What happens when the buffer reaches `cap_bytes` (sensor spec §8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    /// No transport (sub-project 1): the buffer is local history. Delete the
    /// oldest segment; counted, but not a gap.
    Retention,
    /// Keep the oldest ~25% of undelivered data and the newest; delete the
    /// oldest segment after the pinned head, and report a gap.
    HeadTail,
    /// Delete the oldest segment and report a gap.
    DropOldest,
    /// Keep what is on disk; drop new records and report a gap.
    DropNewest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub dir: PathBuf,
    /// Size at which the active segment is sealed and a new one started. A record
    /// larger than this gets a segment of its own.
    pub segment_bytes: u64,
    /// Total size of all segments; at least 4 × `segment_bytes`. It can be
    /// exceeded by one oversized record, or while a segment cannot be deleted.
    pub cap_bytes: u64,
    pub overflow: Overflow,
    pub flush_interval: Duration,
    pub retry_interval: Duration,
    /// In-memory bytes waiting to be written. Records beyond it are dropped.
    pub backlog_bytes: usize,
}

impl Config {
    /// Spec defaults: 16 MiB segments, a 1 GiB cap, rolling retention, flush
    /// every 1 s, retry every 10 s, 32 MiB backlog.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            segment_bytes: 16 << 20,
            cap_bytes: 1 << 30,
            overflow: Overflow::Retention,
            flush_interval: Duration::from_secs(1),
            retry_interval: Duration::from_secs(10),
            backlog_bytes: 32 << 20,
        }
    }

    fn validate(&self) -> io::Result<()> {
        let bad = |msg: &str| Err(io::Error::new(io::ErrorKind::InvalidInput, msg.to_owned()));
        if self.segment_bytes <= SEGMENT_HEADER {
            return bad("segment_bytes must exceed the segment header");
        }
        if self.cap_bytes < self.segment_bytes.saturating_mul(4) {
            return bad("cap_bytes must be at least 4 × segment_bytes");
        }
        if self.backlog_bytes < record::RECORD_HEADER + record::MAX_PAYLOAD {
            return bad("backlog_bytes must hold at least one maximum-size record");
        }
        Ok(())
    }
}

/// Records deleted or dropped before delivery: the content of a Sensor Health gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    pub records: u64,
    /// Smallest and largest [`RecordTime`] among them; `None` if none decoded.
    pub first_time: Option<i64>,
    pub last_time: Option<i64>,
}

impl Gap {
    fn empty() -> Self {
        Self { records: 0, first_time: None, last_time: None }
    }

    fn add(&mut self, time: Option<i64>) {
        self.records += 1;
        if let Some(t) = time {
            self.first_time = Some(self.first_time.map_or(t, |f| f.min(t)));
            self.last_time = Some(self.last_time.map_or(t, |l| l.max(t)));
        }
    }
}

/// Cumulative writer counters; the agent reports deltas in Sensor Health.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub records_written: u64,
    pub bytes_written: u64,
    /// Segments deleted by [`Overflow::Retention`].
    pub retention_evictions: u64,
    /// Segments deleted by the other overflow policies (each also a [`Gap`]).
    pub overflow_evictions: u64,
    /// Records dropped because the policy deletes nothing ([`Overflow::DropNewest`]).
    pub overflow_drops: u64,
    /// Segments that could not be deleted (another process holds them open
    /// without delete sharing). They are kept and retried later.
    pub delete_failures: u64,
    /// Records dropped because the in-memory backlog was full.
    pub backlog_drops: u64,
    /// Records refused by `append`: empty or over `record::MAX_PAYLOAD`.
    pub rejected: u64,
    /// Ticks whose writes failed.
    pub write_errors: u64,
    /// Successful retries after a failure.
    pub recoveries: u64,
}
```

- [ ] **Step 4: Implement the writer**

`crates/atlas-buffer/src/writer.rs`, implementation:

```rust
//! The single writer (sensor spec §8.1–8.4).

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::cursor::{self, Cursor, Loaded};
use crate::fsx::{self, SEGMENT_HEADER, SEGMENT_MAGIC, segment_path};
use crate::record::{self, MAX_PAYLOAD, RECORD_HEADER};
use crate::{Config, Gap, Overflow, RecordTime, Stats};

/// Why `append` refused a record. Each is counted in [`Stats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    /// Empty, or larger than `record::MAX_PAYLOAD`.
    Rejected,
    /// The in-memory backlog is full (the disk is failing or far behind).
    BacklogFull,
}

/// What `Writer::open` found and repaired.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovery {
    /// Torn or corrupt bytes cut from the end of the newest segment.
    pub truncated_bytes: u64,
    /// The cursor file was damaged; delivery restarts from the oldest record.
    pub cursor_reset: bool,
    /// The newest segment did not start with the segment header; it is kept as
    /// a sealed segment, which readers skip.
    pub foreign_segment: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct Segment {
    seq: u64,
    len: u64,
}

enum Room {
    Made,
    Full,
}

pub struct Writer {
    cfg: Config,
    record_time: RecordTime,
    /// Every segment on disk, oldest first. The last one is active; its file is
    /// created when the first record is written to it.
    segments: VecDeque<Segment>,
    /// The active segment, positioned at its end. `None` until first use and after an I/O error.
    file: Option<File>,
    /// Bytes written to `file` since its last flush.
    unsynced: bool,
    /// Framed records not yet written.
    pending: Vec<u8>,
    acked: Option<Cursor>,
    last_flush: Option<Instant>,
    /// `Some` while failing: when to retry.
    retry_at: Option<Instant>,
    stats: Stats,
    gaps: Vec<Gap>,
    /// Records being dropped by `DropNewest`; closed by `take_gaps`.
    drop_gap: Option<Gap>,
    #[cfg(test)]
    faults: Faults,
}

impl Writer {
    /// Opens (or creates) the buffer (§8.2):
    /// - loads the cursor and deletes segments wholly behind it;
    /// - cuts the newest segment after its last valid record;
    /// - starts a new segment, so nothing written from now on lands in a segment
    ///   a reader may already have read past (an ack can be ahead of what a power cut kept).
    pub fn open(cfg: Config, record_time: RecordTime) -> io::Result<(Self, Recovery)> {
        cfg.validate()?;
        fsx::prepare_dir(&cfg.dir)?;
        let mut recovery = Recovery::default();
        let mut stats = Stats::default();
        let acked = match cursor::load(&cfg.dir)? {
            Loaded::Missing => None,
            Loaded::Valid(c) => Some(c),
            Loaded::Corrupt => {
                recovery.cursor_reset = true;
                None
            }
        };

        let seqs = fsx::list_segments(&cfg.dir)?;
        // The new segment sorts after every segment ever written and after the cursor.
        let next_seq = next(seqs.last().copied().unwrap_or(0).max(acked.map_or(0, |c| c.segment)))?;

        let mut segments = VecDeque::new();
        for (i, &seq) in seqs.iter().enumerate() {
            // Delivered; a crash between the cursor write and the delete can leave them.
            if acked.is_some_and(|c| seq < c.segment) && remove_segment(&cfg.dir, seq).is_ok() {
                continue;
            }
            let len = if i + 1 == seqs.len() {
                match recover_newest(&cfg.dir, seq, &mut recovery)? {
                    Some(len) => len,
                    None => {
                        recovery.foreign_segment = Some(seq);
                        fsx::segment_len(&segment_path(&cfg.dir, seq))?
                    }
                }
            } else {
                fsx::segment_len(&segment_path(&cfg.dir, seq))?
            };
            if acked.is_some_and(|c| seq < c.segment) {
                stats.delete_failures += 1; // kept and counted; deleted by a later ack or eviction
            }
            segments.push_back(Segment { seq, len });
        }
        segments.push_back(Segment { seq: next_seq, len: 0 });

        let writer = Self {
            cfg,
            record_time,
            segments,
            file: None,
            unsynced: false,
            pending: Vec::new(),
            acked,
            last_flush: None,
            retry_at: None,
            stats,
            gaps: Vec::new(),
            drop_gap: None,
            #[cfg(test)]
            faults: Faults::default(),
        };
        Ok((writer, recovery))
    }

    /// Queues one record in memory. Does no I/O. (Only the thread that owns the
    /// writer calls it, so it waits while that thread is inside `tick`.)
    pub fn append(&mut self, payload: &[u8]) -> Result<(), Dropped> {
        if payload.is_empty() || payload.len() > MAX_PAYLOAD {
            self.stats.rejected += 1;
            return Err(Dropped::Rejected);
        }
        if self.pending.len() + RECORD_HEADER + payload.len() > self.cfg.backlog_bytes {
            self.stats.backlog_drops += 1;
            return Err(Dropped::BacklogFull);
        }
        record::encode(payload, &mut self.pending);
        Ok(())
    }

    /// Does the I/O that is due: writes and flushes once `flush_interval` has
    /// passed since the last flush (or sooner if half the backlog is used); while
    /// failing, retries once `retry_interval` has passed since the failure.
    /// An `Err` means this attempt failed; records stay queued for the retry.
    pub fn tick(&mut self, now: Instant) -> io::Result<()> {
        if let Some(retry_at) = self.retry_at {
            if now < retry_at {
                return Ok(());
            }
        } else {
            let due = self.last_flush.is_none_or(|t| now.duration_since(t) >= self.cfg.flush_interval);
            let crowded = self.pending.len() >= self.cfg.backlog_bytes / 2;
            if !due && !crowded {
                return Ok(());
            }
        }
        self.last_flush = Some(now);
        match self.flush_now() {
            Ok(()) => {
                if self.retry_at.take().is_some() {
                    self.stats.recoveries += 1;
                }
                Ok(())
            }
            Err(e) => {
                // Reopened (and truncated to the last good length) on the retry.
                self.file = None;
                self.retry_at = Some(now + self.cfg.retry_interval);
                self.stats.write_errors += 1;
                Err(e)
            }
        }
    }

    /// Clean stop: writes and flushes everything queued.
    pub fn close(mut self) -> io::Result<()> {
        self.flush_now()
    }

    /// Persists `cursor` as delivered and deletes the segments wholly before it (§8.5).
    /// A cursor at or before the current one is ignored. A segment that cannot be
    /// deleted (another process has it open without delete sharing) is kept,
    /// counted in `Stats::delete_failures`, and retried by the next ack.
    pub fn ack(&mut self, cursor: Cursor) -> io::Result<()> {
        if self.acked.is_some_and(|a| cursor <= a) {
            return Ok(());
        }
        if cursor.segment > self.active().seq {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "cursor is past the newest segment"));
        }
        cursor::store(&self.cfg.dir, cursor)?;
        self.acked = Some(cursor);
        let active = self.active().seq;
        let dir = self.cfg.dir.clone();
        let mut failures = 0;
        self.segments.retain(|s| {
            if s.seq >= cursor.segment || s.seq == active {
                return true;
            }
            let kept = remove_segment(&dir, s.seq).is_err();
            failures += u64::from(kept);
            kept
        });
        self.stats.delete_failures += failures;
        Ok(())
    }

    /// The last acknowledged position: where a transport's reader starts.
    pub fn acked(&self) -> Option<Cursor> {
        self.acked
    }

    pub fn is_failing(&self) -> bool {
        self.retry_at.is_some()
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Gaps since the last call. A `DropNewest` gap still in progress is closed
    /// and returned, so a periodic report always includes current drops.
    pub fn take_gaps(&mut self) -> Vec<Gap> {
        if let Some(g) = self.drop_gap.take() {
            self.gaps.push(g);
        }
        std::mem::take(&mut self.gaps)
    }

    /// Bytes in all segments, including the active one.
    pub fn disk_bytes(&self) -> u64 {
        self.segments.iter().map(|s| s.len).sum()
    }

    fn active(&self) -> Segment {
        *self.segments.back().expect("there is always an active segment")
    }

    fn path(&self, seq: u64) -> PathBuf {
        segment_path(&self.cfg.dir, seq)
    }

    /// Writes what is queued and flushes what was written. Idle: no I/O at all.
    fn flush_now(&mut self) -> io::Result<()> {
        if self.file.is_none() && (!self.pending.is_empty() || self.unsynced) {
            self.open_active()?;
        }
        if !self.pending.is_empty() {
            self.write_pending()?;
        }
        if self.unsynced {
            self.file.as_ref().expect("opened above").sync_data()?;
            self.unsynced = false;
        }
        Ok(())
    }

    /// Opens the active segment at its last good length, creating it with its
    /// header if it is new. Also removes a torn write left by an I/O error.
    fn open_active(&mut self) -> io::Result<()> {
        let active = self.active();
        let mut file = fsx::open_rw(&self.path(active.seq))?;
        file.set_len(active.len)?;
        file.seek(SeekFrom::Start(active.len))?;
        if active.len == 0 {
            file.write_all(&SEGMENT_MAGIC)?;
            self.segments.back_mut().expect("active").len = SEGMENT_HEADER;
            self.unsynced = true;
        }
        self.file = Some(file);
        Ok(())
    }

    fn write_pending(&mut self) -> io::Result<()> {
        let mut done = 0;
        let result = self.write_from(&mut done);
        self.pending.drain(..done);
        result
    }

    /// Writes `pending[*done..]`, rotating as segments fill. `*done` always marks
    /// the bytes already written (or dropped), even when an error is returned.
    fn write_from(&mut self, done: &mut usize) -> io::Result<()> {
        while *done < self.pending.len() {
            let active_len = self.active().len;
            // Whole records that fit; an empty segment always takes at least one.
            let (mut end, mut count) = (*done, 0u64);
            while end < self.pending.len() {
                let next = end + record_len(&self.pending, end);
                let fits = active_len + (next - *done) as u64 <= self.cfg.segment_bytes;
                if !fits && !(end == *done && active_len == SEGMENT_HEADER) {
                    break;
                }
                (end, count) = (next, count + 1);
            }
            if end == *done {
                match self.rotate()? {
                    Room::Made => continue,
                    Room::Full => {
                        self.drop_pending(*done);
                        *done = self.pending.len();
                        return Ok(());
                    }
                }
            }
            self.fault()?;
            let file = self.file.as_mut().expect("opened by flush_now or rotate");
            file.write_all(&self.pending[*done..end])?;
            self.unsynced = true;
            self.segments.back_mut().expect("active").len += (end - *done) as u64;
            self.stats.records_written += count;
            self.stats.bytes_written += (end - *done) as u64;
            *done = end;
        }
        Ok(())
    }

    /// Seals the active segment and starts the next one, after making room for it.
    fn rotate(&mut self) -> io::Result<Room> {
        if let Room::Full = self.make_room() {
            return Ok(Room::Full);
        }
        if let Some(file) = self.file.take() {
            file.sync_data()?;
            self.unsynced = false;
        }
        let seq = next(self.active().seq)?;
        self.segments.push_back(Segment { seq, len: 0 });
        self.open_active()?;
        Ok(Room::Made)
    }

    /// Deletes segments until a new full segment fits under the cap (§8.4). A
    /// segment that cannot be deleted is skipped and counted; if none can be,
    /// the new segment goes over the cap rather than stalling every write.
    fn make_room(&mut self) -> Room {
        let mut undeletable = Vec::new();
        while self.disk_bytes() + self.cfg.segment_bytes > self.cfg.cap_bytes {
            let Some(i) = self.victim(&undeletable) else {
                // Nothing the policy may delete (DropNewest): drop. Only undeletable ones left: go over.
                return if undeletable.is_empty() { Room::Full } else { Room::Made };
            };
            let seq = self.segments[i].seq;
            // Read before deleting, count only once deleted.
            let gap = (self.cfg.overflow != Overflow::Retention).then(|| self.segment_gap(seq));
            if remove_segment(&self.cfg.dir, seq).is_err() {
                self.stats.delete_failures += 1;
                undeletable.push(seq);
                continue;
            }
            self.segments.remove(i);
            match gap {
                None => self.stats.retention_evictions += 1,
                Some(gap) => {
                    self.stats.overflow_evictions += 1;
                    if gap.records > 0 {
                        self.gaps.push(gap);
                    }
                }
            }
        }
        Room::Made
    }

    /// The index of the next segment the policy deletes, never the active one
    /// and never one in `skip`.
    fn victim(&self, skip: &[u64]) -> Option<usize> {
        let sealed = self.segments.len() - 1;
        let first = match self.cfg.overflow {
            Overflow::Retention | Overflow::DropOldest => 0,
            Overflow::DropNewest => return None,
            Overflow::HeadTail => {
                // Pin the oldest segments holding ~25% of the undelivered bytes.
                let target = self.disk_bytes() / 4;
                let (mut pinned, mut bytes) = (0, 0);
                while pinned < sealed && bytes < target {
                    bytes += self.segments[pinned].len;
                    pinned += 1;
                }
                pinned
            }
        };
        (first..sealed).find(|&i| !skip.contains(&self.segments[i].seq))
    }

    /// Counts the records in a segment about to be deleted, and their time range.
    /// An unreadable segment yields an empty gap rather than blocking eviction.
    fn segment_gap(&self, seq: u64) -> Gap {
        let mut gap = Gap::empty();
        let mut data = Vec::new();
        let read = fsx::open_read(&self.path(seq)).and_then(|mut f| f.read_to_end(&mut data));
        if read.is_ok() && data.starts_with(&SEGMENT_MAGIC) {
            for (start, end) in record::scan(&data, SEGMENT_HEADER as usize).records {
                gap.add((self.record_time)(&data[start..end]));
            }
        }
        gap
    }

    /// Drops `pending[from..]` for lack of room (`DropNewest`), adding it to the open gap.
    fn drop_pending(&mut self, from: usize) {
        let gap = self.drop_gap.get_or_insert_with(Gap::empty);
        let mut at = from;
        while at < self.pending.len() {
            let next = at + record_len(&self.pending, at);
            gap.add((self.record_time)(&self.pending[at + RECORD_HEADER..next]));
            self.stats.overflow_drops += 1;
            at = next;
        }
    }

    #[cfg(not(test))]
    fn fault(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// Test hook: after `skip` good writes, fails the next `fail` writes, leaving
    /// torn bytes behind as a real failure might.
    #[cfg(test)]
    fn fault(&mut self) -> io::Result<()> {
        if self.faults.skip > 0 {
            self.faults.skip -= 1;
            return Ok(());
        }
        if self.faults.fail == 0 {
            return Ok(());
        }
        self.faults.fail -= 1;
        if let Some(file) = self.file.as_mut() {
            file.write_all(b"torn")?;
        }
        Err(io::Error::other("injected fault"))
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
struct Faults {
    skip: u32,
    fail: u32,
}

/// The sequence number after `seq`. A hostile file name or cursor near `u64::MAX`
/// is refused rather than wrapping to a number that sorts first.
fn next(seq: u64) -> io::Result<u64> {
    seq.checked_add(1).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "segment numbers exhausted"))
}

/// Length of the framed record at `at` in `pending` (built by `record::encode`).
fn record_len(pending: &[u8], at: usize) -> usize {
    let len = u32::from_le_bytes(pending[at..at + 4].try_into().expect("4 bytes"));
    RECORD_HEADER + len as usize
}

fn remove_segment(dir: &Path, seq: u64) -> io::Result<()> {
    match fs::remove_file(segment_path(dir, seq)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// A header torn while the segment was being created: a prefix of the magic,
/// followed by nothing or by zeros (NTFS can keep a file's size after a power
/// cut while the unwritten bytes read as zeros).
fn torn_header(data: &[u8]) -> bool {
    let head = &data[..data.len().min(SEGMENT_MAGIC.len())];
    let written = head.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    SEGMENT_MAGIC.starts_with(&head[..written]) && data[head.len()..].iter().all(|&b| b == 0)
}

/// Scans the newest segment and cuts it after its last valid record; a torn
/// header becomes an empty segment. `None` if it is not an Atlas segment (it is
/// then left untouched).
fn recover_newest(dir: &Path, seq: u64, recovery: &mut Recovery) -> io::Result<Option<u64>> {
    let mut file = fsx::open_rw(&segment_path(dir, seq))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    let header = SEGMENT_MAGIC.len();
    if data.len() >= header && data[..header] == SEGMENT_MAGIC {
        let valid_end = record::scan(&data, header).valid_end;
        if valid_end < data.len() {
            file.set_len(valid_end as u64)?;
            file.sync_all()?;
            recovery.truncated_bytes += (data.len() - valid_end) as u64;
        }
        return Ok(Some(valid_end as u64));
    }
    if !torn_header(&data) {
        return Ok(None);
    }
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?; // `read_to_end` left the position at the old end
    file.write_all(&SEGMENT_MAGIC)?;
    file.sync_all()?;
    recovery.truncated_bytes += data.len() as u64;
    Ok(Some(SEGMENT_HEADER))
}
```

Its unit tests use the test-only fault hook (`Faults { skip, fail }`). After `skip` good writes it fails the next `fail` writes, each leaving torn bytes behind as a real failure might. Put them at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::Reader;

    fn time_of(payload: &[u8]) -> Option<i64> {
        Some(i64::from_le_bytes(payload.get(..8)?.try_into().ok()?))
    }

    fn rec(t: i64) -> Vec<u8> {
        let mut p = t.to_le_bytes().to_vec();
        p.extend_from_slice(&[0xaa; 24]);
        p
    }

    fn cfg(dir: &Path) -> Config {
        Config { segment_bytes: 256, cap_bytes: 1024, ..Config::new(dir) }
    }

    fn read_all(dir: &Path) -> Vec<Vec<u8>> {
        let mut reader = Reader::open(dir, Cursor::default());
        std::iter::from_fn(|| reader.next_record().unwrap()).map(|r| r.payload).collect()
    }

    fn segment_files(dir: &Path) -> usize {
        fsx::list_segments(dir).unwrap().len()
    }

    #[test]
    fn backlog_survives_a_failure_and_is_written_on_retry() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = Writer::open(cfg(tmp.path()), time_of).unwrap();
        let t0 = Instant::now();
        w.append(&rec(1)).unwrap();
        w.tick(t0).unwrap();

        w.faults.fail = 1;
        w.append(&rec(2)).unwrap();
        assert!(w.tick(t0 + Duration::from_secs(1)).is_err());
        assert!(w.is_failing());
        w.append(&rec(3)).unwrap();

        // Before the retry interval nothing is attempted, even though a flush is due.
        w.tick(t0 + Duration::from_secs(5)).unwrap();
        assert!(w.is_failing());
        assert_eq!(read_all(tmp.path()), vec![rec(1)], "the torn bytes are not a record");

        w.tick(t0 + Duration::from_secs(11)).unwrap();
        assert!(!w.is_failing());
        assert_eq!((w.stats().write_errors, w.stats().recoveries), (1, 1));
        assert_eq!(read_all(tmp.path()), vec![rec(1), rec(2), rec(3)]);
    }

    #[test]
    fn a_failure_right_after_a_rotation_is_retried_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = Writer::open(cfg(tmp.path()), time_of).unwrap();
        let t0 = Instant::now();
        let records: Vec<_> = (0..12).map(rec).collect(); // 40 B framed: 6 per segment
        for r in &records {
            w.append(r).unwrap();
        }
        // The first batch (segment 1) is written, then the write into the new segment 2
        // fails, twice, each time leaving torn bytes in segment 2.
        w.faults = Faults { skip: 1, fail: 2 };
        assert!(w.tick(t0).is_err());
        assert_eq!(segment_files(tmp.path()), 2, "the failure happened after the rotation");
        assert!(w.tick(t0 + Duration::from_secs(10)).is_err());
        w.tick(t0 + Duration::from_secs(20)).unwrap();
        assert_eq!(segment_files(tmp.path()), 2);
        assert_eq!(read_all(tmp.path()), records);
    }

    #[test]
    fn a_full_backlog_drops_and_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let backlog_bytes = RECORD_HEADER + MAX_PAYLOAD;
        let (mut w, _) = Writer::open(Config { backlog_bytes, ..cfg(tmp.path()) }, time_of).unwrap();
        w.append(&vec![1; MAX_PAYLOAD - 40]).unwrap();
        assert_eq!(w.append(&[1; 64]), Err(Dropped::BacklogFull));
        assert_eq!(w.stats().backlog_drops, 1);
    }

    #[test]
    fn empty_and_oversized_records_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = Writer::open(cfg(tmp.path()), time_of).unwrap();
        assert_eq!(w.append(&[]), Err(Dropped::Rejected));
        assert_eq!(w.append(&vec![0; MAX_PAYLOAD + 1]), Err(Dropped::Rejected));
        assert_eq!(w.stats().rejected, 2);
        w.append(&vec![0; MAX_PAYLOAD]).unwrap();
    }

    #[test]
    fn idle_ticks_touch_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = Writer::open(cfg(tmp.path()), time_of).unwrap();
        let t0 = Instant::now();
        for s in 0..3 {
            w.tick(t0 + Duration::from_secs(s)).unwrap();
        }
        assert_eq!(segment_files(tmp.path()), 0, "no empty segment is created");
        w.append(&rec(1)).unwrap();
        w.tick(t0 + Duration::from_secs(5)).unwrap();
        assert!(!w.unsynced);
        w.tick(t0 + Duration::from_secs(6)).unwrap();
        assert!(!w.unsynced);
    }

    #[test]
    fn torn_headers_are_recognised() {
        for torn in [&b""[..], b"ATL", b"ATLSEG0", &[0; 8], &[0; 64], b"ATL\0\0\0\0\0\0\0"] {
            assert!(torn_header(torn), "{torn:?}");
        }
        for foreign in [&b"ATX"[..], b"not an atlas segment", b"ATL\0\0\0\0\0x", b"\0\0\0x"] {
            assert!(!torn_header(foreign), "{foreign:?}");
        }
    }

    #[test]
    fn sequence_numbers_never_wrap() {
        assert_eq!(next(7).unwrap(), 8);
        assert_eq!(next(u64::MAX).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
```

- [ ] **Step 5: Implement the reader**

`crates/atlas-buffer/src/reader.rs`:
```rust
//! Readers (sensor spec §8.5). Safe beside a live writer, in this process or another:
//! - a record that is not completely there yet in the newest segment means
//!   "not written yet": `next_record` returns `None` and the caller polls again;
//! - a bad record in a sealed segment (one with a newer segment after it) is
//!   corruption: the rest of that segment is skipped and counted;
//! - a segment deleted under the reader (retention, overflow, ack) is skipped.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::cursor::Cursor;
use crate::fsx::{self, SEGMENT_HEADER, SEGMENT_MAGIC, segment_path};
use crate::record::{self, Frame, RECORD_HEADER};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Where this record starts.
    pub at: Cursor,
    /// Where the next record starts: pass it to `Writer::ack` once this record is delivered.
    pub next: Cursor,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReaderStats {
    /// Sealed segments whose remainder was skipped as corrupt (bad record or bad header).
    pub corrupt_segments: u64,
}

pub struct Reader {
    dir: PathBuf,
    pos: Cursor,
    file: Option<File>,
    stats: ReaderStats,
}

enum At {
    Record(Record),
    /// Nothing more at this position yet.
    End,
    /// A partial or invalid record at this position.
    Bad,
}

impl Reader {
    /// A reader positioned at `from` (`Cursor::default()` for the oldest record).
    /// No I/O happens until `next_record`.
    pub fn open(dir: &Path, from: Cursor) -> Self {
        Self { dir: dir.to_owned(), pos: from, file: None, stats: ReaderStats::default() }
    }

    /// The position of the next record to read.
    pub fn position(&self) -> Cursor {
        self.pos
    }

    pub fn stats(&self) -> ReaderStats {
        self.stats
    }

    /// The next record, or `None` if the reader has caught up with the writer.
    pub fn next_record(&mut self) -> io::Result<Option<Record>> {
        loop {
            if self.file.is_none() && !self.enter_segment()? {
                return Ok(None);
            }
            match self.read_at()? {
                At::Record(r) => return Ok(Some(r)),
                At::End | At::Bad if !self.newer_segment_exists()? => return Ok(None),
                At::End | At::Bad => {
                    // Sealed: its contents are final now, so read once more before moving on.
                    match self.read_at()? {
                        At::Record(r) => return Ok(Some(r)),
                        At::End => {}
                        At::Bad => self.stats.corrupt_segments += 1,
                    }
                    self.next_segment();
                }
            }
        }
    }

    /// Only called on a sealed segment, so a larger number exists and this cannot overflow.
    fn next_segment(&mut self) {
        self.file = None;
        self.pos = Cursor { segment: self.pos.segment.saturating_add(1), offset: 0 };
    }

    fn newer_segment_exists(&self) -> io::Result<bool> {
        Ok(fsx::list_segments(&self.dir)?.last().is_some_and(|&s| s > self.pos.segment))
    }

    /// Opens the first existing segment at or after the position. `false`: none
    /// is ready yet.
    fn enter_segment(&mut self) -> io::Result<bool> {
        loop {
            let seqs = fsx::list_segments(&self.dir)?;
            let Some(&seq) = seqs.iter().find(|&&s| s >= self.pos.segment) else { return Ok(false) };
            if seq > self.pos.segment {
                self.pos = Cursor { segment: seq, offset: 0 };
            }
            let sealed = seqs.last().is_some_and(|&last| last > seq);
            let mut file = match fsx::open_read(&segment_path(&self.dir, seq)) {
                Ok(f) => f,
                // A sealed segment deleted (or delete-pending) since the listing: skip it.
                // The newest segment is never deleted, so there the error is real.
                Err(e) if sealed && matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied) => {
                    self.next_segment();
                    continue;
                }
                Err(e) => return Err(e),
            };
            let mut header = [0u8; SEGMENT_MAGIC.len()];
            let n = read_up_to(&mut file, &mut header)?;
            if n == header.len() && header == SEGMENT_MAGIC {
                self.pos.offset = self.pos.offset.max(SEGMENT_HEADER);
                self.file = Some(file);
                return Ok(true);
            }
            if !sealed {
                return Ok(false); // being created
            }
            self.stats.corrupt_segments += 1;
            self.next_segment();
        }
    }

    fn read_at(&mut self) -> io::Result<At> {
        let file = self.file.as_mut().expect("entered");
        file.seek(SeekFrom::Start(self.pos.offset))?;
        let mut buf = vec![0u8; RECORD_HEADER];
        match read_up_to(file, &mut buf)? {
            0 => return Ok(At::End),
            n if n < RECORD_HEADER => return Ok(At::Bad),
            _ => {}
        }
        // The header alone tells a bad length (Invalid) from a missing payload (Incomplete),
        // so nothing is allocated for an invalid length.
        if record::parse(&buf, 0) == Frame::Invalid {
            return Ok(At::Bad);
        }
        let len = u32::from_le_bytes(buf[..4].try_into().expect("4 bytes")) as usize;
        buf.resize(RECORD_HEADER + len, 0);
        if read_up_to(file, &mut buf[RECORD_HEADER..])? < len {
            return Ok(At::Bad);
        }
        let Frame::Record { .. } = record::parse(&buf, 0) else { return Ok(At::Bad) };
        let at = self.pos;
        self.pos.offset += buf.len() as u64;
        buf.drain(..RECORD_HEADER);
        Ok(At::Record(Record { at, next: self.pos, payload: buf }))
    }
}

/// Reads until `buf` is full or the file ends; returns the bytes read.
fn read_up_to(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match file.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}
```

- [ ] **Step 6: Run the tests, lint, and commit**

```powershell
cargo test -p atlas-buffer
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
git add crates/atlas-buffer
git commit -m "feat(buffer): writer (batching, backlog, retry, overflow policies, acks) and reader"
```
Expected: `18 passed` (unit) and `19 passed` (`log`, including `overflow_invariants`, about 12 s). Clippy is clean.

### Task 7: Recovery and concurrency tests

**Files:**
- Create: `crates/atlas-buffer/tests/recovery.rs`, `tests/concurrent.rs`

**Interfaces:**
- Consumes: Task 6. These tests check §8.2 and §8.5 against the implementation. A failure here means a defect in Task 6's code, not in the test.

- [ ] **Step 1: Recovery tests**

`crates/atlas-buffer/tests/recovery.rs`:
```rust
//! Crash recovery (sensor spec §8.2): a segment torn at any byte, or corrupted,
//! recovers to exactly its valid prefix, and writing continues after it.

mod common;

use atlas_buffer::record::{self, MAX_PAYLOAD, RECORD_HEADER};
use atlas_buffer::{Cursor, Reader, SEGMENT_HEADER, SEGMENT_MAGIC, Writer};
use common::*;
use proptest::prelude::*;

const FIRST: &str = "00000000000000000001.seg";

/// A segment holding records of different sizes, and each record's end offset.
fn sample_segment() -> (Vec<u8>, Vec<usize>) {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    let payloads: [&[u8]; 4] = [&rec(0), b"x", &[7; 100], &rec(3)];
    for p in payloads {
        w.append(p).unwrap();
    }
    w.close().unwrap();
    let data = std::fs::read(tmp.path().join(FIRST)).unwrap();
    let mut ends = Vec::new();
    let mut at = SEGMENT_HEADER as usize;
    for p in payloads {
        at += RECORD_HEADER + p.len();
        ends.push(at);
    }
    assert_eq!(*ends.last().unwrap(), data.len());
    (data, ends)
}

#[test]
fn a_segment_torn_at_every_byte_recovers_its_whole_records() {
    let (data, ends) = sample_segment();
    for cut in 0..=data.len() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(FIRST), &data[..cut]).unwrap();

        let (mut w, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
        let whole = ends.iter().filter(|&&e| e <= cut).count();
        let valid_end = if cut < SEGMENT_HEADER as usize { 0 } else { ends[..whole].last().map_or(8, |&e| e) };
        assert_eq!(recovery.truncated_bytes, (cut - valid_end) as u64, "cut at {cut}");
        assert_eq!(recovery.foreign_segment, None, "cut at {cut}");

        // Writing continues right after the last whole record.
        write(&mut w, [99]);
        let mut reader = Reader::open(tmp.path(), Cursor::default());
        let got: Vec<_> = std::iter::from_fn(|| reader.next_record().unwrap()).map(|r| r.payload).collect();
        assert_eq!(got.len(), whole + 1, "cut at {cut}");
        assert_eq!(got.last().unwrap(), &rec(99), "cut at {cut}");
        assert_eq!(reader.stats().corrupt_segments, 0);
    }
}

#[test]
fn a_corrupt_record_in_the_newest_segment_is_cut_with_everything_after_it() {
    let (mut data, ends) = sample_segment();
    data[ends[1] + RECORD_HEADER + 50] ^= 0x01; // inside the third record's payload
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(FIRST), &data).unwrap();
    let (_, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
    assert_eq!(recovery.truncated_bytes, (data.len() - ends[1]) as u64);
    assert_eq!(std::fs::metadata(tmp.path().join(FIRST)).unwrap().len(), ends[1] as u64);
}

#[test]
fn a_foreign_newest_segment_is_kept_and_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..2);
    drop(w);
    std::fs::write(tmp.path().join("00000000000000000002.seg"), b"not an atlas segment").unwrap();

    let (mut w, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
    assert_eq!(recovery.foreign_segment, Some(2));
    write(&mut w, [2]);
    assert!(tmp.path().join("00000000000000000003.seg").exists());

    let mut reader = Reader::open(tmp.path(), Cursor::default());
    let got: Vec<_> =
        std::iter::from_fn(|| reader.next_record().unwrap()).map(|r| time_of(&r.payload).unwrap()).collect();
    assert_eq!(got, vec![0, 1, 2]);
    assert_eq!(reader.stats().corrupt_segments, 1);
}

#[test]
fn a_header_torn_at_creation_is_rewritten() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(FIRST), &SEGMENT_MAGIC[..3]).unwrap();
    let (mut w, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
    assert_eq!((recovery.truncated_bytes, recovery.foreign_segment), (3, None));
    write(&mut w, [5]);
    assert_eq!(read_times(tmp.path(), Cursor::default()), vec![5]);
}

#[test]
fn a_zero_filled_newest_segment_is_a_torn_header_not_foreign() {
    // NTFS can keep a new file's size after a power cut while its bytes read as zeros.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(FIRST), [0u8; 64]).unwrap();
    let (mut w, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
    assert_eq!((recovery.truncated_bytes, recovery.foreign_segment), (64, None));
    write(&mut w, [5]);
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    assert_eq!(reader.next_record().unwrap().map(|r| time_of(&r.payload)), Some(Some(5)));
    assert_eq!(reader.stats().corrupt_segments, 0);
}

/// A reader (the transport) can ack records that were written but not yet flushed.
/// If a power cut then loses them, new records must not land behind the ack.
#[test]
fn an_ack_ahead_of_a_power_cut_does_not_hide_new_records() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..4);
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    let acked = (0..4).map(|_| reader.next_record().unwrap().unwrap().next).last().unwrap();
    w.ack(acked).unwrap();
    drop(w);
    // The power cut kept only the first two records.
    let file = std::fs::OpenOptions::new().write(true).open(tmp.path().join(FIRST)).unwrap();
    file.set_len(SEGMENT_HEADER + 2 * FRAMED).unwrap();
    drop(file);

    let mut w = open(cfg(tmp.path()));
    assert_eq!(w.acked(), Some(acked));
    write(&mut w, 100..110);
    assert_eq!(read_times(tmp.path(), acked), (100..110).collect::<Vec<_>>());
}

#[test]
fn a_cursor_at_the_last_segment_number_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let mut bytes = u64::MAX.to_le_bytes().to_vec();
    bytes.extend_from_slice(&8u64.to_le_bytes());
    let crc = crc32c::crc32c(&bytes);
    bytes.extend_from_slice(&crc.to_le_bytes());
    std::fs::write(tmp.path().join("cursor"), bytes).unwrap();
    let err = Writer::open(cfg(tmp.path()), time_of).err().expect("no segment number after u64::MAX");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

fn framed(payloads: &[Vec<u8>]) -> (Vec<u8>, Vec<usize>) {
    let mut data = Vec::new();
    let mut ends = Vec::new();
    for p in payloads {
        data.extend_from_slice(&(p.len() as u32).to_le_bytes());
        data.extend_from_slice(&crc32c::crc32c(p).to_le_bytes());
        data.extend_from_slice(p);
        ends.push(data.len());
    }
    (data, ends)
}

proptest! {
    /// The pure scan behind recovery: any prefix yields exactly the whole records in it.
    #[test]
    fn scan_of_any_prefix_is_the_whole_records(
        payloads in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..300), 0..12),
        cut in any::<prop::sample::Index>(),
    ) {
        let (data, ends) = framed(&payloads);
        let cut = cut.index(data.len() + 1);
        let scan = record::scan(&data[..cut], 0);
        let whole = ends.iter().filter(|&&e| e <= cut).count();
        prop_assert_eq!(scan.records.len(), whole);
        prop_assert_eq!(scan.valid_end, if whole == 0 { 0 } else { ends[whole - 1] });
    }

    /// A changed byte inside record k leaves exactly records 0..k.
    #[test]
    fn a_changed_byte_stops_the_scan_at_its_record(
        payloads in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..300), 1..12),
        at in any::<prop::sample::Index>(),
        delta in 1u8..=255,
    ) {
        let (mut data, ends) = framed(&payloads);
        let i = at.index(data.len());
        data[i] = data[i].wrapping_add(delta);
        let k = ends.iter().filter(|&&e| e <= i).count();
        prop_assert_eq!(record::scan(&data, 0).records.len(), k);
    }

    #[test]
    fn scan_never_panics_on_arbitrary_bytes(data in prop::collection::vec(any::<u8>(), 0..2048)) {
        let scan = record::scan(&data, 0);
        prop_assert!(scan.valid_end <= data.len());
        for (start, end) in scan.records {
            prop_assert!(end - start <= MAX_PAYLOAD);
        }
    }
}
```

- [ ] **Step 2: Concurrency tests**

The two symlink tests skip (with a message) where creating a symlink is not permitted. GitHub's Windows runners are elevated, so they run there.

`crates/atlas-buffer/tests/concurrent.rs`:
```rust
//! Readers beside a live writer (sensor spec §8.5): `dump --follow` and, later, the transport.

mod common;

use std::fs::OpenOptions;
use std::io::Write;

use atlas_buffer::{Cursor, Reader, SEGMENT_HEADER};
use common::*;

fn seg(dir: &std::path::Path, seq: u64) -> std::path::PathBuf {
    dir.join(format!("{seq:020}.seg"))
}

fn times(reader: &mut Reader) -> Vec<i64> {
    std::iter::from_fn(|| reader.next_record().unwrap()).map(|r| time_of(&r.payload).unwrap()).collect()
}

#[test]
fn a_follower_sees_new_records_and_new_segments() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    assert_eq!(times(&mut reader), Vec::<i64>::new(), "nothing written yet");
    write(&mut w, 0..4);
    assert_eq!(times(&mut reader), vec![0, 1, 2, 3]);
    write(&mut w, 4..15); // rotates twice
    assert_eq!(times(&mut reader), (4..15).collect::<Vec<_>>());
    assert_eq!(reader.position().segment, 3);
}

#[test]
fn an_incomplete_tail_in_the_newest_segment_is_waited_for() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..2);
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    assert_eq!(times(&mut reader), vec![0, 1]);

    // The next record lands in two pieces, as a concurrent read may see it.
    let mut framed = Vec::new();
    let payload = rec(2);
    framed.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    framed.extend_from_slice(&crc32c::crc32c(&payload).to_le_bytes());
    framed.extend_from_slice(&payload);
    let mut file = OpenOptions::new().append(true).open(seg(tmp.path(), 1)).unwrap();
    file.write_all(&framed[..13]).unwrap();
    assert_eq!(times(&mut reader), Vec::<i64>::new(), "not written yet, not corrupt");
    file.write_all(&framed[13..]).unwrap();
    assert_eq!(times(&mut reader), vec![2]);
    assert_eq!(reader.stats().corrupt_segments, 0);
}

#[test]
fn a_bad_record_in_a_sealed_segment_skips_the_rest_of_that_segment() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20); // segments 1..=4, 6 records each (4 in the last)
    let path = seg(tmp.path(), 2);
    let mut data = std::fs::read(&path).unwrap();
    data[SEGMENT_HEADER as usize + 2 * FRAMED as usize + 20] ^= 0xff; // third record of segment 2
    std::fs::write(&path, data).unwrap();

    let mut reader = Reader::open(tmp.path(), Cursor::default());
    let expected: Vec<i64> = (0..6).chain(6..8).chain(12..20).collect();
    assert_eq!(times(&mut reader), expected);
    assert_eq!(reader.stats().corrupt_segments, 1);
}

#[test]
fn a_bad_record_in_the_newest_segment_is_not_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..3);
    let path = seg(tmp.path(), 1);
    let mut data = std::fs::read(&path).unwrap();
    data[SEGMENT_HEADER as usize + FRAMED as usize + 20] ^= 0xff; // second record
    std::fs::write(&path, data).unwrap();

    let mut reader = Reader::open(tmp.path(), Cursor::default());
    assert_eq!(times(&mut reader), vec![0], "waits: the writer may still be writing it");
    assert_eq!(reader.stats().corrupt_segments, 0);
}

#[test]
fn segments_deleted_under_a_reader_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20);
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    assert_eq!(time_of(&reader.next_record().unwrap().unwrap().payload), Some(0));

    // Segments 1 and 2 are deleted while the reader holds segment 1 open. On Windows this
    // needs FILE_SHARE_DELETE on the reader's handle.
    w.ack(Cursor { segment: 3, offset: SEGMENT_HEADER }).unwrap();
    assert!(!seg(tmp.path(), 1).exists() && !seg(tmp.path(), 2).exists());

    // The open segment is read to its end, then the deleted one is skipped.
    let expected: Vec<i64> = (1..6).chain(12..20).collect();
    assert_eq!(times(&mut reader), expected);
}

#[test]
fn a_cursor_into_a_deleted_segment_starts_at_the_next_one() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20);
    w.ack(Cursor { segment: 3, offset: SEGMENT_HEADER }).unwrap();
    assert_eq!(read_times(tmp.path(), Cursor { segment: 2, offset: 48 }), (12..20).collect::<Vec<_>>());
}

#[cfg(windows)]
#[test]
fn a_segment_that_is_a_symlink_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, [1]);
    drop(w);
    let target = tmp.path().join("elsewhere.bin");
    std::fs::copy(seg(tmp.path(), 1), &target).unwrap();
    // Creating symlinks needs Developer Mode or elevation; CI runners have it.
    if let Err(e) = std::os::windows::fs::symlink_file(&target, seg(tmp.path(), 2)) {
        eprintln!("skipped: cannot create a symlink here ({e})");
        return;
    }
    let err = atlas_buffer::Writer::open(cfg(tmp.path()), time_of).err().expect("newest segment is a symlink");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

    let mut reader = Reader::open(tmp.path(), Cursor { segment: 2, offset: 0 });
    assert_eq!(reader.next_record().unwrap_err().kind(), std::io::ErrorKind::InvalidData);

    // A sealed segment that is a symlink is refused too.
    std::fs::copy(seg(tmp.path(), 1), seg(tmp.path(), 3)).unwrap();
    let err = atlas_buffer::Writer::open(cfg(tmp.path()), time_of).err().expect("sealed segment is a symlink");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

/// A tool that opens a segment without delete sharing (an editor, `Get-Content`)
/// must not stall the writer or double-count the eviction.
#[cfg(windows)]
#[test]
fn a_segment_that_cannot_be_deleted_is_skipped_and_counted_once() {
    use std::os::windows::fs::OpenOptionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(atlas_buffer::Config { overflow: atlas_buffer::Overflow::DropOldest, ..cfg(tmp.path()) });
    write(&mut w, 0..24); // 4 full segments: the cap is reached at the next rotation
    let held = OpenOptions::new().read(true).share_mode(0x1).open(seg(tmp.path(), 1)).unwrap();

    for t in 24..31 {
        write(&mut w, [t]); // `write` panics if a tick fails
    }
    assert!(!w.is_failing());
    assert_eq!(w.stats().delete_failures, 2, "segment 1 was tried at the two rotations");
    // Segments 2 and 3 went instead; segment 1 is still there.
    assert_eq!(w.take_gaps().iter().map(|g| g.first_time).collect::<Vec<_>>(), vec![Some(6), Some(12)]);
    assert_eq!(w.stats().overflow_evictions, 2);

    drop(held);
    write(&mut w, 31..37);
    assert_eq!(w.take_gaps().len(), 1, "segment 1 goes at the next rotation");
    assert!(!seg(tmp.path(), 1).exists());
}

#[test]
fn a_buffer_directory_that_is_a_symlink_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = tmp.path().join("buffer");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(&real, &link);
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&real, &link);
    if let Err(e) = made {
        eprintln!("skipped: cannot create a symlink here ({e})");
        return;
    }
    let err = atlas_buffer::Writer::open(cfg(&link), time_of).err().expect("directory is a symlink");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}
```

- [ ] **Step 3: Run the tests and commit**

```powershell
cargo test -p atlas-buffer
git add crates/atlas-buffer/tests
git commit -m "test(buffer): torn writes at every offset, corruption, concurrent readers, reparse points"
```
Expected: `recovery` `10 passed`, `concurrent` `9 passed` (7 on Linux, where the two Windows-only tests do not exist). Check that the output contains no `skipped:` line, or note it in the PR.

### Task 8: Fuzz target and CI wiring

**Files:**
- Create: `crates/atlas-buffer/fuzz/Cargo.toml`, `fuzz/fuzz_targets/buffer_recover.rs`, `fuzz/Cargo.lock` (generated)
- Modify: `.github/workflows/fuzz.yml` (replace), `.github/workflows/ci.yml`

**Interfaces:**
- Produces: the nightly `buffer_recover` fuzz run (§12.1), and the fuzz matrix that plan 1b-2 extends with `parse_any` and `dns_query_results`. Adding a target means adding one entry to `targets` in `fuzz.yml` and one `cargo check` / `cargo audit` line in `ci.yml`.

The existing corpus cache key, `fuzz-corpus-decode_event-…`, is unchanged, so `decode_event` keeps its corpus.

- [ ] **Step 1: The fuzz crate**

`crates/atlas-buffer/fuzz/Cargo.toml`:
```toml
[package]
name = "atlas-buffer-fuzz"
version = "0.0.0"
edition = "2024"
publish = false

[package.metadata]
cargo-fuzz = true

[dependencies]
atlas-buffer = { path = ".." }
libfuzzer-sys = "0.4"

[[bin]]
name = "buffer_recover"
path = "fuzz_targets/buffer_recover.rs"
test = false
doc = false
bench = false

# Standalone workspace: cargo-fuzz needs nightly + sanitizers, so keep it out
# of the main workspace build.
[workspace]
members = ["."]
```

`crates/atlas-buffer/fuzz/fuzz_targets/buffer_recover.rs`:
```rust
#![no_main]

use std::path::PathBuf;
use std::sync::OnceLock;

use atlas_buffer::{Config, Cursor, Reader, SEGMENT_HEADER, SEGMENT_MAGIC, Writer, record};
use libfuzzer_sys::fuzz_target;

/// One scratch directory per fuzzing process, emptied before every input.
fn scratch() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("atlas-buffer-fuzz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    })
}

// Arbitrary bytes as the newest segment file: recovery must never panic; it either
// keeps the file as foreign (and only if it does not start with the segment header),
// or repairs it to exactly its valid prefix, after which a reader sees exactly
// those records.
fuzz_target!(|data: &[u8]| {
    let dir = scratch();
    for entry in std::fs::read_dir(dir).expect("list") {
        std::fs::remove_file(entry.expect("entry").path()).expect("clean");
    }
    let path = dir.join("00000000000000000001.seg");
    std::fs::write(&path, data).expect("write");

    let cfg = Config { segment_bytes: 1 << 20, cap_bytes: 4 << 20, ..Config::new(dir) };
    let (writer, recovery) = Writer::open(cfg, |_| None).expect("recovery handles any bytes");
    drop(writer);

    let header = SEGMENT_HEADER as usize;
    let is_segment = data.len() >= header && data[..header] == SEGMENT_MAGIC;
    if recovery.foreign_segment == Some(1) {
        assert!(!is_segment, "a real segment was called foreign");
        assert_eq!(std::fs::read(&path).expect("read"), data, "a foreign segment is left untouched");
        return;
    }
    assert_eq!(recovery.foreign_segment, None);
    // A repaired torn header is an empty segment: no records.
    let scan = if is_segment { record::scan(data, header) } else { record::scan(&[], 0) };
    let expected_len = if is_segment { scan.valid_end } else { header };
    assert_eq!(std::fs::metadata(&path).expect("stat").len() as usize, expected_len);
    if is_segment {
        assert_eq!(recovery.truncated_bytes as usize, data.len() - scan.valid_end);
    }

    let mut reader = Reader::open(dir, Cursor::default());
    for (start, end) in scan.records {
        let got = reader.next_record().expect("read").expect("a recovered record");
        assert_eq!(got.payload, &data[start..end]);
    }
    assert!(reader.next_record().expect("read").is_none());
    assert_eq!(reader.stats().corrupt_segments, 0);
});
```

```powershell
cargo check --manifest-path crates/atlas-buffer/fuzz/Cargo.toml
```
Expected: `Finished`, which also writes `crates/atlas-buffer/fuzz/Cargo.lock`. The existing `.gitignore` entries already cover `fuzz/target`, `corpus` and `artifacts`.

- [ ] **Step 2: Replace `.github/workflows/fuzz.yml`**

`.github/workflows/fuzz.yml`:
```yaml
# One cargo-fuzz job per (fuzz workspace, target). Spec: docs/specs/2026-09-25-scaffolding-design.md section 3.2;
# the matrix is docs/specs/2026-10-01-etw-sensor-design.md section 12.1.
name: fuzz

on:
  schedule:
    - cron: "17 3 * * *" # nightly, 03:17 UTC
  workflow_dispatch:
  pull_request:
    paths:
      - "crates/atlas-schema/**"
      - "crates/atlas-proto/**"
      - "crates/atlas-buffer/**"
      - ".github/workflows/fuzz.yml"

permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always

jobs:
  # Every target runs nightly and on manual runs. On a pull request, only the targets whose
  # watched paths changed run (all of them if this workflow changed).
  select:
    runs-on: ubuntu-latest
    outputs:
      matrix: ${{ steps.select.outputs.matrix }}
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0
      - id: select
        env:
          EVENT: ${{ github.event_name }}
          BASE: ${{ github.event.pull_request.base.sha }}
        run: |
          # crate: the directory holding fuzz/. watch: path prefixes that trigger the target on a PR.
          targets='[
            {"target": "decode_event", "crate": "crates/atlas-schema", "watch": ["crates/atlas-schema/", "crates/atlas-proto/"]},
            {"target": "buffer_recover", "crate": "crates/atlas-buffer", "watch": ["crates/atlas-buffer/"]}
          ]'
          if [ "$EVENT" = pull_request ]; then
            changed=$(git diff --name-only "$BASE"...HEAD | jq -R . | jq -s .)
            targets=$(jq -c --argjson changed "$changed" '
              if any($changed[]; . == ".github/workflows/fuzz.yml") then .
              else map(. as $t | select(any($changed[]; . as $c | any($t.watch[]; . as $w | $c | startswith($w)))))
              end' <<<"$targets")
          fi
          echo "matrix=$(jq -c '{include: .}' <<<"$targets")" >> "$GITHUB_OUTPUT"

  fuzz:
    needs: select
    if: needs.select.outputs.matrix != '{"include":[]}'
    name: ${{ matrix.target }}
    runs-on: ubuntu-latest
    timeout-minutes: 30
    strategy:
      fail-fast: false
      matrix: ${{ fromJSON(needs.select.outputs.matrix) }}
    steps:
      - uses: actions/checkout@v7
      - name: Install nightly Rust
        run: |
          rustup toolchain install nightly --profile minimal
          rustup default nightly
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-fuzz
      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.target }}
          workspaces: ${{ matrix.crate }}/fuzz -> target
      # Each run saves a new corpus entry; restore-keys picks up the most recent one for this target.
      - name: Restore corpus
        uses: actions/cache/restore@v6
        with:
          path: ${{ matrix.crate }}/fuzz/corpus
          key: fuzz-corpus-${{ matrix.target }}-${{ github.run_id }}
          restore-keys: fuzz-corpus-${{ matrix.target }}-
      - name: Fuzz
        working-directory: ${{ matrix.crate }}
        env:
          TARGET: ${{ matrix.target }}
          # 10 minutes nightly and on manual runs; 2 minutes on pull requests.
          SECONDS_TO_RUN: ${{ github.event_name == 'pull_request' && '120' || '600' }}
        # --target: the prebuilt cargo-fuzz is a static musl binary and would otherwise default to the musl
        # target, which AddressSanitizer does not support.
        run: cargo fuzz run --target x86_64-unknown-linux-gnu "$TARGET" -- -max_total_time="$SECONDS_TO_RUN"
      - name: Save corpus
        if: always()
        uses: actions/cache/save@v6
        with:
          path: ${{ matrix.crate }}/fuzz/corpus
          key: fuzz-corpus-${{ matrix.target }}-${{ github.run_id }}
      - name: Upload crashing inputs
        if: failure()
        uses: actions/upload-artifact@v7
        with:
          name: fuzz-artifacts-${{ matrix.target }}
          path: ${{ matrix.crate }}/fuzz/artifacts/
          if-no-files-found: ignore
```

- [ ] **Step 3: Extend `ci.yml`**

`.github/workflows/ci.yml`:
```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -30,11 +30,13 @@ jobs:
           workspaces: |
             . -> target
             crates/atlas-schema/fuzz -> target
+            crates/atlas-buffer/fuzz -> target
       - run: cargo fmt --all --check
       - run: cargo clippy --workspace --all-targets -- -D warnings
       - run: cargo test --workspace
-      # The fuzz crate is its own workspace, so the commands above don't build it.
+      # Each fuzz crate is its own workspace, so the commands above don't build them.
       - run: cargo check --manifest-path crates/atlas-schema/fuzz/Cargo.toml
+      - run: cargo check --manifest-path crates/atlas-buffer/fuzz/Cargo.toml
 
   rust-windows:
     runs-on: windows-latest
@@ -98,3 +100,4 @@ jobs:
           tool: cargo-audit
       - run: cargo audit
       - run: cargo audit --file crates/atlas-schema/fuzz/Cargo.lock
+      - run: cargo audit --file crates/atlas-buffer/fuzz/Cargo.lock
```

- [ ] **Step 4: Lint the workflows, then commit**

If `actionlint` is installed (`winget install rhysd.actionlint`), run it:
```powershell
actionlint .github/workflows/fuzz.yml .github/workflows/ci.yml
```
Expected: no output. Then:
```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git add crates/atlas-buffer/fuzz .github/workflows
git commit -m "ci: fuzz matrix per (crate, target) with PR selection; buffer_recover target"
```

### Task 9: Documentation, spec clarifications, PR

**Files:**
- Modify: `docs/schema-reference.md`, `docs/specs/2026-09-24-event-schema-design.md`, `docs/specs/2026-10-01-etw-sensor-design.md`, `docs/architecture-overview.md`

- [ ] **Step 1: `docs/schema-reference.md`**

1. **Classes table:** add `Open 14` to File System Activity, and add two rows:

   | Class | OCSF `class_uid` | Category | Activities (`activity_id`) | Fixture(s) |
   |---|---|---|---|---|
   | Event Log Activity | 1008 | 1 System | Stop 7, Restart 8, Disable 10 | `event_log_*.json` |
   | Sensor Health (Atlas extension 500) | 50006001 | 6 Application Activity | Report 1 | `sensor_health_report.json` |

   Under the table, note that `type_uid` for Sensor Health is 5000600101, so it needs a 64-bit integer.
2. **File System Activity:** add `Open` (a handle was opened on a watchlisted path; access intent, not proof of a read) to the activity table, with no extra fields.
3. **Registry Key Activity:** all activities gain `path_unresolved` (bool). When true, `reg_key.path` holds only what the sensor saw (a relative name, or empty).
4. **Registry Value Activity:**
   - All activities gain `path_unresolved` (for `reg_value.path`).
   - Set gains `data_read_after` (bool: the data was read from the registry after the event, not captured with it) and `data_unavailable` (bool: no data was obtained; `data` is then empty and `data_truncated` false).
   - Set also gains `raw_type` (optional u32, above 11). Exactly one of `reg_value.type` (0–11) and `raw_type` is present; neither is `reg_value.type: Missing`, as in 0a.
5. **New section "Event Log Activity":** fields `actor.process` (optional), `log_name` (session name, required, ≤ 256 B), `log_provider` (≤ 256 B, required for Disable) and `status_code` (optional u32). Activities: Stop (a session was found stopped or replaced), Restart (the agent recreated it), Disable (a provider was disabled or changed, or its canary went silent).
6. **New section "Sensor Health":**
   - `interval_start` (i64 ns), then one activity, Report, with the groups `loss`, `quality`, `housekeeping`, `resources` and `buffer`, and an optional `gap`. Each group's fields are listed as in `sensor_health.proto`.
   - Counters are deltas over `[interval_start, time]`, and gauges are sampled at the end. Absent means not measured.
   - `actor_dropped` / `actor_unresolved` are lists of `{class_uid, count}` (≤ 32). `gap` has `first_time` / `last_time` (both or neither, ordered) and `events`.
7. **Limits table:** add `log_name`, `log_provider` (256 B) and the per-class count lists (32 entries).
8. **Validation errors:** add the new paths from clarification 8.
9. **New section "The `atlas` extension namespace"** (for exporters): `path_unresolved`, `data_read_after`, `data_unavailable` and `raw_type` have no OCSF field and export as `atlas.path_unresolved`, `atlas.data_read_after`, `atlas.data_unavailable` and `atlas.raw_type`. Sensor Health is an Atlas extension class (uid 500, unregistered with OCSF; see the decision log).
10. **Known limitations:** replace the last bullet ("Items for sub-project 1 to verify…"). Sub-project 1 answered all three (sensor spec §15.3 S1, S2, S3).

- [ ] **Step 2: The 0a spec**

- Status line: replace "Sub-project 1 adds three additive schema changes: [ETW sensor spec §10](…)" with "Sub-project 1 added File System Activity `Open`, registry flags, Event Log Activity and Sensor Health, all additive: [ETW sensor spec §10](2026-10-01-etw-sensor-design.md), implemented in plan 1b-1."
- §5.9: replace "It is **empty in v1**." with "Sub-project 1 put the first fields in it: `path_unresolved`, `data_read_after`, `data_unavailable` and `raw_type` on the registry classes (sensor spec §10.4). Exporters render them as `atlas.*`. Sensor Health is an Atlas extension *class* with extension uid 500 (sensor spec §10.3)."

- [ ] **Step 3: The sensor spec**

- §8.1: add the segment header (clarification 1) and the writer API (clarification 5).
- §8.2: the writer starts a new segment at open (clarification 2).
- §8.4: add the overflow mechanics (clarification 6).
- §8.5: add what a reader treats as corruption (clarification 4).
- §10 intro: replace "updates the OCSF exporter (`atlas-schema/src/ocsf.rs`)" per clarification 7.
- §10.2: `log_name` and `log_provider` must be non-empty where required.
- §10.3: the extension uid (500, `class_uid` 50006001) and the counter semantics (D1).
- §11.1: the cursor file is `buffer\cursor`.
- Status line: "Plan 1b-1 (schema additions + `atlas-buffer`) done YYYY-MM-DD."
- §17: add a "Plan 1b-1 clarifications" bullet that lists the above.

- [ ] **Step 4: `docs/architecture-overview.md`**

- Roadmap row 1, Status: "Building: plan 1b-1 done (schema additions + `atlas-buffer`); plan 1b-2 (`atlas-etw`) next".
- Decision log (dated the day of the build), one entry: "Plan 1b-1 built: the schema additions (File `Open`, registry flags, Event Log Activity, Sensor Health with typed delta counters under Atlas extension uid 500) and `atlas-buffer` (segment header `ATLSEG01`; append queues, `tick` does the I/O; four overflow policies; a reader waits on the newest segment and skips corrupt sealed ones). The fuzz workflow is now a matrix over (crate, target)."

- [ ] **Step 5: Final verification and PR**

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --manifest-path crates/atlas-schema/fuzz/Cargo.toml
cargo check --manifest-path crates/atlas-buffer/fuzz/Cargo.toml
git add docs
git commit -m "docs(1): plan 1b-1 built; schema reference, spec clarifications, roadmap"
git push -u origin feat/1b-1-schema-buffer
gh pr create --title "Sub-project 1b-1: schema additions and atlas-buffer" --body-file <body>
```
The PR body lists the decisions applied (D1, D2), the clarifications, and the CI jobs to watch:
- `proto`: `buf breaking` against `main`;
- `rust-linux`: the first Linux build of `fsx.rs`'s non-Windows code;
- `rust-windows`: the symlink tests;
- `fuzz`: both targets run on this PR, because `fuzz.yml` changed.

Expected: all green. If `fuzz / buffer_recover` finds a crash, download the `fuzz-artifacts-buffer_recover` artifact, reproduce it with `cargo fuzz run buffer_recover <file>` on Linux, and fix the bug before merging.

- [ ] **Step 6: After merge**

Update the roadmap row to "plan 1b-2 next" (if Step 4's wording no longer fits), and delete the branch. The next plan is 1b-2 (`atlas-etw`).

## Review Log

**Independent review of the first draft (2026-10-04).** No blockers, 3 major and 10 minor findings, all folded in; the counts and claims above are re-verified.
- **M1:** a segment that could not be deleted made every write fail, and its eviction was counted on every retry. Now it is deleted first and counted after; one that cannot be deleted is skipped and counted, and the cap is exceeded rather than stalling (clarification 6). Deletes at open and on ack are best effort.
- **M2:** an ack can be ahead of what a power cut keeps. Records appended after recovery could then be skipped. Fix: the writer starts a new segment at every open (clarification 2).
- **M3:** Sensor Health had no fields for the buffer's own health (§8.3). Added the `buffer` group (D1), and moved corrupt segments out of `quality.buffer_invalid_records`, which counts records.
- **Minor:**
  - A zero-filled header is now torn, not foreign.
  - Segment numbers cannot wrap.
  - Sealed symlink segments are refused.
  - Empty gaps are not reported.
  - The rotation-failure test now really fails after a rotation, and there is a 16-segment head + tail test. `overflow_invariants` checks the files on disk.
  - Idle ticks do no I/O.
  - Writer ownership and blocking are stated (clarification 5).
  - Oversized records exceeding the cap are documented.
  - The `RegType` wording is corrected.
  - A reader skips a segment that fails to open only if it is sealed.
  - The fix for a zero-filled header first broke recovery of a torn header (the file position was left at the old end); the every-byte torn-write test caught it before the plan was regenerated.
