# Sub-project 1b-3b — Agent Windows Services Implementation Plan

> **Status:** Draft (2026-10-06); independent review next. **For agentic workers:** steps use checkbox (`- [ ]`) syntax for tracking. Task 7 needs one elevated run on the host, by the user, from a script; nothing in this plan needs the VM or a kernel driver.

**Goal:** Build the Windows side of `atlas-agent` (sensor spec §5.3, §5.5, §6, §7.2, §7.4, §7.5): the implementations behind the `services` interfaces that plan 1b-3a defined and faked.
- start-up identity: device uid, boot id, the clock anchor, the current control set, the agent's own start key;
- the `Lookups` the pipeline thread calls: the device map, live processes, account names;
- the workers that answer `Request`s: SHA-256 and Authenticode, registry value reads, 8.3 expansion, and the seeder that names handles from the system handle table.

Each part is tested on its own, against the real system. Wiring the threads to the pipeline and the agent-level live test are plan 1b-3c (decision D3).

**Architecture:** everything is in `atlas-agent/src/win/`, compiled on Windows only. All of the agent's Windows `unsafe` code lives there, behind a safe API.
- **`identity`:** `start(device_file)` returns what `Setup` needs. The boot id comes from `KUSER_SHARED_DATA.BootId`, checked against the kernel's telemetry value, and the System process's creation time.
- **`lookups::WinLookups`** implements `Lookups`:
  - the device map from `QueryDosDeviceW`, rebuilt every 60 s and on a miss (at most every 5 s);
  - `live_process` from `PROCESS_TELEMETRY_ID_INFORMATION`, never cached;
  - `account_name` from `LookupAccountSidW`, cached, on a helper thread with a 100 ms timeout so a slow domain lookup never stalls the pipeline thread (D5).
- **`Services`** owns the threads and routes each `Request` to its lane. Lanes are bounded and never block; a full lane drops the request (counted), and its event waits out its deadline.
  - **2 hash workers** (below normal): `hash`. One handle per file gives the cache key (volume, file id, USN), the bytes hashed and the signature checked.
  - **The reader lane** (1 thread): `value` reads, on the ordered path and the fast path (`FastRead`), and `expand`, the 8.3 expander with its cache (D4). Invalidations go through the same queue, so they keep their order with expansions.
  - **The seeder** (below normal, when `SeDebugPrivilege` is available): `handles` reads the system handle table and names handles through no-access duplicates (D2); `seeder` plans, names, verifies with a second table read (F2), keeps the CPU budget, and answers with a `Snapshot`.
- **Portable additions:** `config::ServiceConfig` (the services' settings) and `counters::ServiceCounters` (their Sensor Health counters).
- **Schema:** one additive Sensor Health counter, `housekeeping.service_queue_drops`.
- **CI:** a new `agent-live` job runs the tests that need `SeDebugPrivilege` or `SeBackupPrivilege`, which are `#[ignore]`d locally. Plan 1b-3c adds the agent-level live test to it.

**Tech stack:** Rust 1.97 (edition 2024).
- New: `sha2` 0.11 (MIT/Apache-2.0; the spikes measured hashing with it), Windows only.
- Existing, now also used by `atlas-agent`: `windows` 0.62 (Windows only), `blake3` 1.8, `serde_json` 1; `uuid` gains the `v4` feature.
- Tools: `buf` 1.73.0, `actionlint` 1.7.12.

**Spec:** `docs/specs/2026-10-01-etw-sensor-design.md`, revision 3, with the clarifications of plans 1b-2 and 1b-3a. Section numbers (§) refer to it.

**Verification note (2026-10-06, host, Windows 11 build 26200):** every code block below was compiled and run in a scratch worktree of `main` (5f9bb44).
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean. `atlas-agent` and `atlas-etw` are also clippy-clean for `x86_64-unknown-linux-gnu`, checked from Windows.
- **Unelevated:** `cargo test --workspace` passes 322 tests and ignores 6 (1b-2's live test and this plan's 5 elevated tests). `atlas-agent` has 139 unit tests (1b-3a had 93).
- **Elevated** (the user ran Task 7's script, the third run after two test fixes): `cargo test -p atlas-agent --lib -- --include-ignored` passed all 144, none ignored.
  - D2: on all 1,787 key handles compared, `ObjectNameInformation` on a no-access duplicate gave exactly the name `NtQueryKey(KeyNameInformation)` gives on a full-access one.
  - Start-up pass, keys: 10,158 named, 902 unnamable, in 205 ms.
  - Start-up pass, files: 3,428 named, 4,188 unnamable (mostly pipes and other non-disk handles), in 538 ms; spike S6 took 1.18 s for a similar count. One name query on the host blocked, `CancelSynchronousIo` did not free it, and its helper was set aside as stuck (F3), within the limit of 2.
  - A key and a file held by another process were named, with that process as owner; an address not in the table appeared in neither list.
  - A read of `HKLM\SAM\SAM\Domains\Account` (allowed to SYSTEM only) was denied without backup semantics and succeeded with them.
  - The boot time read without a handle equals `GetProcessTimes(PID 4)`.
- `buf lint` and `buf breaking` (against `main`) pass. Regenerating the golden fixtures changed none (line endings only). `actionlint` passes on `ci.yml`.
- Tasks 2–5 leave modules that only Task 6 calls, so their clippy checks allow `dead_code`; Task 6 runs the full check. Test counts per task were measured by building each intermediate state.
- **Not run yet:** the Linux build of `atlas-schema`'s tests (its `criterion` benchmark needs a Linux C compiler; CI has one), `cargo audit` (CI runs it), and the CI runner (build 26100), which Task 9's PR runs for the first time.

## Global Constraints

- **Branches.** This plan is reviewed on `docs/1b-3b-plan`. The build runs on `feat/1b-3b-services`, created from `main` after this plan merges. Never commit to `main`. (Claude may merge a PR itself once every check on its current head is green; the user granted that on 2026-10-05.)
- **Windows code only in `win`.** It is `#[cfg(windows)]`; the rest of `atlas-agent` stays portable and Linux CI keeps building it.
- **Every `unsafe` block has a `// SAFETY:` comment** saying why it is sound. Buffers the kernel fills are 8-aligned (`util::aligned`), and every field is read bounds-checked.
- **Least privilege:** handles from other processes are duplicated with no access rights (D2). Files are opened for reading with full sharing. Nothing in this plan writes to another process's object.
- **Never block the caller:** `Services::submit` and `FastRead` use `try_send`; `Lookups` wait at most 100 ms (`account_name`) and are otherwise cached or a single system call.
- **Elevation:** Claude never runs elevated on the host (plan 1a, D2). Tests that need privileges are `#[ignore]`d; CI's `agent-live` job runs them, and so does Task 7's script, which the user runs.
- **Additive schema only:** `buf breaking` must pass, and the existing golden fixtures must stay byte-identical.
- **Formatting and lint:** the repo's `rustfmt.toml`; clippy with `-D warnings` at the end of every task (allowing `dead_code` in Tasks 2–5).
- **Shell:** PowerShell 7 (`cargo` commands are the same in bash).
- Every commit message ends with the attribution lines the session's system reminder specifies.

## Decisions (chosen 2026-10-06: D1 A, D2 A, D3 A, D4 A, D5 A)

### D1, when the driver and the agent-level live test come

1b-3a's Interfaces gave the driver (the thread wiring) to 1b-4, while §12.3 promised the agent-level live test (seeding, value reads, 8.3 + watchlist, undelete) to 1b-3 and 1b-4. There is no end-to-end run without a driver.
- **(A) Chosen: a minimal driver (to a test sink, not the buffer) and the agent-level live test come before 1b-4.** Integration problems between the real services and the pipeline show up in a plan about those services, not mixed into 1b-4's service and watchdog work.
- (B) The services only, each tested on its own; end-to-end waits for 1b-4. (C) The driver too, but its end-to-end test waits for 1b-4.

### D2, how the seeder names key handles (refines §7.4)

Verified on the host, unelevated, on the agent's own handles:
- `NtQueryKey(KeyNameInformation)` is denied (`STATUS_ACCESS_DENIED`) on a duplicate with no access, and works with any non-zero access, even `KEY_SET_VALUE` alone.
- A duplicate cannot gain a right its source lacks, so the spec's fallback (`KEY_QUERY_VALUE`) fails for every handle opened without it.
- `NtQueryObject(ObjectNameInformation)` works on the no-access duplicate and gives the same name. A deleted key fails both ways (`STATUS_KEY_DELETED`).
- File handles are named with no access (`GetFinalPathNameByHandleW`, and the device check), as the spec says.

- **(A) Chosen: no-access duplicates; keys named with `ObjectNameInformation`.** The agent never holds a copy of another process's handle that could read or write its object. Confirmed elevated on 1,787 key handles (verification note).
- (B) `DUPLICATE_SAME_ACCESS` and `NtQueryKey`, as spikes S6/S7 measured: for a moment the agent, as SYSTEM, holds other processes' write and delete rights.

### D3, how plan 1b-3b is split

With D1, this plan would hold eight services, the driver and the agent-level live test: likely longer than 1b-3a (7,556 lines).
- **(A) Chosen: two plans.** 1b-3b (this plan) is the services, each tested against the real system. **1b-3c** is the minimal driver and the agent-level live test, designed once the real services' behaviour is known. Plan 1b becomes six plans.
- (B) One plan, built in two PRs: about 10,000 lines to review, with the live test written before the services exist. (C) One plan and one PR.

### D4, the 8.3 expansion cache (refines §7.2)

Verified on the host:
- `GetLongPathNameW` rejects `\\?\GLOBALROOT\Device\…` paths (`ERROR_INVALID_NAME`), even fully long ones, so it cannot expand NT paths or shadow copies (F1). It also costs 1.55 ms for a path with 7 short components, because it looks up every component.
- Looking up only the short components, each with `NtQueryDirectoryFile` on its parent directory's NT path and the short name as the filter, gives the exact long path in about 54 µs per short component, and works for shadow copies.
- Short names are reused: a directory created after another was deleted got the deleted one's `SECRET~1`, and a rename gives a new short name.

- **(A) Chosen: a cache by (parent directory, short name), invalidated by the pipeline's `InvalidateHash` requests, with a 60 s TTL.** The pipeline already sends `InvalidateHash` for every Delete, Rename source, Update and SetAttributes it sees; `Services` also routes it to the expander, which drops that path and everything under it. The TTL covers changes the pipeline never names. No change to 1b-3a's interfaces.
- (B) No cache: about 54 µs per short component, for every event that has one (13% of file events on the CI runner).
- (C) TTL only: after a short name is reused, a watchlisted file could be reported under the old directory, and missed, for up to a minute.

### D5, defaults for everything else

Accepted together:
- the Windows code as a `cfg(windows)` module of `atlas-agent` (as `atlas-etw`'s `session`);
- `account_name` on a helper thread with a 100 ms timeout; a slow answer serves later events;
- one `Services` object owning the lanes, so plan 1b-3c's driver only wires channels;
- bounded lanes (8,192 requests each); a dropped request costs only its event's deadline (1b-3a, clarification 25), and is counted;
- `sha2` for SHA-256;
- value reads fall back to a normal open without `SeBackupPrivilege` (unelevated runs);
- the tests that need privileges run in CI and once on the host (build 26200; CI is 26100).

## Findings from the build

- **F1, `GetLongPathNameW` cannot expand NT paths** (D4). The expander opens directories by NT path instead.
- **F2, a handle can change between the table read and the duplicate.** A process can close a handle and get the same handle value for another object in between, and the seeder would name the wrong object. The seeder keeps its duplicates open, reads the table a second time, and keeps a name only if its own duplicate sits at the address asked about. The snapshot's `taken` is the QPC of that second read: every name was valid then, and the pipeline's reuse rule (1b-3a, clarification 18) covers everything before. Cost: one more table read (about 40 ms).
- **F3, which name queries block.** The device check (`FileFsDeviceInformation`) is answered at once even with a read pending on a pipe, so pipes are filtered out before any query that can block. `GetFinalPathNameByHandleW` on a disk file blocks while another thread's synchronous operation on the same file object is pending (a byte-range lock wait in the test), and `CancelSynchronousIo` does not free it. The helper is set aside as stuck until the operation ends; the elevated start-up pass met one such handle.
- **F4, kernel key names keep their case:** 96 of the host's key handles are named `\Registry\Machine\…` or `\Registry\User\…`. `paths::registry` already matches the roots and the control set case-insensitively.
- **F5, PID 4 cannot be opened unelevated.** The System process's creation time comes from `SystemProcessInformation` instead: the same kernel field `GetProcessTimes` returns, read without a handle. The elevated test checks they are equal.
- **F6, a dead counter:** 1b-3a's `Counters::seeder_deferred` was never incremented; deferral happens in the seeder, which counts `seeder_deferred_rereads`. It is removed.
- **F7, `\DEVICE\` in upper case** was not recognised as a device path by the expander; the routing test caught it, and `split_device_in_any_case` pins it.

## Deliberate clarifications of the spec (applied to the spec text in Task 8)

1. **Plan 1b is six plans** (D1, D3): 1b-3b the Windows services, **1b-3c** the minimal driver and the agent-level live test, 1b-4 unchanged otherwise.
2. **Key naming** (D2; §7.4): no-access duplicates for keys and files; keys are named with `ObjectNameInformation`. The `KEY_QUERY_VALUE` / `FILE_READ_ATTRIBUTES` fallback is removed.
3. **8.3 expansion** (D4, F1; §7.2):
   - each short component (the §7.2 pattern) is looked up in its parent directory, opened by NT path, with `NtQueryDirectoryFile` (`FileBothDirectoryInformation`) and the short name as the filter; the answer is accepted only if its short or long name equals the component exactly;
   - only `\Device\HarddiskVolume…` paths (volumes and shadow copies) are expanded; a network redirector could stall the reader lane;
   - the cache is keyed by (parent directory, short name). `InvalidateHash(path)` drops the entry for the path and everything under it, resolving short components from the cache only; entries expire after 60 s.
4. **Seeding** (§7.4):
   - each covered address is answered once: `named` if any holder's handle was named, else `unnamable`;
   - its owner is the holder with the lowest PID, the agent excluded; an address only the agent holds is unnamable, with the agent as owner;
   - the name query is made once per address; a holder that cannot be opened or duplicated is skipped for the next;
   - verification by a second table read, and `taken` = its QPC (F2);
   - when file seeding pauses (2 stuck helpers), the remaining file addresses of that snapshot are answered unnamable;
   - the CPU budget charges the seeder thread's CPU time (not its helpers'), and not the start-up pass. Re-reads that wait for the budget are merged per kind and counted once each (`seeder_deferred_rereads`);
   - a table read that fails gets no reply: the waiting events run to their deadlines.
5. **Boot time** (F5; §6.1) is read from `SystemProcessInformation`.
6. **`device.json`** (§6.1): `{"device_uid": "<32 lowercase hex>"}`, written once through a temporary file and a hard link, so no reader sees a partial file and a second writer loses cleanly. A damaged file is an error, never replaced: a new uid would split the device's history. Its location and permissions are plan 1b-4's (§11.1).
7. **Hashes and signatures** (§6.3):
   - the file is opened by `\\?\GLOBALROOT` + its NT path, for reading, with full sharing and backup semantics; one handle gives the key, the hash and the signature;
   - a USN of zero (the journal has not recorded the file) counts as no USN: not cached;
   - results with an operational error are not cached; the cache (65,536 entries) starts over when full, counting `hash_cache_evictions`;
   - the signature is checked even above the hash size cap;
   - `signer` is the leaf certificate's subject CN for both `Valid` and `Invalid`;
   - the `WinVerifyTrust` result maps to `Invalid` for failures of the signature or its chain (certificate facility `0x800B….`, trust errors `0x80096002`–`0x800960FF`, revocation, admin policy, malformed ASN.1 `0x80093xxx`), and to an operational error otherwise.
8. **Account names** (D5; §5.3): cached, failures included (4,096 entries, cleared when full); a lookup waits at most 100 ms, and one still running is not waited for again.
9. **The device map** (§5.5) keeps only `\Device\…` targets: a `subst` drive's target is itself a drive path.
10. **Value reads** (§7.5): without `SeBackupPrivilege`, a normal open. Values larger than 16 MiB are not read (the read fails).
11. **Service lanes** (§3.2): the hash workers, the reader lane and the seeder each take at most 8,192 waiting requests (`ServiceConfig::lane_cap`). A full lane drops the request and counts `service_queue_drops`; its event waits out its deadline. An `InvalidateHash` also goes to the reader lane, in order with expansions.
12. **Sensor Health** (§9.3, §10.3): `housekeeping.service_queue_drops` is added; the pipeline's unused `seeder_deferred` is removed (F6).
13. **Tests** (§12.3): CI's `agent-live` job runs the service tests that need privileges; the agent-level live test comes with plan 1b-3c.
14. **Known limitations** (§16):
    - 8.3 expansion happens about 1 s after the event (the ordering hold). A directory swapped within that window resolves to the new one's long name.
    - Paths on network redirectors are not expanded.

## Review Focus

These would slip past a plain unit test, so each has a pinned check:
1. **No-access naming:** keys through `ObjectNameInformation` (`keys_are_named_through_a_no_access_duplicate`; elevated: `object_name_matches_key_name_information`).
2. **Verification of seeded names:** a name is kept only if the agent's duplicate is at the address (`verification_needs_our_duplicate_at_the_address`).
3. **Owners and coverage:** lowest PID, never the agent, each address once, absent addresses in neither list (`plan_groups_by_address_with_the_lowest_pid_first`; elevated: `names_a_key_and_a_file_another_process_holds`, `start_up_pass_covers_the_table`).
4. **Blocked name queries** time out, are set aside, pause file seeding at the limit, and recover (`blocked_queries_time_out_and_recover`).
5. **The CPU budget** defers re-reads until charges age out (`budget_defers_until_charges_age_out`).
6. **The 8.3 cache:** reuse after a delete, invalidation by long or short path, siblings and look-alikes kept, the TTL, local volumes only (`invalidation_by_long_path_covers_the_entry_and_below`, `invalidation_by_short_path`, `invalidation_leaves_siblings_and_lookalikes`, `entries_expire`, `only_local_volumes`).
7. **Value reads:** symbolic-link keys are not followed (with a control that a normal open follows the link), embedded NULs in value names, the 4 KiB cut with the full size kept (`symbolic_link_keys_are_not_followed`, `embedded_nuls_in_value_names_are_kept`, `reads_types_sizes_and_truncates`; elevated: `backup_semantics_pass_a_system_only_dacl`).
8. **Signatures:** catalog-signed, embedded-signed, a tampered embedded-signed copy is `Invalid`, the size cap skips only the hash (`catalog_signed_os_file`, `embedded_signature_and_tampering`, `size_cap_skips_the_hash_only`, `classification`).
9. **The hash cache key** changes with each version of a file, and errors are not cached (`hashes_a_file_and_caches_by_version`, `missing_files_are_errors_and_not_cached`).
10. **Never block the pipeline thread:** a slow account lookup returns within the timeout and is not waited for twice (`accounts_are_cached_and_never_wait_long`); a full lane drops and counts, and each request is answered or counted, never both (`full_lanes_drop_and_count`).
11. **Identity:** the boot id formula, the start key's BootId, `device.json` written once and never replaced (`boot_id_formula`, `start_reads_this_machine`, `device_uid_is_created_once_and_kept`; elevated: `boot_time_matches_get_process_times`).
12. **Every request kind is answered** through `Services` (`every_request_kind_is_answered`).

## File Structure

```
Cargo.toml                                    + sha2 in [workspace.dependencies]
crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto   + service_queue_drops
crates/atlas-schema/src/classes/sensor_health.rs               + the field
crates/atlas-schema/tests/common/mod.rs                        strategies cover it
.github/workflows/ci.yml                                       + agent-live job
crates/atlas-agent/
  Cargo.toml           + blake3, serde_json, uuid v4; Windows: sha2, windows
  src/lib.rs           + win (Windows only)
  src/config.rs        + ServiceConfig
  src/counters.rs      + ServiceCounters; − Counters::seeder_deferred
  src/win/mod.rs       module list, Services re-export
  src/win/util.rs      wide strings, owned handles, QPC, counted strings, aligned buffers
  src/win/privilege.rs enabling SeDebugPrivilege and SeBackupPrivilege
  src/win/telemetry.rs PROCESS_TELEMETRY_ID_INFORMATION
  src/win/identity.rs  Started, start, anchor, boot_id, device_uid
  src/win/lookups.rs   DeviceMap, Accounts, WinLookups
  src/win/hash.rs      HashCache, Enricher (SHA-256, Authenticode)
  src/win/value.rs     registry value reads
  src/win/expand.rs    Expander, NtDir (8.3)
  src/win/handles.rs   the handle table, no-access duplicates, FileNamer
  src/win/seeder.rs    plan, verified, Budget, Seeder, the seeder thread
  src/win/services.rs  Services: lanes, routing, FastRead
spikes/run-1b3b-elevated.ps1                   git-ignored; Task 7
docs/…                 Task 8
```

## Interfaces for later plans

- **Plan 1b-3c (the driver and the agent-level live test):**
  - `win::identity::start(device_file)` gives `Started`: `identity`, `ticks`, `anchor`, `current_control_set`, `self_key` (for `Setup::self_keys`) and `started`. Log `boot_id_disagreement` if set. Call `win::identity::anchor()` every 60 s for `Pipeline::set_anchor`.
  - `win::lookups::WinLookups::new()` is the `Lookups` for `Pipeline::new`.
  - `win::Services::start(&ServiceConfig::default())`:
    - if `seeding()` is false, set `Config::seed_on_start` and `seed_on_miss` to false before building the pipeline;
    - `fast_read()` gives Session A's `Intake` its `FastRead` (call once per `Intake`);
    - pass every request from `Pipeline::take_requests()` to `submit()`, and every reply from `replies()` to `Pipeline::reply()`.
  - Dropping `Services` closes the lanes; the threads exit when their queue is empty. A stuck seeder helper exits when its query returns.
- **Plan 1b-4 (Sensor Health and the service):**
  - `Services::counters()` gives `ServiceCounters`. Its fields are `housekeeping` fields of the same names; `seeder_stuck_helpers` is a gauge. `housekeeping.seeding_enabled` is `Services::seeding()`.
  - `device.json` lives in `C:\ProgramData\Atlas\` with the §11.1 permissions; `identity::start` takes its path.
  - `ServiceConfig` is read from `agent.toml` with the pipeline's `Config`.

---

### Task 1: The schema counter

**Files:**
- Modify: `crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto`, `crates/atlas-schema/src/classes/sensor_health.rs`, `crates/atlas-schema/tests/common/mod.rs`

- [ ] **Step 1: `housekeeping.service_queue_drops`**

`sensor_health.proto`:
```diff
--- a/crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto
+++ b/crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto
@@ -101,6 +101,9 @@ message SensorHousekeeping {
   // Gauges.
   optional uint64 seeder_stuck_helpers = 16;
   optional uint64 seeder_negative_cache_size = 17;
+  // Requests to the hash workers, reader lane or seeder dropped because the lane was full;
+  // each one's event waited out its deadline.
+  optional uint64 service_queue_drops = 18;
 }
 
 // The agent's own resource use.
```

`crates/atlas-schema/src/classes/sensor_health.rs`:
```diff
--- a/crates/atlas-schema/src/classes/sensor_health.rs
+++ b/crates/atlas-schema/src/classes/sensor_health.rs
@@ -195,6 +195,8 @@ counter_group!(
         seeder_stuck_helpers: Option<u64>,
         /// Gauge.
         seeder_negative_cache_size: Option<u64>,
+        /// Requests dropped because their service lane was full.
+        service_queue_drops: Option<u64>,
     }
 );
 
```

`crates/atlas-schema/tests/common/mod.rs` (the property-test strategy covers the new field):
```diff
--- a/crates/atlas-schema/tests/common/mod.rs
+++ b/crates/atlas-schema/tests/common/mod.rs
@@ -413,7 +413,7 @@ fn arb_health_report() -> BoxedStrategy<HealthReport> {
         });
     let housekeeping = (
         (counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
-        (any::<Option<bool>>(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
+        (any::<Option<bool>>(), counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
     )
         .prop_map(|(a, b)| SensorHousekeeping {
             process_cache_evictions: a.0,
@@ -433,6 +433,7 @@ fn arb_health_report() -> BoxedStrategy<HealthReport> {
             seeder_deferred_rereads: b.5,
             seeder_stuck_helpers: b.6,
             seeder_negative_cache_size: b.7,
+            service_queue_drops: b.8,
         });
     let resources =
         (counter(), counter()).prop_map(|(cpu_time, working_set)| SensorResources { cpu_time, working_set });
```

- [ ] **Step 2: Check and commit**

```powershell
cargo test -p atlas-schema -p atlas-proto
buf lint crates/atlas-proto/proto
buf breaking crates/atlas-proto/proto --against '.git#branch=main,subdir=crates/atlas-proto/proto'
$env:ATLAS_UPDATE_FIXTURES = '1'; cargo test -p atlas-schema --test golden; Remove-Item Env:ATLAS_UPDATE_FIXTURES
git diff --ignore-cr-at-eol --stat crates/atlas-schema/tests/fixtures
```
Expected: all pass, `buf` reports nothing, and the fixture diff is empty (`git checkout` the line-ending noise).
```powershell
git add crates/atlas-proto crates/atlas-schema
git commit -m "feat(schema): Sensor Health service_queue_drops"
```

### Task 2: Manifests, settings, counters, identity and lookups

**Files:**
- Modify: `Cargo.toml`, `crates/atlas-agent/Cargo.toml`, `src/lib.rs`, `src/config.rs`, `src/counters.rs`
- Create: `crates/atlas-agent/src/win/mod.rs`, `util.rs`, `privilege.rs`, `telemetry.rs`, `identity.rs`, `lookups.rs`

- [ ] **Step 1: Manifests**

`Cargo.toml`:
```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -22,6 +22,7 @@ prost-reflect = { version = "0.16", features = ["serde"] }
 proptest = "1.11"
 protox = "0.9"
 serde_json = "1"
+sha2 = "0.11"
 tempfile = "3.27"
 thiserror = "2"
 uuid = { version = "1.26", features = ["v7"] }
```

`crates/atlas-agent/Cargo.toml`:
```diff
--- a/crates/atlas-agent/Cargo.toml
+++ b/crates/atlas-agent/Cargo.toml
@@ -10,12 +10,41 @@ publish.workspace = true
 [dependencies]
 atlas-etw.workspace = true
 atlas-schema.workspace = true
+blake3.workspace = true
 globset.workspace = true
-uuid.workspace = true
+serde_json.workspace = true
+uuid = { workspace = true, features = ["v4"] }
+
+[target.'cfg(windows)'.dependencies]
+sha2.workspace = true
+
+[target.'cfg(windows)'.dependencies.windows]
+workspace = true
+features = [
+    "Wdk_Foundation",
+    "Wdk_Storage_FileSystem",
+    "Wdk_System_Registry",
+    "Wdk_System_SystemInformation",
+    "Wdk_System_Threading",
+    "Win32_Security",
+    "Win32_Security_Authorization",
+    "Win32_Security_Cryptography",
+    "Win32_Security_Cryptography_Catalog",
+    "Win32_Security_Cryptography_Sip",
+    "Win32_Security_WinTrust",
+    "Win32_Storage_FileSystem",
+    "Win32_System_IO",
+    "Win32_System_Ioctl",
+    "Win32_System_Kernel",
+    "Win32_System_Performance",
+    "Win32_System_Pipes",
+    "Win32_System_Registry",
+    "Win32_System_SystemInformation",
+    "Win32_System_Threading",
+]
 
 [dev-dependencies]
 atlas-proto.workspace = true
 prost.workspace = true
 prost-reflect.workspace = true
 proptest.workspace = true
-serde_json.workspace = true
```

`crates/atlas-agent/src/lib.rs`:
```diff
--- a/crates/atlas-agent/src/lib.rs
+++ b/crates/atlas-agent/src/lib.rs
@@ -16,3 +16,5 @@ pub mod recent;
 pub mod services;
 pub mod time;
 pub mod watchlist;
+#[cfg(windows)]
+pub mod win;
```

- [ ] **Step 2: Settings and counters**

`src/config.rs`:
```diff
--- a/crates/atlas-agent/src/config.rs
+++ b/crates/atlas-agent/src/config.rs
@@ -91,6 +91,43 @@ impl Default for Config {
     }
 }
 
+/// Settings of the Windows services (sensor spec §6.3, §7.4; plan 1b-3b).
+#[derive(Debug, Clone, PartialEq, Eq)]
+pub struct ServiceConfig {
+    /// Hash and signature workers (§6.3).
+    pub hash_workers: usize,
+    /// Files larger than this are not hashed (§6.3).
+    pub hash_size_cap: u64,
+    /// Cached hash results; the cache starts over when full.
+    pub hash_cache_cap: usize,
+    /// The seeder's CPU budget: at most `seeder_cpu` per `seeder_cpu_window`
+    /// (default 1% of one core over 60 s). Re-reads past it are deferred (§7.4).
+    pub seeder_cpu: Duration,
+    pub seeder_cpu_window: Duration,
+    /// A file name query that takes longer is abandoned (§7.4).
+    pub name_timeout: Duration,
+    /// Past this many stuck name queries, file seeding pauses (§7.4).
+    pub max_stuck_helpers: usize,
+    /// Requests waiting per lane (hash workers, reader lane, seeder). A full
+    /// lane drops the request; its event waits out its deadline.
+    pub lane_cap: usize,
+}
+
+impl Default for ServiceConfig {
+    fn default() -> Self {
+        ServiceConfig {
+            hash_workers: 2,
+            hash_size_cap: 100 << 20,
+            hash_cache_cap: 65_536,
+            seeder_cpu: Duration::from_millis(600),
+            seeder_cpu_window: Duration::from_secs(60),
+            name_timeout: Duration::from_millis(200),
+            max_stuck_helpers: 2,
+            lane_cap: 8_192,
+        }
+    }
+}
+
 /// Converts durations to QPC ticks (the pipeline's only clock, §3.3).
 #[derive(Debug, Clone, Copy, PartialEq, Eq)]
 pub struct Ticks {
```

`src/counters.rs` (`seeder_deferred` was never incremented, F6):
```diff
--- a/crates/atlas-agent/src/counters.rs
+++ b/crates/atlas-agent/src/counters.rs
@@ -61,7 +61,6 @@ pub struct Counters {
     pub flow_table_evictions: u64,
     pub file_op_failed: u64,
     pub pending_overflow: u64,
-    pub seeder_deferred: u64,
     /// Events dropped at emission because their actor is the agent (§5.5).
     pub self_filtered: u64,
 }
@@ -96,3 +95,23 @@ impl IntakeCounters {
         c.load(Ordering::Relaxed)
     }
 }
+
+/// Counters kept by the Windows services (plan 1b-3b), shared with their
+/// threads. Each one is a Sensor Health field; `seeder_stuck_helpers` is a
+/// gauge, the rest count occurrences.
+#[derive(Debug, Default)]
+pub struct ServiceCounters {
+    /// Requests dropped because their lane was full (`housekeeping.service_queue_drops`).
+    /// The event waits out its deadline.
+    pub service_queue_drops: AtomicU64,
+    /// Hash results dropped when the cache started over (`housekeeping.hash_cache_evictions`).
+    pub hash_cache_evictions: AtomicU64,
+    pub seeder_handles_named: AtomicU64,
+    /// Covered addresses answered as unnamable, including timed-out ones.
+    pub seeder_handles_failed: AtomicU64,
+    pub seeder_handles_timed_out: AtomicU64,
+    pub seeder_table_reads: AtomicU64,
+    /// Re-reads that waited for the CPU budget (§7.4).
+    pub seeder_deferred_rereads: AtomicU64,
+    pub seeder_stuck_helpers: AtomicU64,
+}
```

- [ ] **Step 3: The module, helpers and privileges**

`src/win/mod.rs` (this is the final form; until Tasks 3–6 add their modules, leave out `mod expand;`, `mod handles;`, `mod hash;`, `mod seeder;`, `mod services;`, `mod value;` and the `pub use`):
```rust
//! The Windows services behind [`crate::services`] (sensor spec §5–§7; plan 1b-3b).
//! All of the agent's Windows `unsafe` code lives in this module, behind a safe API.
//!
//! - [`identity`]: device and boot identity, the anchor, and `Setup`'s facts.
//! - [`lookups::WinLookups`]: the [`crate::services::Lookups`] the pipeline thread calls.
//! - [`Services`]: the hash workers, the reader lane and the seeder, answering
//!   [`crate::services::Request`]s.

mod expand;
mod handles;
mod hash;
pub mod identity;
pub mod lookups;
pub mod privilege;
mod seeder;
mod services;
mod telemetry;
mod util;
mod value;

pub use services::Services;
```

`src/win/util.rs`:
```rust
//! Small helpers shared by the Windows services.

use windows::Win32::Foundation::{CloseHandle, HANDLE, UNICODE_STRING};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::core::PWSTR;

/// A NUL-terminated UTF-16 copy of `s`.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A kernel handle closed on drop.
#[derive(Debug)]
pub(crate) struct Owned(pub HANDLE);

impl Owned {
    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0.0.is_null() {
            // SAFETY: the handle is owned by this value and closed only here.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// SAFETY: a kernel handle value can be used and closed from any thread.
unsafe impl Send for Owned {}

/// The current QPC value.
pub(crate) fn qpc_now() -> i64 {
    let mut v = 0i64;
    // SAFETY: writes one i64; cannot fail on Windows XP and later.
    let _ = unsafe { QueryPerformanceCounter(&mut v) };
    v
}

/// QPC ticks per second.
pub(crate) fn qpc_frequency() -> i64 {
    let mut v = 0i64;
    // SAFETY: as above.
    let _ = unsafe { QueryPerformanceFrequency(&mut v) };
    v
}

/// A counted string over `units`, or `None` if it is longer than a
/// `UNICODE_STRING` can describe (32,767 units). `units` must outlive it.
pub(crate) fn counted(units: &mut [u16]) -> Option<UNICODE_STRING> {
    let bytes = u16::try_from(units.len() * 2).ok().filter(|b| *b <= 0xFFFE)?;
    Some(UNICODE_STRING { Length: bytes, MaximumLength: bytes, Buffer: PWSTR(units.as_mut_ptr()) })
}

/// The UTF-16 string at `off` in `b`, up to its NUL (or the end of `b`).
pub(crate) fn utf16z_at(b: &[u8], off: usize) -> Option<String> {
    let units: Vec<u16> =
        b.get(off..)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    Some(String::from_utf16_lossy(&units))
}

pub(crate) fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

pub(crate) fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

/// A buffer of `bytes` bytes (rounded up to 8), aligned for the structures the
/// NT query functions return.
pub(crate) fn aligned(bytes: usize) -> Vec<u64> {
    vec![0u64; bytes.div_ceil(8)]
}

/// The bytes of an [`aligned`] buffer.
pub(crate) fn as_bytes(buf: &[u64]) -> &[u8] {
    // SAFETY: any u64 slice is a valid byte slice of eight times its length.
    unsafe { std::slice::from_raw_parts(buf.as_ptr().cast(), buf.len() * 8) }
}

/// FILETIME (100 ns since 1601) → Unix nanoseconds.
pub(crate) fn filetime_to_unix_ns(ft: u64) -> i64 {
    const EPOCH_DIFF: i64 = 116_444_736_000_000_000;
    (ft as i64 - EPOCH_DIFF).saturating_mul(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16z_stops_at_nul_and_end() {
        let b: Vec<u8> = "ab\0c".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(utf16z_at(&b, 0).as_deref(), Some("ab"));
        assert_eq!(utf16z_at(&b, 6).as_deref(), Some("c"));
        assert_eq!(utf16z_at(&b, 100), None);
    }

    #[test]
    fn counted_strings_have_a_limit() {
        let mut ok = vec![b'a' as u16; 32_767];
        assert_eq!(counted(&mut ok).map(|u| u.Length), Some(65_534));
        let mut long = vec![b'a' as u16; 32_768];
        assert!(counted(&mut long).is_none());
    }

    #[test]
    fn filetime_epoch() {
        assert_eq!(filetime_to_unix_ns(116_444_736_000_000_000), 0);
        assert_eq!(filetime_to_unix_ns(116_444_736_000_000_001), 100);
    }
}
```

`src/win/privilege.rs`:
```rust
//! Enabling token privileges (sensor spec §7.4, §7.5).

use windows::Win32::Foundation::{ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

use super::util::{Owned, wide};

/// Object addresses in the handle table, and duplicating other processes' handles (§7.4).
pub const DEBUG: &str = "SeDebugPrivilege";
/// Opening registry keys past their DACL for value reads (§7.5).
pub const BACKUP: &str = "SeBackupPrivilege";

/// Enables `name` in the process token. Returns whether it is now enabled:
/// false when the token does not hold it (an unelevated run).
pub fn enable(name: &str) -> bool {
    let mut token = HANDLE::default();
    // SAFETY: opens our own token; the handle is closed by `Owned`.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let token = Owned(token);
    let name = wide(name);
    let mut luid = LUID::default();
    // SAFETY: `name` is NUL-terminated and outlives the call.
    if unsafe { LookupPrivilegeValueW(PCWSTR::null(), PCWSTR(name.as_ptr()), &mut luid) }.is_err() {
        return false;
    }
    let tp = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
    };
    // SAFETY: `tp` is a valid one-entry TOKEN_PRIVILEGES. The call succeeds even
    // when the privilege is not held, so the last error tells.
    let ok = unsafe { AdjustTokenPrivileges(token.raw(), false, Some(&tp), 0, None, None) }.is_ok();
    ok && unsafe { GetLastError() } != ERROR_NOT_ALL_ASSIGNED
}
```

- [ ] **Step 4: Process telemetry and identity**

`src/win/telemetry.rs`:
```rust
//! `PROCESS_TELEMETRY_ID_INFORMATION` (`NtQueryInformationProcess` class 64;
//! sensor spec §5.2, §6.1): a live process's start key, BootId, image path and
//! command line. The layout is undocumented (phnt `ntpsapi.h`); every field is
//! read bounds-checked, and the spikes (S1, S2, S8) confirmed it on the host.

use windows::Wdk::System::Threading::{NtQueryInformationProcess, PROCESSINFOCLASS};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

use super::util::{Owned, aligned, as_bytes, u32_at, u64_at, utf16z_at};

const PROCESS_TELEMETRY_ID_INFORMATION: PROCESSINFOCLASS = PROCESSINFOCLASS(64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Telemetry {
    pub pid: u32,
    pub start_key: u64,
    pub boot_id: u32,
    /// NT path (`\Device\HarddiskVolume3\…`).
    pub image_path: Option<String>,
    pub command_line: Option<String>,
}

/// Field offsets (x64).
const PID: usize = 4;
const START_KEY: usize = 8;
const BOOT_ID: usize = 60;
const IMAGE_PATH_OFFSET: usize = 76;
const COMMAND_LINE_OFFSET: usize = 88;
/// The fixed part ends after `CommandLineOffset`.
const FIXED: usize = 92;

/// Parses the buffer the kernel filled. `None` if it is too short.
pub(crate) fn parse(b: &[u8]) -> Option<Telemetry> {
    if b.len() < FIXED {
        return None;
    }
    let string = |field: usize| match u32_at(b, field)? as usize {
        0 => None,
        off => utf16z_at(b, off),
    };
    Some(Telemetry {
        pid: u32_at(b, PID)?,
        start_key: u64_at(b, START_KEY)?,
        boot_id: u32_at(b, BOOT_ID)?,
        image_path: string(IMAGE_PATH_OFFSET),
        command_line: string(COMMAND_LINE_OFFSET),
    })
}

/// The process with this PID, if it is running and can be opened.
pub(crate) fn query(pid: u32) -> Option<Telemetry> {
    // SAFETY: the handle is closed by `Owned`.
    let h = Owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?);
    let t = query_handle(h.raw())?;
    // A PID can be reused at any moment; the kernel answered for this handle's process.
    (t.pid == pid).then_some(t)
}

/// The agent's own process.
pub(crate) fn current() -> Option<Telemetry> {
    // SAFETY: the pseudo-handle needs no closing.
    query_handle(unsafe { GetCurrentProcess() })
}

fn query_handle(h: HANDLE) -> Option<Telemetry> {
    let mut buf = aligned(4096);
    loop {
        let mut ret = 0u32;
        let len = (buf.len() * 8) as u32;
        // SAFETY: `buf` is writable for `len` bytes and 8-aligned.
        let st = unsafe {
            NtQueryInformationProcess(h, PROCESS_TELEMETRY_ID_INFORMATION, buf.as_mut_ptr().cast(), len, &mut ret)
        };
        if st.is_ok() {
            return parse(as_bytes(&buf).get(..ret as usize)?);
        }
        if ret > len && ret <= 1 << 20 {
            buf = aligned(ret as usize);
            continue;
        }
        return None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(image: Option<&str>, cmd: Option<&str>) -> Vec<u8> {
        let mut b = vec![0u8; 96];
        b[0..4].copy_from_slice(&96u32.to_le_bytes());
        b[4..8].copy_from_slice(&1234u32.to_le_bytes());
        b[8..16].copy_from_slice(&0x007c_0000_0000_33eeu64.to_le_bytes());
        b[60..64].copy_from_slice(&124u32.to_le_bytes());
        for (field, s) in [(IMAGE_PATH_OFFSET, image), (COMMAND_LINE_OFFSET, cmd)] {
            if let Some(s) = s {
                let off = b.len() as u32;
                b[field..field + 4].copy_from_slice(&off.to_le_bytes());
                b.extend(s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
            }
        }
        b
    }

    #[test]
    fn parses_fields_and_strings() {
        let t = parse(&buffer(Some(r"\Device\HarddiskVolume3\x.exe"), Some("x.exe -a"))).unwrap();
        assert_eq!(t.pid, 1234);
        assert_eq!(t.start_key, 0x007c_0000_0000_33ee);
        assert_eq!(t.boot_id, 124);
        assert_eq!(t.image_path.as_deref(), Some(r"\Device\HarddiskVolume3\x.exe"));
        assert_eq!(t.command_line.as_deref(), Some("x.exe -a"));
    }

    #[test]
    fn missing_strings_and_short_buffers() {
        let t = parse(&buffer(None, None)).unwrap();
        assert_eq!((t.image_path, t.command_line), (None, None));
        assert_eq!(parse(&buffer(None, None)[..FIXED - 1]), None);
    }

    #[test]
    fn reads_our_own_process() {
        let me = current().expect("own telemetry");
        assert_eq!(me.pid, std::process::id());
        assert_eq!(me.start_key >> 48, u64::from(me.boot_id) & 0xFFFF, "start key = BootId << 48 | sequence");
        let exe = me.image_path.expect("image path");
        assert!(exe.starts_with(r"\Device\"), "NT path: {exe}");
        assert_eq!(query(std::process::id()).map(|t| t.start_key), Some(me.start_key));
    }
}
```

`src/win/identity.rs`:
```rust
//! Device and boot identity, the clock anchor, and the other start-up facts
//! the pipeline's `Setup` needs (sensor spec §3.3, §5.5, §6.1).

use std::fmt;
use std::io::Write;
use std::path::Path;

use atlas_schema::{BootId, DeviceUid};
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::core::w;

use super::telemetry;
use super::util::{aligned, as_bytes, filetime_to_unix_ns, qpc_frequency, qpc_now, u32_at, u64_at};
use crate::config::Ticks;
use crate::process::Identity;
use crate::time::Anchor;

/// What the agent learns about itself and the machine at start.
#[derive(Debug, Clone)]
pub struct Started {
    pub identity: Identity,
    pub ticks: Ticks,
    pub anchor: Anchor,
    /// `HKLM\SYSTEM\Select\Current` (§5.5).
    pub current_control_set: u32,
    /// The agent's own start key (§5.5 self-filter).
    pub self_key: u64,
    /// QPC at start.
    pub started: i64,
    /// `KUSER_SHARED_DATA.BootId` when it disagreed with the kernel's
    /// telemetry value, which is then used (§6.1). The agent logs it.
    pub boot_id_disagreement: Option<u32>,
}

#[derive(Debug)]
pub enum IdentityError {
    /// `device.json` could not be read or created, or is not valid.
    DeviceFile(String),
    /// The agent's own `PROCESS_TELEMETRY_ID_INFORMATION`.
    Telemetry,
    /// The System process's creation time.
    BootTime,
    /// `HKLM\SYSTEM\Select\Current`.
    ControlSet,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityError::DeviceFile(e) => write!(f, "device identity file: {e}"),
            IdentityError::Telemetry => f.write_str("the agent's own process telemetry could not be read"),
            IdentityError::BootTime => f.write_str("the System process's creation time could not be read"),
            IdentityError::ControlSet => f.write_str(r"HKLM\SYSTEM\Select\Current could not be read"),
        }
    }
}

impl std::error::Error for IdentityError {}

/// Gathers the start-up facts. `device_file` is `device.json` (§6.1); the
/// install location and its permissions are plan 1b-4's.
pub fn start(device_file: &Path) -> Result<Started, IdentityError> {
    let device = device_uid(device_file)?;
    let me = telemetry::current().ok_or(IdentityError::Telemetry)?;
    let kusd = kusd_boot_id();
    // The two are the same kernel value; on a disagreement trust telemetry (§6.1).
    let disagreement = (kusd != me.boot_id).then_some(kusd);
    let boot_time = boot_time().ok_or(IdentityError::BootTime)?;
    let identity = Identity { device, boot: boot_id(me.boot_id, boot_time), kernel_boot_id: me.boot_id as u16 };
    Ok(Started {
        identity,
        ticks: ticks(),
        anchor: anchor(),
        current_control_set: current_control_set().ok_or(IdentityError::ControlSet)?,
        self_key: me.start_key,
        started: qpc_now(),
        boot_id_disagreement: disagreement,
    })
}

pub fn ticks() -> Ticks {
    Ticks::new(qpc_frequency())
}

/// A (QPC, wall clock) pair (§3.3); re-taken every 60 s by the driver.
pub fn anchor() -> Anchor {
    let before = qpc_now();
    // SAFETY: no arguments; returns the current time.
    let ft = unsafe { GetSystemTimePreciseAsFileTime() };
    let after = qpc_now();
    let ft = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
    Anchor { qpc: before + (after - before) / 2, unix_ns: filetime_to_unix_ns(ft) }
}

/// `device.boot_id` = `BLAKE3("atlas.boot.v1" ‖ BootId[u32 LE] ‖ boot_time[u64 LE])[0..16]`
/// (0a §4.4, §6.1).
pub fn boot_id(kernel_boot_id: u32, boot_time: u64) -> BootId {
    let mut h = blake3::Hasher::new();
    h.update(b"atlas.boot.v1");
    h.update(&kernel_boot_id.to_le_bytes());
    h.update(&boot_time.to_le_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&h.finalize().as_bytes()[..16]);
    BootId::from_bytes(out)
}

/// The persisted `device.uid`: read from `path`, or generated and written once
/// (§6.1). A file that exists but is not valid is an error, never replaced: a
/// new uid would split the device's history.
pub fn device_uid(path: &Path) -> Result<DeviceUid, IdentityError> {
    let err = |e: &dyn fmt::Display| IdentityError::DeviceFile(format!("{}: {e}", path.display()));
    if !path.exists() {
        let uid = DeviceUid::from_bytes(*uuid::Uuid::new_v4().as_bytes());
        // Write a temporary file, then link it into place: the link fails if
        // another process created the file first, and nobody reads a partial file.
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        let written = (|| {
            let mut f = std::fs::File::create_new(&tmp)?;
            f.write_all(serde_json::json!({ "device_uid": uid.to_string() }).to_string().as_bytes())?;
            f.sync_all()
        })();
        let linked = written.and_then(|()| std::fs::hard_link(&tmp, path));
        let _ = std::fs::remove_file(&tmp);
        match linked {
            Ok(()) => return Ok(uid),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(err(&e)),
        }
    }
    let text = std::fs::read_to_string(path).map_err(|e| err(&e))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| err(&e))?;
    let hex = v.get("device_uid").and_then(|h| h.as_str()).ok_or_else(|| err(&"no device_uid"))?;
    parse_uid(hex).ok_or_else(|| err(&"device_uid is not 32 lowercase hex digits"))
}

fn parse_uid(hex: &str) -> Option<DeviceUid> {
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(DeviceUid::from_bytes(out))
}

/// `KUSER_SHARED_DATA.BootId`: user-mode address `0x7FFE0000 + 0x2c4`, an offset
/// from public symbols on builds 26200 and 26300 (§6.1).
fn kusd_boot_id() -> u32 {
    const BOOT_ID: usize = 0x7FFE_0000 + 0x2c4;
    // SAFETY: KUSER_SHARED_DATA is mapped read-only at this fixed address in
    // every process; the field is a 4-byte-aligned u32 inside its first page.
    unsafe { (BOOT_ID as *const u32).read_volatile() }
}

/// The System process's creation time (FILETIME), §6.1. Read from
/// `SystemProcessInformation`, the same kernel field `GetProcessTimes` returns,
/// without needing a handle to PID 4 (an unelevated caller cannot open it).
fn boot_time() -> Option<u64> {
    const SYSTEM_PROCESS_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(5);
    // SYSTEM_PROCESS_INFORMATION (x64): NextEntryOffset @0, CreateTime @32, UniqueProcessId @80.
    let mut buf = aligned(1 << 20);
    loop {
        let mut ret = 0u32;
        let len = (buf.len() * 8) as u32;
        // SAFETY: `buf` is writable for `len` bytes and 8-aligned.
        let st =
            unsafe { NtQuerySystemInformation(SYSTEM_PROCESS_INFORMATION, buf.as_mut_ptr().cast(), len, &mut ret) };
        if st.is_err() {
            if ret > len && ret < 1 << 28 {
                buf = aligned(ret as usize + (1 << 16));
                continue;
            }
            return None;
        }
        let b = as_bytes(&buf);
        let mut off = 0usize;
        loop {
            if u64_at(b, off + 80)? == 4 {
                return u64_at(b, off + 32).filter(|t| *t != 0);
            }
            match u32_at(b, off)? {
                0 => return None,
                next => off += next as usize,
            }
        }
    }
}

fn current_control_set() -> Option<u32> {
    let mut v = 0u32;
    let mut size = 4u32;
    // SAFETY: writes at most `size` bytes into `v`.
    let st = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SYSTEM\Select"),
            w!("Current"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut v as *mut u32).cast()),
            Some(&mut size),
        )
    };
    st.is_ok().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_id_formula() {
        // BLAKE3("atlas.boot.v1" ‖ 7u32 LE ‖ 9u64 LE), first 16 bytes.
        let mut h = blake3::Hasher::new();
        h.update(b"atlas.boot.v1\x07\x00\x00\x00\x09\x00\x00\x00\x00\x00\x00\x00");
        assert_eq!(boot_id(7, 9).as_bytes()[..], h.finalize().as_bytes()[..16]);
        assert_ne!(boot_id(7, 9), boot_id(8, 9));
        assert_ne!(boot_id(7, 9), boot_id(7, 10));
    }

    #[test]
    fn device_uid_is_created_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("atlas-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("device.json");
        let a = device_uid(&path).unwrap();
        assert_eq!(device_uid(&path).unwrap(), a);
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("device.json")]);

        // A damaged file is an error, and stays as it was.
        std::fs::write(&path, r#"{"device_uid":"xyz"}"#).unwrap();
        assert!(matches!(device_uid(&path), Err(IdentityError::DeviceFile(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"device_uid":"xyz"}"#);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn uid_parsing() {
        assert!(parse_uid("000102030405060708090a0b0c0d0e0f").is_some());
        assert!(parse_uid("000102030405060708090A0B0C0D0E0F").is_none());
        assert!(parse_uid("0001").is_none());
    }

    #[test]
    fn start_reads_this_machine() {
        let dir = std::env::temp_dir().join(format!("atlas-start-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = start(&dir.join("device.json")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(s.self_key >> 48, u64::from(s.identity.kernel_boot_id), "own start key carries the BootId");
        assert_eq!(s.boot_id_disagreement, None, "KUSER_SHARED_DATA.BootId offset");
        assert!(s.current_control_set >= 1);
        assert!(s.ticks.frequency > 0 && s.started >= s.anchor.qpc);
        let now_ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as i64;
        assert!((now_ns - s.anchor.unix_ns).abs() < 5_000_000_000, "anchor is the wall clock");
        let bt = boot_time().unwrap();
        assert!(filetime_to_unix_ns(bt) < now_ns, "booted in the past");
        assert_eq!(s.identity.boot, boot_id(u32::from(s.identity.kernel_boot_id), bt));
    }

    /// The System process's creation time from `SystemProcessInformation`
    /// equals `GetProcessTimes` on PID 4, which needs an elevated caller.
    #[test]
    #[ignore = "opening PID 4 needs an elevated run"]
    fn boot_time_matches_get_process_times() {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let h = super::super::util::Owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, 4) }.unwrap());
        let (mut c, mut e, mut k, mut u) =
            (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: test-only; four writable FILETIMEs.
        unsafe { GetProcessTimes(h.raw(), &mut c, &mut e, &mut k, &mut u) }.unwrap();
        let created = (u64::from(c.dwHighDateTime) << 32) | u64::from(c.dwLowDateTime);
        assert_eq!(boot_time(), Some(created));
    }
}
```

- [ ] **Step 5: Lookups**

`src/win/lookups.rs`:
```rust
//! [`Lookups`] on Windows: the device map, live processes and account names
//! (sensor spec §5.3, §5.5). They run on the pipeline thread, so each is cheap:
//! the device map and account names are cached, and an account lookup never
//! waits more than [`ACCOUNT_TIMEOUT`] (plan 1b-3b, D5).

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows::Win32::Security::{LookupAccountSidW, PSID, SID_NAME_USE};
use windows::Win32::Storage::FileSystem::QueryDosDeviceW;
use windows::core::{PCWSTR, PWSTR};

use super::telemetry;
use super::util::wide;
use crate::services::{LiveProcess, Lookups};

/// Full rebuild interval of the device map (§5.5).
pub const DEVICE_REFRESH: Duration = Duration::from_secs(60);
/// A lookup miss rebuilds the map at most this often (§5.5).
pub const DEVICE_MISS_REFRESH: Duration = Duration::from_secs(5);
/// How long the pipeline thread waits for an account name (plan 1b-3b, D5).
pub const ACCOUNT_TIMEOUT: Duration = Duration::from_millis(100);
/// Account cache bound; it is cleared when full (SIDs seen are few).
const ACCOUNT_CAP: usize = 4096;

/// NT device prefix → drive, rebuilt on a timer and on a miss.
pub struct DeviceMap<F> {
    load: F,
    /// (device, drive), longest device first.
    entries: Vec<(String, String)>,
    loaded: Instant,
    missed: Option<Instant>,
}

impl<F: FnMut() -> Vec<(String, String)>> DeviceMap<F> {
    pub fn new(mut load: F, now: Instant) -> Self {
        let entries = sorted(load());
        DeviceMap { load, entries, loaded: now, missed: None }
    }

    /// The drive form of `nt`, or `None` to keep the NT path.
    pub fn dos_path(&mut self, nt: &str, now: Instant) -> Option<String> {
        if now.duration_since(self.loaded) >= DEVICE_REFRESH {
            self.reload(now);
        }
        if let Some(p) = self.find(nt) {
            return Some(p);
        }
        if self.missed.is_none_or(|m| now.duration_since(m) >= DEVICE_MISS_REFRESH) {
            self.missed = Some(now);
            self.reload(now);
            return self.find(nt);
        }
        None
    }

    fn reload(&mut self, now: Instant) {
        self.entries = sorted((self.load)());
        self.loaded = now;
    }

    /// Prefix match on a component boundary, ignoring case (§5.5).
    fn find(&self, nt: &str) -> Option<String> {
        self.entries.iter().find_map(|(dev, drive)| {
            let head = nt.get(..dev.len())?;
            let rest = &nt[dev.len()..];
            (head.eq_ignore_ascii_case(dev) && (rest.is_empty() || rest.starts_with('\\')))
                .then(|| format!("{drive}{rest}"))
        })
    }
}

fn sorted(mut v: Vec<(String, String)>) -> Vec<(String, String)> {
    v.sort_by_key(|(dev, _)| std::cmp::Reverse(dev.len()));
    v
}

/// `QueryDosDeviceW` for every drive letter. Only `\Device\…` targets are kept:
/// a `subst` drive's target is itself a drive path.
pub fn query_drives() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut buf = vec![0u16; 1024];
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:", letter as char);
        let name = wide(&drive);
        // SAFETY: `name` is NUL-terminated; `buf` is writable.
        let n = unsafe { QueryDosDeviceW(PCWSTR(name.as_ptr()), Some(&mut buf)) } as usize;
        let first = buf[..n.min(buf.len())].split(|&u| u == 0).next().unwrap_or(&[]);
        let target = String::from_utf16_lossy(first);
        if n > 0 && target.starts_with(r"\Device\") {
            out.push((target, drive));
        }
    }
    out
}

/// `DOMAIN\name` per SID string, resolved on a helper thread.
pub struct Accounts {
    cache: HashMap<String, Option<String>>,
    asked: HashSet<String>,
    tx: Sender<String>,
    rx: Receiver<(String, Option<String>)>,
    timeout: Duration,
}

impl Accounts {
    /// `resolve` runs on the helper thread.
    pub fn new(resolve: impl Fn(&str) -> Option<String> + Send + 'static, timeout: Duration) -> Self {
        let (tx, jobs) = channel::<String>();
        let (done, rx) = channel();
        std::thread::Builder::new()
            .name("atlas-accounts".into())
            .spawn(move || {
                for sid in jobs {
                    let name = resolve(&sid);
                    if done.send((sid, name)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn the account lookup thread");
        Accounts { cache: HashMap::new(), asked: HashSet::new(), tx, rx, timeout }
    }

    /// The cached name, or a lookup waited for up to the timeout. A slow
    /// lookup finishes in the background and serves later calls.
    pub fn name(&mut self, sid: &str) -> Option<String> {
        while let Ok((s, n)) = self.rx.try_recv() {
            self.store(s, n);
        }
        if let Some(n) = self.cache.get(sid) {
            return n.clone();
        }
        // Already asked and still running: don't wait a second time.
        if !self.asked.insert(sid.to_string()) || self.tx.send(sid.to_string()).is_err() {
            return None;
        }
        let until = Instant::now() + self.timeout;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok((s, n)) => {
                    let mine = s == sid;
                    self.store(s, n.clone());
                    if mine {
                        return n;
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    fn store(&mut self, sid: String, name: Option<String>) {
        if self.cache.len() >= ACCOUNT_CAP {
            self.cache.clear();
        }
        self.asked.remove(&sid);
        self.cache.insert(sid, name);
    }
}

/// `LookupAccountSidW` on the local machine: `DOMAIN\name`, or `name` when
/// the domain is empty (e.g. `Everyone`).
pub fn lookup_account(sid: &str) -> Option<String> {
    let s = wide(sid);
    let mut psid = PSID::default();
    // SAFETY: `s` is NUL-terminated; the SID is freed with LocalFree below.
    unsafe { ConvertStringSidToSidW(PCWSTR(s.as_ptr()), &mut psid) }.ok()?;
    let mut name = vec![0u16; 256];
    let mut domain = vec![0u16; 256];
    let (mut n, mut d) = (name.len() as u32, domain.len() as u32);
    let mut use_ = SID_NAME_USE::default();
    // SAFETY: the buffers are writable for the lengths given.
    let r = unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            psid,
            Some(PWSTR(name.as_mut_ptr())),
            &mut n,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut d,
            &mut use_,
        )
    };
    // SAFETY: allocated by ConvertStringSidToSidW.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(psid.0)));
    }
    r.ok()?;
    let name = String::from_utf16_lossy(&name[..n as usize]);
    let domain = String::from_utf16_lossy(&domain[..d as usize]);
    Some(if domain.is_empty() { name } else { format!(r"{domain}\{name}") })
}

/// Loads the (device, drive) pairs.
type Drives = fn() -> Vec<(String, String)>;

/// The Windows [`Lookups`].
pub struct WinLookups {
    devices: DeviceMap<Drives>,
    accounts: Accounts,
}

impl WinLookups {
    pub fn new() -> Self {
        WinLookups {
            devices: DeviceMap::new(query_drives as Drives, Instant::now()),
            accounts: Accounts::new(lookup_account, ACCOUNT_TIMEOUT),
        }
    }
}

impl Default for WinLookups {
    fn default() -> Self {
        Self::new()
    }
}

impl Lookups for WinLookups {
    fn dos_path(&mut self, nt_path: &str) -> Option<String> {
        self.devices.dos_path(nt_path, Instant::now())
    }

    /// Never cached: PIDs are reused (1b-3a Interfaces).
    fn live_process(&mut self, pid: u32) -> Option<LiveProcess> {
        let t = telemetry::query(pid)?;
        Some(LiveProcess { start_key: t.start_key, image_path: t.image_path?, command_line: t.command_line })
    }

    fn account_name(&mut self, sid: &str) -> Option<String> {
        self.accounts.name(sid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    fn counting(entries: Vec<(&str, &str)>) -> (Rc<Cell<u32>>, impl FnMut() -> Vec<(String, String)>) {
        let loads = Rc::new(Cell::new(0));
        let l = loads.clone();
        let e: Vec<(String, String)> = entries.into_iter().map(|(a, b)| (a.into(), b.into())).collect();
        (loads, move || {
            l.set(l.get() + 1);
            e.clone()
        })
    }

    #[test]
    fn device_map_matches_on_component_boundaries() {
        let t = Instant::now();
        let (_, load) = counting(vec![(r"\Device\HarddiskVolume1", "C:"), (r"\Device\HarddiskVolume10", "D:")]);
        let mut m = DeviceMap::new(load, t);
        assert_eq!(m.dos_path(r"\Device\HarddiskVolume1\x", t).as_deref(), Some(r"C:\x"));
        assert_eq!(m.dos_path(r"\device\harddiskvolume10\x", t).as_deref(), Some(r"D:\x"));
        assert_eq!(m.dos_path(r"\Device\HarddiskVolume1", t).as_deref(), Some("C:"));
        assert_eq!(m.dos_path(r"\Device\HarddiskVolume12\x", t), None);
    }

    #[test]
    fn device_map_refreshes_on_a_timer_and_on_misses() {
        let t = Instant::now();
        let (loads, load) = counting(vec![(r"\Device\HarddiskVolume1", "C:")]);
        let mut m = DeviceMap::new(load, t);
        assert_eq!(loads.get(), 1);
        m.dos_path(r"\Device\HarddiskVolume1\x", t + Duration::from_secs(59));
        assert_eq!(loads.get(), 1, "a hit before 60 s does not reload");
        m.dos_path(r"\Device\HarddiskVolume1\x", t + Duration::from_secs(60));
        assert_eq!(loads.get(), 2, "reloaded after 60 s");
        let t = t + Duration::from_secs(60);
        m.dos_path(r"\Device\Mup\x", t);
        assert_eq!(loads.get(), 3, "first miss reloads");
        m.dos_path(r"\Device\Mup\x", t + Duration::from_secs(4));
        assert_eq!(loads.get(), 3, "misses reload at most every 5 s");
        m.dos_path(r"\Device\Mup\x", t + Duration::from_secs(5));
        assert_eq!(loads.get(), 4);
    }

    #[test]
    fn device_map_picks_up_a_new_drive_on_a_miss() {
        let t = Instant::now();
        let drives = Rc::new(Cell::new(1));
        let d = drives.clone();
        let load = move || {
            let mut v = vec![(r"\Device\HarddiskVolume1".to_string(), "C:".to_string())];
            if d.get() > 1 {
                v.push((r"\Device\HarddiskVolume7".to_string(), "E:".to_string()));
            }
            v
        };
        let mut m = DeviceMap::new(load, t);
        drives.set(2);
        assert_eq!(m.dos_path(r"\Device\HarddiskVolume7\f", t).as_deref(), Some(r"E:\f"));
    }

    #[test]
    fn this_machine_has_a_system_drive() {
        let windir = std::env::var("SystemRoot").unwrap();
        let drive = &windir[..2];
        assert!(query_drives().iter().any(|(dev, d)| d.eq_ignore_ascii_case(drive) && dev.starts_with(r"\Device\")));
    }

    #[test]
    fn accounts_are_cached_and_never_wait_long() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let c = calls.clone();
        let mut a = Accounts::new(
            move |sid| {
                c.lock().unwrap().push(sid.to_string());
                if sid == "slow" {
                    std::thread::sleep(Duration::from_millis(300));
                }
                (sid != "none").then(|| format!("D\\{sid}"))
            },
            Duration::from_millis(100),
        );
        assert_eq!(a.name("S-1").as_deref(), Some(r"D\S-1"));
        assert_eq!(a.name("S-1").as_deref(), Some(r"D\S-1"));
        assert_eq!(a.name("none"), None);
        assert_eq!(a.name("none"), None, "failures are cached too");
        let t = Instant::now();
        assert_eq!(a.name("slow"), None, "a slow lookup times out");
        assert!(t.elapsed() < Duration::from_millis(250));
        let t = Instant::now();
        assert_eq!(a.name("slow"), None, "still running: not asked twice");
        assert!(t.elapsed() < Duration::from_millis(50), "and not waited for twice");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(a.name("slow").as_deref(), Some(r"D\slow"), "finished in the background");
        assert_eq!(*calls.lock().unwrap(), ["S-1", "none", "slow"]);
    }

    #[test]
    fn well_known_accounts() {
        assert_eq!(lookup_account("S-1-5-18").as_deref(), Some(r"NT AUTHORITY\SYSTEM"));
        assert_eq!(lookup_account("S-1-1-0").as_deref(), Some("Everyone"));
        assert_eq!(lookup_account("not a sid"), None);
    }

    #[test]
    fn live_process_describes_our_own() {
        let mut l = WinLookups::new();
        let me = l.live_process(std::process::id()).expect("own process");
        assert!(me.image_path.ends_with(".exe"), "{}", me.image_path);
        assert!(me.command_line.is_some());
        assert!(l.dos_path(&me.image_path).is_some_and(|p| p.as_bytes()[1] == b':'));
    }
}
```

- [ ] **Step 6: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy -p atlas-agent --all-targets -- -D warnings -A dead-code
cargo clippy -p atlas-agent --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
```
Expected: 110 tests pass and 1 is ignored (`boot_time_matches_get_process_times`, elevated); both clippy runs are clean. (`dead_code` is allowed until Task 6 calls the new modules.)
```powershell
git add Cargo.toml Cargo.lock crates/atlas-agent
git commit -m "feat(agent): Windows identity and lookups: boot id, device uid, device map, telemetry, accounts"
```

### Task 3: Hashes and signatures

**Files:**
- Modify: `crates/atlas-agent/src/win/mod.rs` (`mod hash;`)
- Create: `crates/atlas-agent/src/win/hash.rs`

- [ ] **Step 1: The enricher**

`src/win/hash.rs`:
```rust
//! SHA-256 and Authenticode for Launch and Module Load images (sensor spec §6.3).
//!
//! One handle per file serves the cache key (volume serial, 128-bit file id,
//! USN), the bytes hashed and the signature check, so the three cannot come
//! from different files (plan 1b-3b, D5).

use std::collections::HashMap;
use std::sync::Mutex;

use atlas_schema::{Hashes, Signature, SignatureStatus};
use sha2::{Digest, Sha256};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Security::Cryptography::Catalog::{
    CATALOG_INFO, CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2, CryptCATAdminEnumCatalogFromHash,
    CryptCATAdminReleaseCatalogContext, CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext,
};
use windows::Win32::Security::Cryptography::{CERT_NAME_ATTR_TYPE, CertGetNameStringW, szOID_COMMON_NAME};
use windows::Win32::Security::WinTrust::{
    DRIVER_ACTION_VERIFY, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_CATALOG_INFO, WINTRUST_DATA, WINTRUST_FILE_INFO,
    WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE,
    WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
    WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_BEGIN, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_SEQUENTIAL_SCAN, FILE_GENERIC_READ, FILE_ID_INFO,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandleEx, GetFileSizeEx,
    OPEN_EXISTING, ReadFile, SetFilePointerEx,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{FSCTL_READ_FILE_USN_DATA, READ_FILE_USN_DATA};
use windows::core::{GUID, PCWSTR, w};

use super::util::{Owned, aligned, as_bytes, u32_at, u64_at, wide};

/// Cache key: which file, in which version (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FileKey {
    volume: u64,
    id: [u8; 16],
    usn: i64,
}

/// One file's result, as `Reply::Enriched` carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Enriched {
    pub hashes: Option<Hashes>,
    pub signature: Option<Signature>,
    /// An operational error: the signature (and maybe the hash) is absent (§6.3).
    pub error: bool,
}

impl Enriched {
    fn failed() -> Self {
        Enriched { hashes: None, signature: None, error: true }
    }
}

/// Results by file version, shared by the two workers. Errors are not cached.
pub(crate) struct HashCache {
    map: HashMap<FileKey, Enriched>,
    /// Lowercased NT path → the keys stored under it, for `InvalidateHash`.
    by_path: HashMap<String, Vec<FileKey>>,
    cap: usize,
    pub evictions: u64,
}

impl HashCache {
    pub(crate) fn new(cap: usize) -> Self {
        HashCache { map: HashMap::new(), by_path: HashMap::new(), cap, evictions: 0 }
    }

    fn get(&self, key: &FileKey) -> Option<Enriched> {
        self.map.get(key).cloned()
    }

    fn insert(&mut self, nt_path: &str, key: FileKey, value: Enriched) {
        if self.map.len() >= self.cap {
            // Rare and cheap to rebuild: start over rather than track recency.
            self.evictions += self.map.len() as u64;
            self.map.clear();
            self.by_path.clear();
        }
        self.map.insert(key, value);
        self.by_path.entry(nt_path.to_lowercase()).or_default().push(key);
    }

    /// Forgets results for a path that changed (§6.3). The USN in the key
    /// already separates versions; this keeps the cache from holding them.
    pub(crate) fn invalidate(&mut self, nt_path: &str) {
        for key in self.by_path.remove(&nt_path.to_lowercase()).unwrap_or_default() {
            self.map.remove(&key);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }
}

/// What a signature check concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Valid,
    /// No embedded signature: try the catalogs.
    NotEmbedded,
    Invalid,
    /// An operational error: signature absent (§6.3).
    Error,
}

const TRUST_E_PROVIDER_UNKNOWN: u32 = 0x800B_0001;
const TRUST_E_ACTION_UNKNOWN: u32 = 0x800B_0002;
const TRUST_E_SUBJECT_FORM_UNKNOWN: u32 = 0x800B_0003;
const TRUST_E_NOSIGNATURE: u32 = 0x800B_0100;
const TRUST_E_SYSTEM_ERROR: u32 = 0x8009_6001;
const CRYPT_E_REVOKED: u32 = 0x8009_2010;
const CRYPT_E_SECURITY_SETTINGS: u32 = 0x8009_2026;

/// `WinVerifyTrust`'s result → verdict (§6.3). Failures of the signature or
/// its chain are `Invalid`: the certificate facility (`0x800B….`: untrusted
/// root, expired, revoked, explicit distrust, chaining), the trust errors
/// `0x80096002`–`0x800960FF` (bad digest, no signer certificate, bad
/// certificate signature), revocation, admin policy, and malformed ASN.1
/// (`0x80093xxx`). Anything else, such as a file or RPC error, is `Error`.
fn classify(hr: i32) -> Verdict {
    let hr = hr as u32;
    match hr {
        0 => Verdict::Valid,
        TRUST_E_NOSIGNATURE | TRUST_E_SUBJECT_FORM_UNKNOWN => Verdict::NotEmbedded,
        TRUST_E_PROVIDER_UNKNOWN | TRUST_E_ACTION_UNKNOWN | TRUST_E_SYSTEM_ERROR => Verdict::Error,
        CRYPT_E_REVOKED | CRYPT_E_SECURITY_SETTINGS => Verdict::Invalid,
        _ if hr >> 16 == 0x800B => Verdict::Invalid,
        0x8009_6002..=0x8009_60FF => Verdict::Invalid,
        0x8009_3000..=0x8009_31FF => Verdict::Invalid,
        _ => Verdict::Error,
    }
}

/// Hashes and checks files; one per worker thread (the catalog context is per thread).
pub(crate) struct Enricher {
    cat_admin: Option<isize>,
    size_cap: u64,
}

impl Enricher {
    pub(crate) fn new(size_cap: u64) -> Self {
        let mut admin = 0isize;
        // SAFETY: writes the context handle; released in Drop.
        let ok =
            unsafe { CryptCATAdminAcquireContext2(&mut admin, Some(&DRIVER_ACTION_VERIFY), w!("SHA256"), None, None) }
                .is_ok();
        Enricher { cat_admin: ok.then_some(admin), size_cap }
    }

    /// Hash and signature of the file at `nt_path`, from the cache when this
    /// version of the file was seen before.
    pub(crate) fn enrich(&self, nt_path: &str, cache: &Mutex<HashCache>) -> Enriched {
        let path = format!(r"\\?\GLOBALROOT{nt_path}");
        let Some(file) = open(&path) else { return Enriched::failed() };
        let key = file_key(file.raw());
        if let Some(hit) = key.and_then(|k| cache.lock().expect("hash cache").get(&k)) {
            return hit;
        }
        let Some(size) = size(file.raw()) else { return Enriched::failed() };
        let hashes = if size <= self.size_cap {
            match sha256(file.raw()) {
                Some(h) => Some(Hashes { sha256: Some(h) }),
                None => return Enriched::failed(),
            }
        } else {
            None
        };
        let (signature, error) = match self.signature(&path, file.raw()) {
            Some(s) => (Some(s), false),
            None => (None, true),
        };
        let out = Enriched { hashes, signature, error };
        if let Some(k) = key
            && !error
        {
            cache.lock().expect("hash cache").insert(nt_path, k, out.clone());
        }
        out
    }

    /// Embedded signature first, then the catalogs; `None` on an operational error.
    fn signature(&self, path: &str, file: HANDLE) -> Option<Signature> {
        let path_w = wide(path);
        let mut info = WINTRUST_FILE_INFO {
            cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(path_w.as_ptr()),
            hFile: file,
            ..Default::default()
        };
        let mut data = trust_data();
        data.dwUnionChoice = WTD_CHOICE_FILE;
        data.Anonymous.pFile = &mut info;
        let (hr, signer) = verify(&mut data);
        match classify(hr) {
            Verdict::Valid => Some(Signature { signer, status: SignatureStatus::Valid }),
            Verdict::Invalid => Some(Signature { signer, status: SignatureStatus::Invalid }),
            Verdict::Error => None,
            Verdict::NotEmbedded => self.catalog(&path_w, file),
        }
    }

    fn catalog(&self, path_w: &[u16], file: HANDLE) -> Option<Signature> {
        let unsigned = Some(Signature { signer: None, status: SignatureStatus::Unsigned });
        let admin = self.cat_admin?;
        rewind(file)?;
        let mut len = 0u32;
        // SAFETY: a size query (no buffer); fails with a size when the buffer is too small.
        let _ = unsafe { CryptCATAdminCalcHashFromFileHandle2(admin, file, &mut len, None, None) };
        if len == 0 || len > 64 {
            return None;
        }
        let mut hash = vec![0u8; len as usize];
        // SAFETY: `hash` is writable for `len` bytes.
        unsafe { CryptCATAdminCalcHashFromFileHandle2(admin, file, &mut len, Some(hash.as_mut_ptr()), None) }.ok()?;
        // SAFETY: `hash` is valid; the returned context is released below.
        let cat = unsafe { CryptCATAdminEnumCatalogFromHash(admin, &hash, None, None) };
        if cat == 0 {
            return unsigned;
        }
        let mut ci = CATALOG_INFO { cbStruct: size_of::<CATALOG_INFO>() as u32, ..Default::default() };
        // SAFETY: `cat` is a live catalog context.
        let out = match unsafe { CryptCATCatalogInfoFromContext(cat, &mut ci, 0) } {
            Err(_) => None,
            Ok(()) => {
                let tag = wide(&hash.iter().map(|b| format!("{b:02X}")).collect::<String>());
                let mut member = WINTRUST_CATALOG_INFO {
                    cbStruct: size_of::<WINTRUST_CATALOG_INFO>() as u32,
                    pcwszCatalogFilePath: PCWSTR(ci.wszCatalogFile.as_ptr()),
                    pcwszMemberTag: PCWSTR(tag.as_ptr()),
                    pcwszMemberFilePath: PCWSTR(path_w.as_ptr()),
                    hMemberFile: file,
                    pbCalculatedFileHash: hash.as_mut_ptr(),
                    cbCalculatedFileHash: len,
                    hCatAdmin: admin,
                    ..Default::default()
                };
                let mut data = trust_data();
                data.dwUnionChoice = WTD_CHOICE_CATALOG;
                data.Anonymous.pCatalog = &mut member;
                let (hr, signer) = verify(&mut data);
                match classify(hr) {
                    Verdict::Valid => Some(Signature { signer, status: SignatureStatus::Valid }),
                    Verdict::Invalid => Some(Signature { signer, status: SignatureStatus::Invalid }),
                    Verdict::NotEmbedded => unsigned,
                    Verdict::Error => None,
                }
            }
        };
        // SAFETY: releases the context enumerated above.
        unsafe {
            let _ = CryptCATAdminReleaseCatalogContext(admin, cat, 0);
        }
        out
    }
}

impl Drop for Enricher {
    fn drop(&mut self) {
        if let Some(admin) = self.cat_admin {
            // SAFETY: acquired in `new`.
            unsafe {
                let _ = CryptCATAdminReleaseContext(admin, 0);
            }
        }
    }
}

/// No UI, no revocation checks and no network: the sensor never fetches (§6.3).
fn trust_data() -> WINTRUST_DATA {
    WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE,
        ..Default::default()
    }
}

/// Runs the check and reads the leaf certificate's subject CN before closing
/// the state.
fn verify(data: &mut WINTRUST_DATA) -> (i32, Option<String>) {
    let mut action: GUID = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    // SAFETY: `data` and what it points to outlive both calls.
    let hr = unsafe { WinVerifyTrust(HWND::default(), &mut action, (data as *mut WINTRUST_DATA).cast()) };
    let signer = signer_cn(data.hWVTStateData);
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: as above; closes the state opened by the first call.
    unsafe { WinVerifyTrust(HWND::default(), &mut action, (data as *mut WINTRUST_DATA).cast()) };
    (hr, signer)
}

fn signer_cn(state: HANDLE) -> Option<String> {
    if state.is_invalid() || state.0.is_null() {
        return None;
    }
    // SAFETY: `state` is open until WTD_STATEACTION_CLOSE; every pointer read
    // here belongs to it and is checked for null and count first.
    unsafe {
        let prov = WTHelperProvDataFromStateData(state);
        if prov.is_null() {
            return None;
        }
        let sgnr = WTHelperGetProvSignerFromChain(prov, 0, false, 0);
        if sgnr.is_null() || (*sgnr).csCertChain == 0 || (*sgnr).pasCertChain.is_null() {
            return None;
        }
        let cert = (*(*sgnr).pasCertChain).pCert;
        if cert.is_null() {
            return None;
        }
        let mut buf = [0u16; 256];
        let n = CertGetNameStringW(cert, CERT_NAME_ATTR_TYPE, 0, Some(szOID_COMMON_NAME.0.cast()), Some(&mut buf));
        let s = String::from_utf16_lossy(&buf[..(n as usize).saturating_sub(1).min(buf.len())]);
        (!s.is_empty()).then_some(s)
    }
}

/// Opens for reading with full sharing, so the agent never blocks the file's
/// users (§6.3). Backup semantics: SYSTEM with `SeBackupPrivilege` can read
/// any file.
fn open(path: &str) -> Option<Owned> {
    let p = wide(path);
    // SAFETY: `p` is NUL-terminated; the handle is owned by `Owned`.
    let h = unsafe {
        CreateFileW(
            PCWSTR(p.as_ptr()),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_SEQUENTIAL_SCAN,
            None,
        )
    };
    h.ok().map(Owned)
}

/// The cache key, or `None` when the volume keeps no USNs (not cached, §6.3).
fn file_key(h: HANDLE) -> Option<FileKey> {
    let mut id = FILE_ID_INFO::default();
    // SAFETY: `id` is writable for its size.
    unsafe {
        GetFileInformationByHandleEx(
            h,
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    }
    .ok()?;
    let usn = usn(h)?;
    Some(FileKey { volume: id.VolumeSerialNumber, id: id.FileId.Identifier, usn })
}

/// The file's USN from its USN_RECORD (V2 or V3). Zero means the journal has
/// not recorded the file: treated as no USN.
fn usn(h: HANDLE) -> Option<i64> {
    let input = READ_FILE_USN_DATA { MinMajorVersion: 2, MaxMajorVersion: 3 };
    let mut out = aligned(1024);
    let mut ret = 0u32;
    // SAFETY: input and output buffers are valid for the sizes given.
    unsafe {
        DeviceIoControl(
            h,
            FSCTL_READ_FILE_USN_DATA,
            Some((&input as *const READ_FILE_USN_DATA).cast()),
            size_of::<READ_FILE_USN_DATA>() as u32,
            Some(out.as_mut_ptr().cast()),
            (out.len() * 8) as u32,
            Some(&mut ret),
            None,
        )
    }
    .ok()?;
    let b = as_bytes(&out);
    let major = u32_at(b, 4)? & 0xFFFF;
    let off = match major {
        2 => 24,
        3 => 40,
        _ => return None,
    };
    Some(u64_at(b, off)? as i64).filter(|u| *u > 0)
}

fn size(h: HANDLE) -> Option<u64> {
    let mut n = 0i64;
    // SAFETY: writes one i64.
    unsafe { GetFileSizeEx(h, &mut n) }.ok()?;
    u64::try_from(n).ok()
}

fn rewind(h: HANDLE) -> Option<()> {
    // SAFETY: moves the file pointer of a synchronous handle.
    unsafe { SetFilePointerEx(h, 0, None, FILE_BEGIN) }.ok()
}

fn sha256(h: HANDLE) -> Option<[u8; 32]> {
    rewind(h)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let mut n = 0u32;
        // SAFETY: `buf` is writable; synchronous read.
        unsafe { ReadFile(h, Some(&mut buf), Some(&mut n), None) }.ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n as usize]);
    }
    Some(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nt(dos: &str) -> String {
        let devices = super::super::lookups::query_drives();
        let (dev, drive) = devices.iter().find(|(_, d)| dos[..2].eq_ignore_ascii_case(d)).expect("drive mapped");
        format!("{dev}{}", &dos[drive.len()..])
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("atlas-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn classification() {
        assert_eq!(classify(0), Verdict::Valid);
        for nosig in [0x800B_0100u32, 0x800B_0003] {
            assert_eq!(classify(nosig as i32), Verdict::NotEmbedded);
        }
        // Bad digest, untrusted root, revoked (cert and CRYPT_E), distrust, expired, bad ASN.1.
        for bad in [0x8009_6010u32, 0x800B_0109, 0x800B_010C, 0x8009_2010, 0x800B_0111, 0x800B_0101, 0x8009_310B] {
            assert_eq!(classify(bad as i32), Verdict::Invalid, "{bad:#x}");
        }
        // Sharing violation, RPC server unavailable, trust system error, unknown provider.
        for err in [0x8007_0020u32, 0x8007_06BA, 0x8009_6001, 0x800B_0001] {
            assert_eq!(classify(err as i32), Verdict::Error, "{err:#x}");
        }
    }

    #[test]
    fn hashes_a_file_and_caches_by_version() {
        let p = temp("hello.bin");
        std::fs::write(&p, b"hello").unwrap();
        let path = nt(&p.to_string_lossy());
        let cache = Mutex::new(HashCache::new(100));
        let e = Enricher::new(100 << 20);
        let r = e.enrich(&path, &cache);
        let want: [u8; 32] = Sha256::digest(b"hello").into();
        assert_eq!(r.hashes, Some(Hashes { sha256: Some(want) }));
        assert_eq!(r.signature, Some(Signature { signer: None, status: SignatureStatus::Unsigned }));
        assert!(!r.error);
        assert_eq!(cache.lock().unwrap().len(), 1, "cached under its USN");

        // A new version has a new USN: a miss, and the new content.
        std::fs::write(&p, b"world").unwrap();
        let r2 = e.enrich(&path, &cache);
        assert_eq!(r2.hashes.unwrap().sha256, Some(Sha256::digest(b"world").into()));
        assert_eq!(cache.lock().unwrap().len(), 2);
        cache.lock().unwrap().invalidate(&path.to_uppercase());
        assert_eq!(cache.lock().unwrap().len(), 0, "invalidation ignores case");
    }

    #[test]
    fn size_cap_skips_the_hash_only() {
        let p = temp("big.bin");
        std::fs::write(&p, vec![0u8; 4096]).unwrap();
        let r = Enricher::new(4095).enrich(&nt(&p.to_string_lossy()), &Mutex::new(HashCache::new(10)));
        assert_eq!(r.hashes, None);
        assert_eq!(r.signature.map(|s| s.status), Some(SignatureStatus::Unsigned));
    }

    #[test]
    fn missing_files_are_errors_and_not_cached() {
        let cache = Mutex::new(HashCache::new(10));
        let r = Enricher::new(1 << 20).enrich(&nt(&temp("absent.exe").to_string_lossy()), &cache);
        assert_eq!(r, Enriched::failed());
        assert_eq!(cache.lock().unwrap().len(), 0);
    }

    #[test]
    fn catalog_signed_os_file() {
        let notepad = format!(r"{}\System32\notepad.exe", std::env::var("SystemRoot").unwrap());
        let r = Enricher::new(100 << 20).enrich(&nt(&notepad), &Mutex::new(HashCache::new(10)));
        let s = r.signature.expect("signature");
        assert_eq!(s.status, SignatureStatus::Valid);
        assert_eq!(s.signer.as_deref(), Some("Microsoft Windows"));
    }

    /// A binary with an embedded signature, present on the host and the CI runner.
    fn embedded_signed() -> String {
        let root = std::env::var("SystemRoot").unwrap();
        let pf = std::env::var("ProgramFiles").unwrap();
        let candidates = [
            format!(r"{pf}\Windows Defender\MpCmdRun.exe"),
            format!(r"{root}\System32\SecurityHealthService.exe"),
            format!(r"{root}\System32\drivers\WdFilter.sys"),
            format!(r"{pf}\Git\cmd\git.exe"),
        ];
        candidates.into_iter().find(|c| std::path::Path::new(c).exists()).expect("an embedded-signed binary")
    }

    #[test]
    fn embedded_signature_and_tampering() {
        let e = Enricher::new(100 << 20);
        let cache = Mutex::new(HashCache::new(10));
        let original = embedded_signed();
        let r = e.enrich(&nt(&original), &cache);
        let s = r.signature.expect("signature");
        assert_eq!(s.status, SignatureStatus::Valid, "{original}");
        assert!(s.signer.is_some_and(|n| !n.is_empty()));

        // One flipped byte in the middle: the embedded digest no longer matches.
        let mut bytes = std::fs::read(&original).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xFF;
        let copy = temp("tampered.exe");
        std::fs::write(&copy, &bytes).unwrap();
        let r = e.enrich(&nt(&copy.to_string_lossy()), &cache);
        assert_eq!(r.signature.map(|s| s.status), Some(SignatureStatus::Invalid));
        assert!(r.hashes.is_some() && !r.error);
    }
}
```

- [ ] **Step 2: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy -p atlas-agent --all-targets -- -D warnings -A dead-code
```
Expected: 116 tests pass, 1 ignored, clippy clean. `embedded_signature_and_tampering` needs one of the embedded-signed binaries it lists (Defender's `MpCmdRun.exe` on the host and the runner); it fails, never skips, when none is present.
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): SHA-256 and Authenticode, cached by file version"
```

### Task 4: The reader lane's work: value reads and 8.3 expansion

**Files:**
- Modify: `crates/atlas-agent/src/win/mod.rs` (`mod expand;`, `mod value;`)
- Create: `crates/atlas-agent/src/win/value.rs`, `expand.rs`

- [ ] **Step 1: Value reads**

`src/win/value.rs` (its test helper `TestKey` is shared with later tests):
```rust
//! Registry value reads after a SetValueKey (sensor spec §7.5).
//!
//! The key is opened by its raw NT name with `OBJ_OPENLINK`, so a key swapped
//! for a symbolic link is not followed, and with `REG_OPTION_BACKUP_RESTORE`
//! when `SeBackupPrivilege` is enabled, which defeats a deny-SYSTEM DACL. The
//! value name is counted, so embedded NULs are kept.

use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::System::Registry::{KeyValuePartialInformation, NtOpenKeyEx, NtQueryValueKey};
use windows::Win32::Foundation::{HANDLE, OBJ_CASE_INSENSITIVE, OBJ_OPENLINK};
use windows::Win32::System::Registry::{KEY_QUERY_VALUE, REG_OPTION_BACKUP_RESTORE};

use super::util::{Owned, aligned, as_bytes, counted, u32_at};
use crate::services::{ValueData, ValueRead};

/// At most this much data is kept (§7.5).
pub(crate) const DATA_MAX: usize = 4096;
/// Values larger than this are not read (registry values are rarely over 1 MB).
const READ_MAX: u32 = 16 << 20;

const STATUS_BUFFER_OVERFLOW: i32 = 0x8000_0005_u32 as i32;
const STATUS_BUFFER_TOO_SMALL: i32 = 0xC000_0023_u32 as i32;

/// Reads the value; `None` if the key or value cannot be read.
pub(crate) fn read(r: &ValueRead, backup: bool) -> Option<ValueData> {
    let key = open_key(&r.key_path, backup)?;
    let mut name = r.value_name.clone();
    let name = counted(&mut name)?;
    // KEY_VALUE_PARTIAL_INFORMATION: TitleIndex @0, Type @4, DataLength @8, Data @12.
    let mut buf = aligned(12 + DATA_MAX);
    for _ in 0..2 {
        let mut ret = 0u32;
        let len = (buf.len() * 8) as u32;
        // SAFETY: `buf` is writable for `len` bytes; `name` points into a live Vec.
        let st = unsafe {
            NtQueryValueKey(key.raw(), &name, KeyValuePartialInformation, Some(buf.as_mut_ptr().cast()), len, &mut ret)
        };
        if st.is_ok() {
            let b = as_bytes(&buf);
            let size = u32_at(b, 8)?;
            let kept = (size as usize).min(DATA_MAX);
            return Some(ValueData { value_type: u32_at(b, 4)?, size, data: b.get(12..12 + kept)?.to_vec() });
        }
        if (st.0 == STATUS_BUFFER_OVERFLOW || st.0 == STATUS_BUFFER_TOO_SMALL) && ret > len && ret <= READ_MAX {
            buf = aligned(ret as usize);
            continue;
        }
        return None;
    }
    None
}

fn open_key(nt_path: &str, backup: bool) -> Option<Owned> {
    let mut units: Vec<u16> = nt_path.encode_utf16().collect();
    let name = counted(&mut units)?;
    let oa = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        ObjectName: &name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_OPENLINK,
        ..Default::default()
    };
    let options = if backup { REG_OPTION_BACKUP_RESTORE.0 } else { 0 };
    let mut h = HANDLE::default();
    // SAFETY: `oa` and the name it points to outlive the call; the handle is owned below.
    unsafe { NtOpenKeyEx(&mut h, KEY_QUERY_VALUE.0, &oa, options) }.ok().ok()?;
    Some(Owned(h))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use windows::Wdk::Foundation::{NtQueryObject, OBJECT_INFORMATION_CLASS};
    use windows::Wdk::System::Registry::NtSetValueKey;
    use windows::Win32::Foundation::UNICODE_STRING;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_BINARY, REG_DWORD, REG_LINK, REG_OPTION_CREATE_LINK,
        REG_OPTION_VOLATILE, REG_SZ, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
    };
    use windows::core::{HSTRING, PCWSTR};

    use super::super::util::wide;

    /// A volatile test key under HKCU, deleted on drop.
    pub(crate) struct TestKey {
        pub hkey: HKEY,
        sub: String,
    }

    impl TestKey {
        pub(crate) fn new(tag: &str) -> Self {
            let sub = format!(r"Software\AtlasTest-{tag}-{}", std::process::id());
            TestKey { hkey: Self::create(&sub, REG_OPTION_VOLATILE.0), sub }
        }

        fn create(sub: &str, options: u32) -> HKEY {
            let mut h = HKEY::default();
            // SAFETY: test-only; creates a key under HKCU.
            unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    &HSTRING::from(sub),
                    None,
                    None,
                    windows::Win32::System::Registry::REG_OPEN_CREATE_OPTIONS(options),
                    KEY_ALL_ACCESS,
                    None,
                    &mut h,
                    None,
                )
            }
            .ok()
            .expect("create test key");
            h
        }

        pub(crate) fn child(&self, name: &str, options: u32) -> HKEY {
            Self::create(&format!(r"{}\{name}", self.sub), options)
        }

        /// The key's NT name (`\REGISTRY\USER\<SID>\Software\…`).
        pub(crate) fn nt_name(h: HKEY) -> String {
            let mut buf = aligned(2048);
            let mut ret = 0u32;
            // SAFETY: ObjectNameInformation into an aligned, writable buffer.
            let st = unsafe {
                NtQueryObject(
                    Some(HANDLE(h.0)),
                    OBJECT_INFORMATION_CLASS(1),
                    Some(buf.as_mut_ptr().cast()),
                    (buf.len() * 8) as u32,
                    Some(&mut ret),
                )
            };
            assert!(st.is_ok());
            // SAFETY: the buffer starts with a UNICODE_STRING pointing into itself.
            let us = unsafe { &*(buf.as_ptr() as *const UNICODE_STRING) };
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(us.Buffer.0, us.Length as usize / 2) })
        }

        pub(crate) fn path(&self) -> String {
            Self::nt_name(self.hkey)
        }

        pub(crate) fn set(&self, h: HKEY, name: &[u16], ty: u32, data: &[u8]) {
            let mut n = name.to_vec();
            let us = counted(&mut n).unwrap();
            // SAFETY: test-only; counted name and data are live.
            let st =
                unsafe { NtSetValueKey(HANDLE(h.0), &us, None, ty, Some(data.as_ptr().cast()), data.len() as u32) };
            assert!(st.is_ok(), "NtSetValueKey {st:?}");
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            // SAFETY: deletes the test tree.
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(self.sub.as_str()));
            }
        }
    }

    pub(crate) fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn read_of(path: String, name: &[u16]) -> ValueRead {
        ValueRead { key_path: path, value_name: name.to_vec() }
    }

    #[test]
    fn reads_types_sizes_and_truncates() {
        let k = TestKey::new("value");
        k.set(k.hkey, &units("d"), REG_DWORD.0, &7u32.to_le_bytes());
        let big = vec![0xAB; 10_000];
        k.set(k.hkey, &units("big"), REG_BINARY.0, &big);
        let d = read(&read_of(k.path(), &units("d")), false).unwrap();
        assert_eq!(d, ValueData { value_type: REG_DWORD.0, size: 4, data: 7u32.to_le_bytes().to_vec() });
        let b = read(&read_of(k.path(), &units("big")), false).unwrap();
        assert_eq!((b.value_type, b.size, b.data.len()), (REG_BINARY.0, 10_000, DATA_MAX));
        assert!(b.data.iter().all(|&x| x == 0xAB));
        assert_eq!(read(&read_of(k.path(), &units("absent")), false), None);
        assert_eq!(read(&read_of(format!(r"{}\nokey", k.path()), &units("d")), false), None);
        // Case-insensitive like the registry; the empty name is the default value.
        k.set(k.hkey, &[], REG_SZ.0, &[b'x', 0, 0, 0]);
        assert_eq!(read(&read_of(k.path().to_uppercase(), &[]), false).map(|v| v.size), Some(4));
    }

    #[test]
    fn embedded_nuls_in_value_names_are_kept() {
        let k = TestKey::new("nul");
        let name = [b'a' as u16, 0, b'b' as u16];
        k.set(k.hkey, &name, REG_DWORD.0, &1u32.to_le_bytes());
        k.set(k.hkey, &units("a"), REG_DWORD.0, &2u32.to_le_bytes());
        assert_eq!(read(&read_of(k.path(), &name), false).map(|v| v.data), Some(1u32.to_le_bytes().to_vec()));
        assert_eq!(read(&read_of(k.path(), &units("a")), false).map(|v| v.data), Some(2u32.to_le_bytes().to_vec()));
    }

    #[test]
    fn symbolic_link_keys_are_not_followed() {
        let k = TestKey::new("link");
        let target = k.child("target", REG_OPTION_VOLATILE.0);
        k.set(target, &units("v"), REG_DWORD.0, &5u32.to_le_bytes());
        let link = k.child("link", REG_OPTION_VOLATILE.0 | REG_OPTION_CREATE_LINK.0);
        let to: Vec<u8> = TestKey::nt_name(target).encode_utf16().flat_map(u16::to_le_bytes).collect();
        let slv = wide("SymbolicLinkValue");
        // SAFETY: test-only; REG_LINK data is the counted target name (no NUL).
        unsafe { RegSetValueExW(link, PCWSTR(slv.as_ptr()), None, REG_LINK, Some(&to)) }.ok().unwrap();
        // Control: a reader that follows links sees the target's value.
        let mut v = 0u32;
        let mut size = 4u32;
        let sub = HSTRING::from(format!(r"{}\link", k.sub));
        // SAFETY: test-only; writes at most 4 bytes.
        unsafe {
            windows::Win32::System::Registry::RegGetValueW(
                HKEY_CURRENT_USER,
                &sub,
                &HSTRING::from("v"),
                windows::Win32::System::Registry::RRF_RT_REG_DWORD,
                None,
                Some((&mut v as *mut u32).cast()),
                Some(&mut size),
            )
        }
        .ok()
        .expect("the link is followed by a normal open");
        assert_eq!(v, 5);
        let through = format!(r"{}\link", k.path());
        assert_eq!(read(&read_of(through, &units("v")), false), None, "the link itself has no value v");
        assert!(read(&read_of(TestKey::nt_name(target), &units("v")), false).is_some());
    }

    #[test]
    fn unreadable_hives_fail() {
        assert_eq!(
            read(&read_of(r"\REGISTRY\A\{00000000-0000-0000-0000-000000000000}".into(), &units("x")), false),
            None
        );
    }

    /// `HKLM\SAM\SAM` allows only SYSTEM: an elevated admin reads it only
    /// with `REG_OPTION_BACKUP_RESTORE` and `SeBackupPrivilege`.
    #[test]
    #[ignore = "needs SeBackupPrivilege (an elevated run)"]
    fn backup_semantics_pass_a_system_only_dacl() {
        assert!(super::super::privilege::enable(super::super::privilege::BACKUP));
        let r = read_of(r"\REGISTRY\MACHINE\SAM\SAM\Domains\Account".into(), &units("F"));
        assert_eq!(read(&r, false), None, "denied without backup semantics");
        let v = read(&r, true).expect("read with backup semantics");
        assert_eq!(v.value_type, REG_BINARY.0);
        assert!(v.size > 0);
    }
}
```

- [ ] **Step 2: 8.3 expansion**

`src/win/expand.rs`:
```rust
//! 8.3 short-name expansion (sensor spec §7.2; plan 1b-3b, D4).
//!
//! `GetLongPathNameW` rejects `\\?\GLOBALROOT\Device\…` paths, so each short
//! component is looked up in its parent directory, opened by NT path, with
//! `NtQueryDirectoryFile` and the short name as the filter. That works for
//! shadow copies too. Only components that look short are looked up
//! (~54 µs each on the host).
//!
//! Results are cached by (parent directory, short name). Short names are
//! reused after a delete or rename, so the pipeline's `InvalidateHash` for a
//! path also drops the entries for that path and everything under it, and an
//! entry expires after [`TTL`] in case a change was never seen.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::time::{Duration, Instant};

use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::Storage::FileSystem::{FileBothDirectoryInformation, NtOpenFile, NtQueryDirectoryFile};
use windows::Win32::Foundation::{HANDLE, OBJ_CASE_INSENSITIVE};
use windows::Win32::System::IO::IO_STATUS_BLOCK;

use super::util::{Owned, aligned, as_bytes, counted, u32_at};
use crate::paths::is_short_name as is_short_component;

pub(crate) const TTL: Duration = Duration::from_secs(60);
const CAP: usize = 16_384;

/// Directory lookups, behind a trait so the cache can be tested without disks.
pub(crate) trait Dir {
    /// The long name of `component` in the directory `dir` (an NT path).
    fn long_name(&mut self, dir: &str, component: &str) -> Option<String>;
}

/// (parent, lowercased) + (short name, uppercased).
type Key = (String, String);

pub(crate) struct Expander<D> {
    dir: D,
    cache: BTreeMap<Key, (String, Instant)>,
    /// Lowercased full long path of a cached directory entry → its key.
    by_long: HashMap<String, Key>,
}

impl<D: Dir> Expander<D> {
    pub(crate) fn new(dir: D) -> Self {
        Expander { dir, cache: BTreeMap::new(), by_long: HashMap::new() }
    }

    /// The long form of `nt_path`, or `None` if a short component cannot be
    /// expanded. Only `\Device\HarddiskVolume…` paths (volumes and shadow
    /// copies) are expanded: a network redirector could stall the reader lane.
    pub(crate) fn expand(&mut self, nt_path: &str, now: Instant) -> Option<String> {
        let (device, rest) = split_device(nt_path)?;
        if !device.to_ascii_lowercase().starts_with(r"\device\harddiskvolume") {
            return None;
        }
        let mut out = device.to_string();
        for comp in rest.split('\\').filter(|c| !c.is_empty()) {
            let long = if is_short_component(comp) { self.component(&out, comp, now)? } else { comp.to_string() };
            out.push('\\');
            out.push_str(&long);
        }
        if rest.ends_with('\\') {
            out.push('\\');
        }
        Some(out)
    }

    fn component(&mut self, parent: &str, short: &str, now: Instant) -> Option<String> {
        let key = (parent.to_lowercase(), short.to_uppercase());
        if let Some((long, at)) = self.cache.get(&key) {
            if now.duration_since(*at) < TTL {
                return Some(long.clone());
            }
            self.remove(&key);
        }
        let long = self.dir.long_name(parent, short)?;
        if self.cache.len() >= CAP {
            self.cache.clear();
            self.by_long.clear();
        }
        self.by_long.insert(format!(r"{}\{}", key.0, long.to_lowercase()), key.clone());
        self.cache.insert(key, (long.clone(), now));
        Some(long)
    }

    fn remove(&mut self, key: &Key) {
        if let Some((long, _)) = self.cache.remove(key) {
            self.by_long.remove(&format!(r"{}\{}", key.0, long.to_lowercase()));
        }
    }

    /// Something at `nt_path` changed (deleted, renamed away, written): forget
    /// what the cache knows about it and below it. `nt_path` may itself hold
    /// short components; they are resolved from the cache only.
    pub(crate) fn invalidate(&mut self, nt_path: &str) {
        let Some((device, rest)) = split_device(nt_path) else { return };
        let mut long = device.to_lowercase();
        let mut whole = true;
        let comps: Vec<&str> = rest.split('\\').filter(|c| !c.is_empty()).collect();
        for (i, comp) in comps.iter().enumerate() {
            let key = (long.clone(), comp.to_uppercase());
            if i + 1 == comps.len() && is_short_component(comp) {
                self.remove(&key);
            }
            match self.cache.get(&key) {
                Some((l, _)) if is_short_component(comp) => long = format!(r"{long}\{}", l.to_lowercase()),
                _ if is_short_component(comp) => {
                    whole = false;
                    break;
                }
                _ => long = format!(r"{long}\{}", comp.to_lowercase()),
            }
        }
        if !whole {
            return;
        }
        if let Some(key) = self.by_long.get(&long).cloned() {
            self.remove(&key);
        }
        // Entries whose parent is the path, or lies under it.
        let below = format!(r"{long}\");
        let doomed: Vec<Key> = self
            .cache
            .range((Bound::Included((long.clone(), String::new())), Bound::Unbounded))
            .map(|(k, _)| k)
            .take_while(|k| k.0 == long || k.0.starts_with(&long))
            .filter(|k| k.0 == long || k.0.starts_with(&below))
            .cloned()
            .collect();
        for k in doomed {
            self.remove(&k);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.cache.len()
    }
}

/// `\Device\X` and the rest (which starts with `\`, or is empty).
fn split_device(nt: &str) -> Option<(&str, &str)> {
    const DEVICE: &str = r"\Device\";
    if !nt.get(..DEVICE.len())?.eq_ignore_ascii_case(DEVICE) {
        return None;
    }
    let end = nt[DEVICE.len()..].find('\\').map_or(nt.len(), |i| i + DEVICE.len());
    Some((&nt[..end], &nt[end..]))
}

/// The real directory lookup.
pub(crate) struct NtDir;

const FILE_LIST_DIRECTORY: u32 = 0x1;
const SYNCHRONIZE: u32 = 0x10_0000;
const FILE_SHARE_ALL: u32 = 0x7;
const FILE_DIRECTORY_FILE: u32 = 0x1;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
const FILE_OPEN_FOR_BACKUP_INTENT: u32 = 0x4000;

impl Dir for NtDir {
    fn long_name(&mut self, dir: &str, component: &str) -> Option<String> {
        // A volume's root directory needs its trailing backslash.
        let dir = if split_device(dir)?.1.is_empty() { format!(r"{dir}\") } else { dir.to_string() };
        let mut d: Vec<u16> = dir.encode_utf16().collect();
        let name = counted(&mut d)?;
        let oa = OBJECT_ATTRIBUTES {
            Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
            ObjectName: &name,
            Attributes: OBJ_CASE_INSENSITIVE,
            ..Default::default()
        };
        let mut h = HANDLE::default();
        let mut iosb = IO_STATUS_BLOCK::default();
        // SAFETY: `oa` and its name outlive the call; the handle is owned below.
        unsafe {
            NtOpenFile(
                &mut h,
                FILE_LIST_DIRECTORY | SYNCHRONIZE,
                &oa,
                &mut iosb,
                FILE_SHARE_ALL,
                FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_FOR_BACKUP_INTENT,
            )
        }
        .ok()
        .ok()?;
        let h = Owned(h);
        let mut c: Vec<u16> = component.encode_utf16().collect();
        let filter = counted(&mut c)?;
        let mut buf = aligned(4096);
        // SAFETY: `buf` is writable for its length; one entry, from the start.
        unsafe {
            NtQueryDirectoryFile(
                h.raw(),
                None,
                None,
                None,
                &mut iosb,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                FileBothDirectoryInformation,
                true,
                Some(&filter),
                true,
            )
        }
        .ok()
        .ok()?;
        // FILE_BOTH_DIR_INFORMATION: FileNameLength @60, ShortNameLength (i8) @68,
        // ShortName [u16; 12] @70, FileName @94.
        let b = as_bytes(&buf);
        let name_len = u32_at(b, 60)? as usize;
        let short_len = (*b.get(68)? as usize).min(24);
        let utf16 = |bytes: &[u8]| {
            String::from_utf16_lossy(
                &bytes.chunks_exact(2).map(|x| u16::from_le_bytes([x[0], x[1]])).collect::<Vec<_>>(),
            )
        };
        let long = utf16(b.get(94..94 + name_len)?);
        let short = utf16(b.get(70..70 + short_len)?);
        // The filter matches either name; accept only an exact match of one.
        (short.eq_ignore_ascii_case(component) || long.eq_ignore_ascii_case(component)).then_some(long)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A fake directory tree: (parent lowercased, short uppercased) → long; counts lookups.
    type Tree = (HashMap<(String, String), String>, u32);

    #[derive(Clone, Default)]
    struct FakeDir(Rc<RefCell<Tree>>);

    impl FakeDir {
        fn put(&self, parent: &str, short: &str, long: &str) {
            self.0.borrow_mut().0.insert((parent.to_lowercase(), short.to_uppercase()), long.into());
        }
        fn lookups(&self) -> u32 {
            self.0.borrow().1
        }
    }

    impl Dir for FakeDir {
        fn long_name(&mut self, dir: &str, c: &str) -> Option<String> {
            let mut s = self.0.borrow_mut();
            s.1 += 1;
            s.0.get(&(dir.to_lowercase(), c.to_uppercase())).cloned()
        }
    }

    const V: &str = r"\Device\HarddiskVolume3";

    fn tree() -> FakeDir {
        let d = FakeDir::default();
        d.put(V, "SECRET~1", "SecretStuffAAA");
        d.put(&format!(r"{V}\SecretStuffAAA"), "LONGFI~1.TXT", "long file.txt");
        d
    }

    #[test]
    fn expands_short_components_only_and_caches() {
        let d = tree();
        let mut e = Expander::new(d.clone());
        let t = Instant::now();
        let p = format!(r"{V}\SECRET~1\LONGFI~1.TXT");
        assert_eq!(e.expand(&p, t), Some(format!(r"{V}\SecretStuffAAA\long file.txt")));
        assert_eq!(d.lookups(), 2);
        let lower = e.expand(&p.to_lowercase(), t).unwrap();
        assert_eq!(lower, format!(r"{}\SecretStuffAAA\long file.txt", V.to_lowercase()), "the device as given");
        assert_eq!(d.lookups(), 2, "cached");
        assert_eq!(e.expand(&format!(r"{V}\Plain\name.txt"), t), Some(format!(r"{V}\Plain\name.txt")));
        assert_eq!(d.lookups(), 2, "long components are never looked up");
        assert_eq!(e.expand(&format!(r"{V}\NOPE~1\x"), t), None);
        assert_eq!(e.expand(&format!(r"{V}\"), t), Some(format!(r"{V}\")));
    }

    #[test]
    fn device_prefix_in_any_case() {
        assert_eq!(split_device(r"\DEVICE\HarddiskVolume3\x"), Some((r"\DEVICE\HarddiskVolume3", r"\x")));
        assert_eq!(split_device(r"\device\HarddiskVolume3"), Some((r"\device\HarddiskVolume3", "")));
        assert_eq!(split_device(r"\Devices\x"), None);
        assert_eq!(split_device(r"C:\x"), None);
    }

    #[test]
    fn only_local_volumes() {
        let d = FakeDir::default();
        let mut e = Expander::new(d.clone());
        assert_eq!(e.expand(r"\Device\Mup\server\SHARE~1\x", Instant::now()), None);
        assert_eq!(d.lookups(), 0);
        let shadow = r"\Device\HarddiskVolumeShadowCopy1";
        d.put(shadow, "USERDA~1", "User Data");
        assert_eq!(e.expand(&format!(r"{shadow}\USERDA~1"), Instant::now()), Some(format!(r"{shadow}\User Data")));
    }

    #[test]
    fn entries_expire() {
        let d = tree();
        let mut e = Expander::new(d.clone());
        let t = Instant::now();
        e.expand(&format!(r"{V}\SECRET~1"), t);
        e.expand(&format!(r"{V}\SECRET~1"), t + TTL - Duration::from_millis(1));
        assert_eq!(d.lookups(), 1);
        e.expand(&format!(r"{V}\SECRET~1"), t + TTL);
        assert_eq!(d.lookups(), 2);
    }

    /// The reuse case measured on the host (D4): delete a directory, and the
    /// next new one gets its short name.
    #[test]
    fn invalidation_by_long_path_covers_the_entry_and_below() {
        let d = tree();
        let mut e = Expander::new(d.clone());
        let t = Instant::now();
        e.expand(&format!(r"{V}\SECRET~1\LONGFI~1.TXT"), t);
        assert_eq!(e.len(), 2);
        e.invalidate(&format!(r"{V}\SecretStuffAAA"));
        assert_eq!(e.len(), 0, "the directory and the entry inside it");
        d.put(V, "SECRET~1", "SecretStuffBBB");
        assert_eq!(e.expand(&format!(r"{V}\SECRET~1"), t), Some(format!(r"{V}\SecretStuffBBB")));
    }

    #[test]
    fn invalidation_by_short_path() {
        let d = tree();
        let mut e = Expander::new(d.clone());
        let t = Instant::now();
        e.expand(&format!(r"{V}\SECRET~1\LONGFI~1.TXT"), t);
        e.invalidate(&format!(r"{V}\secret~1\longfi~1.txt"));
        assert_eq!(e.len(), 1, "only the file's entry");
        e.invalidate(&format!(r"{V}\SECRET~1"));
        assert_eq!(e.len(), 0);
    }

    #[test]
    fn invalidation_leaves_siblings_and_lookalikes() {
        let d = tree();
        d.put(V, "SECRET~2", "SecretStuffAAA-2");
        d.put(&format!(r"{V}\SecretStuffAAA-2"), "OTHERF~1", "other file");
        let mut e = Expander::new(d.clone());
        let t = Instant::now();
        e.expand(&format!(r"{V}\SECRET~1\LONGFI~1.TXT"), t);
        e.expand(&format!(r"{V}\SECRET~2\OTHERF~1"), t);
        assert_eq!(e.len(), 4);
        e.invalidate(&format!(r"{V}\SecretStuffAAA"));
        assert_eq!(e.len(), 2, "SecretStuffAAA-2 and its child stay");
        e.invalidate(&format!(r"{V}\unrelated.txt"));
        assert_eq!(e.len(), 2);
    }

    #[test]
    fn real_directories() {
        let dir = std::env::temp_dir().join(format!("atlas-expand-{}", std::process::id()));
        let deep = dir.join("Long Directory Alpha").join("a-long-file-name.txt");
        std::fs::create_dir_all(deep.parent().unwrap()).unwrap();
        std::fs::write(&deep, b"x").unwrap();
        let devices = super::super::lookups::query_drives();
        let long = deep.to_string_lossy().to_string();
        let (dev, drive) = devices.iter().find(|(_, d)| long[..2].eq_ignore_ascii_case(d)).unwrap();
        let short = short_path(&long);
        assert!(short.contains('~'), "8.3 names are on for this volume: {short}");
        let mut e = Expander::new(NtDir);
        let got = e.expand(&format!("{dev}{}", &short[drive.len()..]), Instant::now());
        // The temp directory itself may be spelled short in TEMP; compare the long forms.
        assert_eq!(got.map(|g| g.to_lowercase()), Some(format!("{dev}{}", &long_path(&long)[2..]).to_lowercase()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn short_path(p: &str) -> String {
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;
        let w = super::super::util::wide(p);
        let mut buf = vec![0u16; 1024];
        // SAFETY: test-only; NUL-terminated input, writable output.
        let n = unsafe { GetShortPathNameW(windows::core::PCWSTR(w.as_ptr()), Some(&mut buf)) } as usize;
        String::from_utf16_lossy(&buf[..n])
    }

    fn long_path(p: &str) -> String {
        use windows::Win32::Storage::FileSystem::GetLongPathNameW;
        let w = super::super::util::wide(p);
        let mut buf = vec![0u16; 1024];
        // SAFETY: as above.
        let n = unsafe { GetLongPathNameW(windows::core::PCWSTR(w.as_ptr()), Some(&mut buf)) } as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy -p atlas-agent --all-targets -- -D warnings -A dead-code
```
Expected: 128 tests pass, 2 ignored, clippy clean.
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): registry value reads and 8.3 expansion with an invalidated cache"
```

### Task 5: The seeder

**Files:**
- Modify: `crates/atlas-agent/src/win/mod.rs` (`mod handles;`, `mod seeder;`)
- Create: `crates/atlas-agent/src/win/handles.rs`, `seeder.rs`

- [ ] **Step 1: The handle table and naming**

`src/win/handles.rs`:
```rust
//! The system handle table and naming handles from it (sensor spec §7.4).
//!
//! Handles are duplicated into the agent with **no access rights** (plan
//! 1b-3b, D2), so the agent never holds a handle that can read or write
//! another process's object. Keys are named with
//! `NtQueryObject(ObjectNameInformation)`: `NtQueryKey(KeyNameInformation)` is
//! denied on a no-access handle. Files are named with
//! `GetFinalPathNameByHandleW` on a helper thread with a timeout, because a
//! query on a synchronous file object can block behind another thread's I/O.

use std::os::windows::io::AsRawHandle;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Wdk::Foundation::{NtQueryObject, OBJECT_INFORMATION_CLASS};
use windows::Wdk::Storage::FileSystem::{FileFsDeviceInformation, NtQueryVolumeInformationFile};
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS};
use windows::Win32::Foundation::{DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, HANDLE, UNICODE_STRING};
use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, VOLUME_NAME_NT};
use windows::Win32::System::IO::{CancelSynchronousIo, IO_STATUS_BLOCK};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE};

use super::util::{Owned, aligned, as_bytes};

/// One row of the handle table (`SYSTEM_HANDLE_TABLE_ENTRY_INFO_EX`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Entry {
    /// Kernel object address: Kernel-Registry's `KeyObject`, Kernel-File's `FileObject`.
    pub object: u64,
    pub pid: u32,
    pub handle: u64,
    pub type_index: u16,
}

/// `SYSTEM_HANDLE_TABLE_ENTRY_INFO_EX` (x64, 40 bytes): Object @0, UniqueProcessId @8,
/// HandleValue @16, GrantedAccess @24, CreatorBackTraceIndex @28, ObjectTypeIndex @30.
const ENTRY: usize = 40;

/// Reads the whole table. Object addresses are zero without `SeDebugPrivilege`.
pub(crate) fn table() -> Option<Vec<Entry>> {
    const SYSTEM_EXTENDED_HANDLE_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(64);
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
    let mut buf = aligned(8 << 20);
    for _ in 0..8 {
        let mut ret = 0u32;
        let len = (buf.len() * 8) as u32;
        // SAFETY: `buf` is writable for `len` bytes and 8-aligned.
        let st = unsafe {
            NtQuerySystemInformation(SYSTEM_EXTENDED_HANDLE_INFORMATION, buf.as_mut_ptr().cast(), len, &mut ret)
        };
        if st.0 == STATUS_INFO_LENGTH_MISMATCH {
            // The table grows between calls: leave headroom.
            buf = aligned((ret as usize).max(len as usize) * 3 / 2);
            continue;
        }
        if st.is_err() {
            return None;
        }
        return Some(parse_table(as_bytes(&buf)));
    }
    None
}

/// Header: NumberOfHandles (usize), Reserved (usize); then the entries.
pub(crate) fn parse_table(b: &[u8]) -> Vec<Entry> {
    let field = |off: usize| b.get(off..off + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()));
    let count = field(0).unwrap_or(0) as usize;
    let count = count.min(b.len().saturating_sub(16) / ENTRY);
    (0..count)
        .filter_map(|i| {
            let e = 16 + i * ENTRY;
            Some(Entry {
                object: field(e)?,
                pid: u32::try_from(field(e + 8)?).ok()?,
                handle: field(e + 16)?,
                type_index: u16::from_le_bytes(b.get(e + 30..e + 32)?.try_into().ok()?),
            })
        })
        .collect()
}

/// A process opened for duplicating its handles.
pub(crate) fn open_process(pid: u32) -> Option<Owned> {
    // SAFETY: the handle is owned by `Owned`.
    unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, pid) }.ok().map(Owned)
}

/// Duplicates `handle` from `process` into the agent with no access rights.
pub(crate) fn duplicate(process: &Owned, handle: u64) -> Option<Owned> {
    let mut dup = HANDLE::default();
    // SAFETY: duplicates into our own process; the copy is owned by `Owned`.
    unsafe {
        DuplicateHandle(
            process.raw(),
            HANDLE(handle as *mut _),
            GetCurrentProcess(),
            &mut dup,
            0,
            false,
            DUPLICATE_HANDLE_OPTIONS(0),
        )
    }
    .ok()?;
    Some(Owned(dup))
}

/// The object's name (`ObjectNameInformation`): for a key, `\REGISTRY\…`.
/// Never used on file handles, where it can block.
pub(crate) fn object_name(h: &Owned) -> Option<String> {
    let mut buf = aligned(4096);
    for _ in 0..2 {
        let mut ret = 0u32;
        let len = (buf.len() * 8) as u32;
        // SAFETY: `buf` is writable and aligned for an OBJECT_NAME_INFORMATION.
        let st = unsafe {
            NtQueryObject(
                Some(h.raw()),
                OBJECT_INFORMATION_CLASS(1),
                Some(buf.as_mut_ptr().cast()),
                len,
                Some(&mut ret),
            )
        };
        if st.is_ok() {
            // SAFETY: the buffer starts with a UNICODE_STRING whose Buffer points into it.
            let us = unsafe { &*(buf.as_ptr() as *const UNICODE_STRING) };
            if us.Buffer.is_null() || us.Length == 0 {
                return None;
            }
            // SAFETY: Length bytes at Buffer lie inside `buf`.
            let units = unsafe { std::slice::from_raw_parts(us.Buffer.0, us.Length as usize / 2) };
            return Some(String::from_utf16_lossy(units));
        }
        if ret > len && ret <= 1 << 16 {
            buf = aligned(ret as usize);
            continue;
        }
        return None;
    }
    None
}

/// `FILE_FS_DEVICE_INFORMATION`.
#[repr(C)]
#[derive(Default)]
struct DeviceInfo {
    device_type: u32,
    characteristics: u32,
}

const FILE_DEVICE_DISK: u32 = 7;
const FILE_REMOTE_DEVICE: u32 = 0x10;

/// The NT name of a disk file; `None` for anything else (pipes, sockets,
/// devices, network redirectors) or a failed query. May block.
fn disk_file_name(h: &Owned) -> Option<String> {
    let mut info = DeviceInfo::default();
    let mut iosb = IO_STATUS_BLOCK::default();
    // SAFETY: `info` is writable for its size.
    let st = unsafe {
        NtQueryVolumeInformationFile(
            h.raw(),
            &mut iosb,
            (&mut info as *mut DeviceInfo).cast(),
            size_of::<DeviceInfo>() as u32,
            FileFsDeviceInformation,
        )
    };
    if st.is_err() || info.device_type != FILE_DEVICE_DISK || info.characteristics & FILE_REMOTE_DEVICE != 0 {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: `buf` is writable; VOLUME_NAME_NT gives `\Device\HarddiskVolumeN\…`, long names.
        let n = unsafe { GetFinalPathNameByHandleW(h.raw(), &mut buf, VOLUME_NAME_NT) } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            return Some(String::from_utf16_lossy(&buf[..n]));
        }
        if n > 32_768 {
            return None;
        }
        buf = vec![0u16; n + 1];
    }
}

/// How a file name query ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FileName {
    /// A disk file's NT name, with the duplicate handed back.
    Named(String, Owned),
    /// Not a disk file, or the query failed.
    Unnamable,
    /// No answer within the timeout: the helper keeps the handle and closes it
    /// when its query returns.
    TimedOut,
}

impl PartialEq for Owned {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Owned {}

/// One helper thread that names file handles.
struct Helper {
    tx: Sender<Owned>,
    rx: Receiver<(Option<String>, Owned)>,
    thread: JoinHandle<()>,
}

impl Helper {
    fn spawn() -> Helper {
        let (tx, jobs) = channel::<Owned>();
        let (done, rx) = channel();
        let thread = std::thread::Builder::new()
            .name("atlas-seeder-namer".into())
            .spawn(move || {
                for h in jobs {
                    let name = disk_file_name(&h);
                    // A failed send means the seeder gave up on this query; `h` closes here.
                    if done.send((name, h)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn a seeder helper");
        Helper { tx, rx, thread }
    }
}

/// File naming with a timeout per query (§7.4). A query that times out is
/// cancelled with `CancelSynchronousIo`; a helper that still does not return
/// is set aside as stuck, and a new one takes over. At most `max_stuck`
/// helpers may be stuck: past that, [`FileNamer::paused`] is true.
pub(crate) struct FileNamer {
    helper: Helper,
    stuck: Vec<Helper>,
    timeout: Duration,
    max_stuck: usize,
}

impl FileNamer {
    pub(crate) fn new(timeout: Duration, max_stuck: usize) -> Self {
        FileNamer { helper: Helper::spawn(), stuck: Vec::new(), timeout, max_stuck }
    }

    /// Helpers stuck now (a Sensor Health gauge). Recovered ones are dropped.
    pub(crate) fn stuck(&mut self) -> usize {
        // A stuck helper that answered has returned: its result is stale, the
        // handle closes with it, and the thread exits once its sender drops.
        self.stuck.retain(|h| matches!(h.rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
        self.stuck.len()
    }

    /// Too many helpers are stuck: file seeding pauses (§7.4).
    pub(crate) fn paused(&mut self) -> bool {
        self.stuck() >= self.max_stuck
    }

    pub(crate) fn name(&mut self, h: Owned) -> FileName {
        if self.helper.tx.send(h).is_err() {
            self.helper = Helper::spawn();
            return FileName::Unnamable;
        }
        match self.helper.rx.recv_timeout(self.timeout) {
            Ok((Some(n), h)) => return FileName::Named(n, h),
            Ok((None, _)) => return FileName::Unnamable,
            Err(RecvTimeoutError::Disconnected) => {
                self.helper = Helper::spawn();
                return FileName::Unnamable;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        // SAFETY: the helper thread is alive (its channel is open); cancels its
        // pending synchronous I/O, if any.
        unsafe {
            let _ = CancelSynchronousIo(HANDLE(self.helper.thread.as_raw_handle()));
        }
        match self.helper.rx.recv_timeout(Duration::from_millis(50)) {
            Ok(_) => {}
            Err(_) => {
                let replacement = Helper::spawn();
                self.stuck.push(std::mem::replace(&mut self.helper, replacement));
            }
        }
        FileName::TimedOut
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::{FromRawHandle, IntoRawHandle};

    /// Duplicates one of our own handles, as the seeder does for others'.
    fn dup_own(h: HANDLE) -> Owned {
        // SAFETY: test-only; a pseudo-handle for our own process.
        let me = Owned(unsafe { windows::Win32::System::Threading::GetCurrentProcess() });
        let d = duplicate(&me, h.0 as u64).expect("duplicate");
        std::mem::forget(me); // pseudo-handle: never closed
        d
    }

    #[test]
    fn parses_entries() {
        let mut b = vec![0u8; 16 + 2 * ENTRY];
        b[0..8].copy_from_slice(&2u64.to_le_bytes());
        let e = 16 + ENTRY;
        b[e..e + 8].copy_from_slice(&0xffff_a000_1234_5678u64.to_le_bytes());
        b[e + 8..e + 16].copy_from_slice(&4321u64.to_le_bytes());
        b[e + 16..e + 24].copy_from_slice(&0x1a4u64.to_le_bytes());
        b[e + 30..e + 32].copy_from_slice(&44u16.to_le_bytes());
        let t = parse_table(&b);
        assert_eq!(t.len(), 2);
        assert_eq!(t[1], Entry { object: 0xffff_a000_1234_5678, pid: 4321, handle: 0x1a4, type_index: 44 });
        // A count larger than the buffer is cut to what is there.
        b[0..8].copy_from_slice(&1000u64.to_le_bytes());
        assert_eq!(parse_table(&b).len(), 2);
    }

    #[test]
    fn our_handles_are_in_the_table() {
        let f = std::fs::File::open(std::env::current_exe().unwrap()).unwrap();
        let t = table().expect("handle table");
        let me = std::process::id();
        assert!(t.iter().any(|e| e.pid == me && e.handle == f.as_raw_handle() as u64));
    }

    #[test]
    fn keys_are_named_through_a_no_access_duplicate() {
        let k = super::super::value::tests::TestKey::new("handles");
        let d = dup_own(HANDLE(k.hkey.0));
        assert_eq!(object_name(&d), Some(k.path()));
    }

    #[test]
    fn disk_files_are_named_and_others_are_not() {
        let mut namer = FileNamer::new(Duration::from_millis(200), 2);
        let exe = std::env::current_exe().unwrap();
        let f = std::fs::File::open(&exe).unwrap();
        let FileName::Named(n, _) = namer.name(dup_own(HANDLE(f.as_raw_handle()))) else { panic!("not named") };
        assert!(
            n.starts_with(r"\Device\")
                && n.to_lowercase().ends_with(&exe.file_name().unwrap().to_string_lossy().to_lowercase()),
            "{n}"
        );
        let (r, _w) = std::io::pipe().unwrap();
        let r = r.into_raw_handle();
        assert_eq!(namer.name(dup_own(HANDLE(r))), FileName::Unnamable, "an anonymous pipe is not a disk file");
        // SAFETY: test-only; reclaim the pipe end so it closes.
        drop(unsafe { std::fs::File::from_raw_handle(r) });
    }

    /// A query on a synchronous file object waits while another thread's
    /// operation on it is pending: here a byte-range lock that cannot be
    /// granted yet. The namer gives up, sets the helper aside as stuck, pauses
    /// at the limit, and recovers when the operation completes. (A pipe with a
    /// read pending does not block: its device type is answered at once, and
    /// pipes are never named.)
    #[test]
    fn blocked_queries_time_out_and_recover() {
        use windows::Win32::Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LockFileEx, UnlockFile};
        use windows::Win32::System::IO::OVERLAPPED;
        let path = std::env::temp_dir().join(format!("atlas-namer-{}.bin", std::process::id()));
        std::fs::write(&path, b"0123").unwrap();
        let holder = std::fs::File::open(&path).unwrap();
        let waiter = std::fs::File::open(&path).unwrap();
        let lock = |f: &std::fs::File| {
            let mut ov = OVERLAPPED::default();
            // SAFETY: test-only; a synchronous handle, so the call waits until granted.
            unsafe { LockFileEx(HANDLE(f.as_raw_handle()), LOCKFILE_EXCLUSIVE_LOCK, None, 1, 0, &mut ov) }.unwrap();
        };
        lock(&holder);
        let waiter_raw = HANDLE(waiter.as_raw_handle());
        let mut namer = FileNamer::new(Duration::from_millis(100), 1);
        let d = dup_own(waiter_raw);
        let t = std::thread::spawn(move || {
            lock(&waiter);
            waiter
        });
        std::thread::sleep(Duration::from_millis(100)); // the lock request is pending
        assert_eq!(namer.name(d), FileName::TimedOut);
        assert_eq!(namer.stuck(), 1);
        assert!(namer.paused());
        // SAFETY: test-only; releasing the first lock grants the second.
        unsafe { UnlockFile(HANDLE(holder.as_raw_handle()), 0, 0, 1, 0) }.unwrap();
        drop(t.join().unwrap());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(namer.stuck(), 0, "the helper returned");
        assert!(!namer.paused());
        drop(holder);
        std::fs::remove_file(&path).unwrap();
    }
}
```

- [ ] **Step 2: Snapshots, verification and the budget**

`src/win/seeder.rs`:
```rust
//! The seeder ([8], sensor spec §7.4): names key and file handles from the
//! system handle table, at start and when the pipeline meets an unknown one.
//!
//! - Each address is answered once: `named` if any holder's handle could be
//!   named, else `unnamable`. Its owner is the holder with the lowest PID, the
//!   agent excluded (§7.4); an address only the agent holds is unnamable.
//! - A holder that cannot be opened or duplicated is skipped for the next one.
//!   A name query is made once per address: every handle reaches the same object.
//! - **Verification** (plan 1b-3b, finding F2): a handle can be closed and its
//!   value reused for another object between the table read and the duplicate.
//!   The duplicates are kept open while the table is read a second time, and a
//!   name is kept only if the agent's duplicate sits at the address asked
//!   about. `taken` is the QPC of that second read: every name was valid then.
//! - The start-up pass covers the whole table and is not charged to the CPU
//!   budget; re-reads on a miss are, and wait while it is spent (counted).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW};
use windows::Win32::System::Threading::{
    GetCurrentThread, GetThreadTimes, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
};
use windows::core::w;

use super::handles::{Entry, FileName, FileNamer, duplicate, object_name, open_process, table};
use super::privilege;
use super::util::{Owned, qpc_now};
use crate::config::ServiceConfig;
use crate::counters::ServiceCounters;
use crate::services::{HandleKind, Named, Reply, Snapshot};

/// The holders of each covered address, the agent excluded, lowest PID first;
/// and the covered addresses only the agent holds.
pub(crate) fn plan(table: &[Entry], type_index: u16, me: u32, asked: &[u64]) -> (BTreeMap<u64, Vec<Entry>>, Vec<u64>) {
    let asked: HashSet<u64> = asked.iter().copied().collect();
    let mut holders: BTreeMap<u64, Vec<Entry>> = BTreeMap::new();
    let mut own: BTreeSet<u64> = BTreeSet::new();
    for e in table.iter().filter(|e| e.type_index == type_index && e.object != 0) {
        if !asked.is_empty() && !asked.contains(&e.object) {
            continue;
        }
        if e.pid == me {
            own.insert(e.object);
        } else {
            holders.entry(e.object).or_default().push(*e);
        }
    }
    for h in holders.values_mut() {
        h.sort_by_key(|e| (e.pid, e.handle));
    }
    let own_only = own.into_iter().filter(|a| !holders.contains_key(a)).collect();
    (holders, own_only)
}

/// The addresses whose duplicate (agent handle value) still refers to them in
/// the second read of the table.
pub(crate) fn verified(after: &[Entry], me: u32, dups: impl Iterator<Item = (u64, u64)>) -> HashSet<u64> {
    let mine: HashMap<u64, u64> = after.iter().filter(|e| e.pid == me).map(|e| (e.handle, e.object)).collect();
    dups.filter(|(address, handle)| mine.get(handle) == Some(address)).map(|(a, _)| a).collect()
}

/// CPU spent per sliding window (§7.4).
pub(crate) struct Budget {
    limit: Duration,
    window: Duration,
    spent: VecDeque<(Instant, Duration)>,
}

impl Budget {
    pub(crate) fn new(limit: Duration, window: Duration) -> Self {
        Budget { limit, window, spent: VecDeque::new() }
    }

    fn prune(&mut self, now: Instant) {
        while self.spent.front().is_some_and(|(t, _)| now.duration_since(*t) >= self.window) {
            self.spent.pop_front();
        }
    }

    pub(crate) fn allows(&mut self, now: Instant) -> bool {
        self.prune(now);
        self.spent.iter().map(|(_, d)| *d).sum::<Duration>() < self.limit
    }

    /// When to look again: once the oldest charge leaves the window.
    pub(crate) fn retry_in(&mut self, now: Instant) -> Duration {
        self.prune(now);
        let next =
            self.spent.front().map_or(Duration::ZERO, |(t, _)| self.window.saturating_sub(now.duration_since(*t)));
        next.max(Duration::from_millis(10))
    }

    pub(crate) fn charge(&mut self, now: Instant, cpu: Duration) {
        self.spent.push_back((now, cpu));
    }
}

/// This thread's CPU time (kernel + user).
fn thread_cpu() -> Duration {
    let (mut c, mut e, mut k, mut u) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: the current-thread pseudo-handle; four writable FILETIMEs.
    if unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) }.is_err() {
        return Duration::ZERO;
    }
    let ticks = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    Duration::from_nanos((ticks(k) + ticks(u)) * 100)
}

pub(crate) struct Seeder {
    me: u32,
    key_type: u16,
    file_type: u16,
    namer: FileNamer,
    counters: Arc<ServiceCounters>,
}

impl Seeder {
    /// `None` when seeding is unavailable: without `SeDebugPrivilege` the table
    /// shows no object addresses (§7.4).
    pub(crate) fn new(cfg: &ServiceConfig, counters: Arc<ServiceCounters>) -> Option<Self> {
        if !privilege::enable(privilege::DEBUG) {
            return None;
        }
        // The type indices come from one handle of each type the agent opens itself.
        let file = std::fs::File::open(std::env::current_exe().ok()?).ok()?;
        let mut key = HKEY::default();
        // SAFETY: opens a key; closed below.
        unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, w!("SOFTWARE"), None, KEY_READ, &mut key) }.ok().ok()?;
        let me = std::process::id();
        let t = table();
        // SAFETY: opened above.
        unsafe {
            let _ = RegCloseKey(key);
        }
        let t = t?;
        let own = |h: u64| t.iter().find(|e| e.pid == me && e.handle == h && e.object != 0).map(|e| e.type_index);
        use std::os::windows::io::AsRawHandle;
        let key_type = own(key.0 as u64)?;
        let file_type = own(file.as_raw_handle() as u64)?;
        Some(Seeder {
            me,
            key_type,
            file_type,
            namer: FileNamer::new(cfg.name_timeout, cfg.max_stuck_helpers),
            counters,
        })
    }

    /// Answers one `Request::Seed`. `None` if the table could not be read: no
    /// reply, and the waiting events run to their deadline.
    pub(crate) fn snapshot(&mut self, kind: HandleKind, asked: Vec<u64>) -> Option<Snapshot> {
        let c = &self.counters;
        let before = table()?;
        c.seeder_table_reads.fetch_add(1, Ordering::Relaxed);
        let type_index = match kind {
            HandleKind::Key => self.key_type,
            HandleKind::File => self.file_type,
        };
        let (holders, own_only) = plan(&before, type_index, self.me, &asked);
        let mut processes: HashMap<u32, Option<Owned>> = HashMap::new();
        let mut held: Vec<(Named, Owned)> = Vec::new();
        let mut unnamable: Vec<(u64, u32)> = own_only.into_iter().map(|a| (a, self.me)).collect();
        for (address, hs) in holders {
            let owner = hs[0].pid;
            if kind == HandleKind::File && self.namer.paused() {
                unnamable.push((address, owner));
                continue;
            }
            let mut answer = None;
            for e in &hs {
                let Some(p) = processes.entry(e.pid).or_insert_with(|| open_process(e.pid)).as_ref() else {
                    continue;
                };
                let Some(dup) = duplicate(p, e.handle) else { continue };
                answer = match kind {
                    HandleKind::Key => object_name(&dup).map(|n| (n, dup)),
                    HandleKind::File => match self.namer.name(dup) {
                        FileName::Named(n, dup) => Some((n, dup)),
                        FileName::Unnamable => None,
                        FileName::TimedOut => {
                            c.seeder_handles_timed_out.fetch_add(1, Ordering::Relaxed);
                            None
                        }
                    },
                };
                break;
            }
            match answer {
                Some((name, dup)) => held.push((Named { address, owner_pid: owner, name }, dup)),
                None => unnamable.push((address, owner)),
            }
        }
        c.seeder_stuck_helpers.store(self.namer.stuck() as u64, Ordering::Relaxed);
        let taken = qpc_now();
        let after = table()?;
        c.seeder_table_reads.fetch_add(1, Ordering::Relaxed);
        let ok = verified(&after, self.me, held.iter().map(|(n, d)| (n.address, d.raw().0 as u64)));
        let mut named = Vec::with_capacity(held.len());
        for (n, _dup) in held {
            if ok.contains(&n.address) {
                named.push(n);
            } else {
                unnamable.push((n.address, n.owner_pid));
            }
        }
        c.seeder_handles_named.fetch_add(named.len() as u64, Ordering::Relaxed);
        c.seeder_handles_failed.fetch_add(unnamable.len() as u64, Ordering::Relaxed);
        Some(Snapshot { kind, taken, asked, named, unnamable })
    }
}

/// The seeder thread: answers `Request::Seed`, below normal priority.
pub(crate) fn run(
    mut seeder: Seeder,
    jobs: Receiver<(HandleKind, Vec<u64>)>,
    replies: Sender<Reply>,
    cfg: &ServiceConfig,
) {
    // SAFETY: lowers this thread's priority.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    let counters = seeder.counters.clone();
    let mut budget = Budget::new(cfg.seeder_cpu, cfg.seeder_cpu_window);
    // Re-reads waiting for the budget, merged per kind.
    let mut waiting: BTreeMap<u8, BTreeSet<u64>> = BTreeMap::new();
    let kind_of = |k: u8| if k == 0 { HandleKind::Key } else { HandleKind::File };
    loop {
        let job = if waiting.is_empty() {
            jobs.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            jobs.recv_timeout(budget.retry_in(Instant::now()))
        };
        match job {
            Ok((kind, addresses)) if addresses.is_empty() => {
                // The start-up pass: at once, and not charged.
                if let Some(s) = seeder.snapshot(kind, Vec::new())
                    && replies.send(Reply::Snapshot(s)).is_err()
                {
                    return;
                }
            }
            Ok((kind, addresses)) => {
                let k = u8::from(kind == HandleKind::File);
                if !budget.allows(Instant::now()) {
                    counters.seeder_deferred_rereads.fetch_add(1, Ordering::Relaxed);
                }
                waiting.entry(k).or_default().extend(addresses);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while let Some(k) = waiting.keys().next().copied() {
            if !budget.allows(Instant::now()) {
                break;
            }
            let addresses: Vec<u64> = waiting.remove(&k).unwrap_or_default().into_iter().collect();
            let cpu = thread_cpu();
            let snapshot = seeder.snapshot(kind_of(k), addresses);
            budget.charge(Instant::now(), thread_cpu().saturating_sub(cpu));
            if let Some(s) = snapshot
                && replies.send(Reply::Snapshot(s)).is_err()
            {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(object: u64, pid: u32, handle: u64, type_index: u16) -> Entry {
        Entry { object, pid, handle, type_index }
    }

    const ME: u32 = 100;

    #[test]
    fn plan_groups_by_address_with_the_lowest_pid_first() {
        let t = [
            e(0xA, 900, 8, 1),
            e(0xA, 300, 4, 1),
            e(0xB, ME, 4, 1),
            e(0xC, ME, 8, 1),
            e(0xC, 50, 12, 1),
            e(0xD, 7, 4, 2), // another type
            e(0, 7, 8, 1),   // no address (unprivileged view)
        ];
        let (h, own) = plan(&t, 1, ME, &[]);
        assert_eq!(h.keys().copied().collect::<Vec<_>>(), [0xA, 0xC]);
        assert_eq!(h[&0xA].iter().map(|e| e.pid).collect::<Vec<_>>(), [300, 900]);
        assert_eq!(h[&0xC].iter().map(|e| e.pid).collect::<Vec<_>>(), [50], "the agent is never the owner");
        assert_eq!(own, [0xB], "held only by the agent");
        let (h, own) = plan(&t, 1, ME, &[0xC, 0xE]);
        assert_eq!(h.keys().copied().collect::<Vec<_>>(), [0xC]);
        assert!(own.is_empty());
    }

    #[test]
    fn verification_needs_our_duplicate_at_the_address() {
        let after = [e(0xA, ME, 0x40, 1), e(0xB, ME, 0x44, 1), e(0xC, 7, 0x48, 1)];
        let ok = verified(&after, ME, [(0xA, 0x40), (0xB, 0x48), (0xC, 0x48), (0xD, 0x4c)].into_iter());
        assert_eq!(ok, HashSet::from([0xA]));
    }

    #[test]
    fn budget_defers_until_charges_age_out() {
        let t = Instant::now();
        let mut b = Budget::new(Duration::from_millis(600), Duration::from_secs(60));
        assert!(b.allows(t));
        b.charge(t, Duration::from_millis(400));
        assert!(b.allows(t + Duration::from_secs(1)));
        b.charge(t + Duration::from_secs(1), Duration::from_millis(250));
        assert!(!b.allows(t + Duration::from_secs(2)), "650 ms spent of 600");
        assert_eq!(b.retry_in(t + Duration::from_secs(2)), Duration::from_secs(58));
        assert!(b.allows(t + Duration::from_secs(60)), "the first charge left the window");
        assert_eq!(Budget::new(Duration::ZERO, Duration::from_secs(1)).retry_in(t), Duration::from_millis(10));
    }

    /// Tests that need `SeDebugPrivilege`: `#[ignore]`d locally, run by CI's
    /// `agent-live` job and the elevated host script.
    mod elevated {
        use super::*;
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation};

        use super::super::super::util::{aligned, as_bytes};
        use super::super::super::value::tests::TestKey;

        fn seeder() -> Seeder {
            Seeder::new(&ServiceConfig::default(), Arc::new(ServiceCounters::default())).expect("SeDebugPrivilege")
        }

        fn object_of(h: HANDLE) -> u64 {
            let me = std::process::id();
            table().unwrap().iter().find(|e| e.pid == me && e.handle == h.0 as u64).expect("own handle").object
        }

        /// A child that inherits `handles` and sleeps; killed on drop.
        struct Holder(std::process::Child);

        impl Holder {
            fn new(handles: &[HANDLE]) -> Holder {
                for h in handles {
                    // SAFETY: test-only; marks our handle inheritable.
                    unsafe { SetHandleInformation(*h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }.unwrap();
                }
                // One process, no grandchildren that would inherit the handles too.
                let child = std::process::Command::new("ping.exe")
                    .args(["-n", "60", "127.0.0.1"])
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap();
                for h in handles {
                    // SAFETY: as above, undone.
                    unsafe { SetHandleInformation(*h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) }.unwrap();
                }
                std::thread::sleep(Duration::from_millis(300));
                Holder(child)
            }
        }

        impl Drop for Holder {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        #[test]
        #[ignore = "needs SeDebugPrivilege"]
        fn names_a_key_and_a_file_another_process_holds() {
            let k = TestKey::new("seeder");
            let path = std::env::temp_dir().join(format!("atlas-seeded-{}.txt", std::process::id()));
            let f = std::fs::File::create(&path).unwrap();
            let (kh, fh) = (HANDLE(k.hkey.0), HANDLE(f.as_raw_handle()));
            let holder = Holder::new(&[kh, fh]);
            let child = holder.0.id();
            let (ka, fa) = (object_of(kh), object_of(fh));
            let mut s = seeder();

            let snap = s.snapshot(HandleKind::Key, vec![ka, 0x1234]).unwrap();
            assert_eq!(snap.asked, [ka, 0x1234]);
            assert_eq!(snap.named, [Named { address: ka, owner_pid: child, name: k.path() }]);
            assert!(snap.unnamable.is_empty(), "0x1234 is in neither list: not in the table");

            let snap = s.snapshot(HandleKind::File, vec![fa]).unwrap();
            assert_eq!(snap.named.len(), 1, "{snap:?}");
            let n = &snap.named[0];
            assert_eq!((n.address, n.owner_pid), (fa, child));
            let file_name = path.file_name().unwrap().to_string_lossy().to_lowercase();
            assert!(n.name.starts_with(r"\Device\") && n.name.to_lowercase().ends_with(&file_name), "{}", n.name);
            assert!(snap.taken <= qpc_now());

            // Only the agent holds it once the child is gone.
            drop(holder);
            let snap = s.snapshot(HandleKind::Key, vec![ka]).unwrap();
            assert!(snap.named.is_empty());
            assert_eq!(snap.unnamable, [(ka, std::process::id())]);
            drop(f);
            std::fs::remove_file(&path).unwrap();
        }

        #[test]
        #[ignore = "needs SeDebugPrivilege"]
        fn start_up_pass_covers_the_table() {
            let mut s = seeder();
            let mut problems = Vec::new();
            for kind in [HandleKind::Key, HandleKind::File] {
                let t = Instant::now();
                let snap = s.snapshot(kind, Vec::new()).unwrap();
                let elapsed = t.elapsed();
                let mut seen = HashSet::new();
                for a in snap.named.iter().map(|n| n.address).chain(snap.unnamable.iter().map(|u| u.0)) {
                    assert!(seen.insert(a), "each address once: {a:#x}");
                }
                println!(
                    "{kind:?}: {} named, {} unnamable in {elapsed:?}; stuck helpers {}",
                    snap.named.len(),
                    snap.unnamable.len(),
                    s.counters.seeder_stuck_helpers.load(Ordering::Relaxed)
                );
                if snap.named.len() <= 100 {
                    problems.push(format!("{kind:?}: only {} named; a live system holds many", snap.named.len()));
                }
                // Keys: `\REGISTRY\…` or the root; files: `\Device\…`. Kernel names
                // keep their case: some keys are `\Registry\Machine\…` (plan 1b-3b,
                // finding F4), which `paths::registry` already matches.
                let well_formed = |n: &str| {
                    let n = n.to_ascii_uppercase();
                    match kind {
                        HandleKind::Key => n == r"\REGISTRY" || n.starts_with(r"\REGISTRY\"),
                        HandleKind::File => n.starts_with(r"\DEVICE\"),
                    }
                };
                let odd: Vec<&str> = snap.named.iter().map(|n| n.name.as_str()).filter(|n| !well_formed(n)).collect();
                if !odd.is_empty() {
                    problems.push(format!("{kind:?}: {} odd names, e.g. {:?}", odd.len(), &odd[..odd.len().min(5)]));
                }
            }
            assert!(problems.is_empty(), "{problems:#?}");
            assert_eq!(s.counters.seeder_table_reads.load(Ordering::Relaxed), 4, "two reads per snapshot");
        }

        /// D2: on a no-access duplicate, `ObjectNameInformation` gives the name
        /// `NtQueryKey(KeyNameInformation)` gives on a handle with access.
        #[test]
        #[ignore = "needs SeDebugPrivilege"]
        fn object_name_matches_key_name_information() {
            use windows::Wdk::System::Registry::{KeyNameInformation, NtQueryKey};
            use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
            use windows::Win32::System::Threading::GetCurrentProcess;
            let s = seeder();
            let t = table().unwrap();
            let me = std::process::id();
            let (mut compared, mut differ) = (0, Vec::new());
            for e in t.iter().filter(|e| e.type_index == s.key_type && e.pid != me && e.pid > 4).take(2000) {
                let Some(p) = open_process(e.pid) else { continue };
                let Some(zero) = duplicate(&p, e.handle) else { continue };
                let mut same = HANDLE::default();
                // SAFETY: test-only; a full-access copy for the reference query.
                let dup = unsafe {
                    DuplicateHandle(
                        p.raw(),
                        HANDLE(e.handle as *mut _),
                        GetCurrentProcess(),
                        &mut same,
                        0,
                        false,
                        DUPLICATE_SAME_ACCESS,
                    )
                };
                if dup.is_err() {
                    continue;
                }
                let same = Owned(same);
                let mut buf = aligned(4096);
                let mut ret = 0u32;
                let len = (buf.len() * 8) as u32;
                // SAFETY: test-only; aligned writable buffer.
                let st =
                    unsafe { NtQueryKey(same.raw(), KeyNameInformation, Some(buf.as_mut_ptr().cast()), len, &mut ret) };
                if st.is_err() {
                    continue;
                }
                let b = as_bytes(&buf);
                let n = u32::from_le_bytes(b[0..4].try_into().unwrap()) as usize;
                let units: Vec<u16> = b[4..4 + n].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                let reference = String::from_utf16_lossy(&units);
                compared += 1;
                let got = object_name(&zero);
                if got.as_deref() != Some(reference.as_str()) {
                    differ.push((reference, got));
                }
            }
            println!("compared {compared} key handles");
            assert!(compared > 100);
            assert!(differ.is_empty(), "{differ:?}");
        }
    }
}
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy -p atlas-agent --all-targets -- -D warnings -A dead-code
```
Expected: 136 tests pass, 5 ignored, clippy clean.
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): the seeder: no-access duplicates, verified snapshots, CPU budget"
```

### Task 6: `Services`, and the CI job

**Files:**
- Modify: `crates/atlas-agent/src/win/mod.rs` (`mod services;`, `pub use services::Services;`), `.github/workflows/ci.yml`
- Create: `crates/atlas-agent/src/win/services.rs`

- [ ] **Step 1: The lanes and routing**

`src/win/services.rs`:
```rust
//! The services' threads and the routing of [`Request`]s to them (sensor spec
//! §3.2 [4] and [8]; plan 1b-3b, D5).
//!
//! - **Hash workers** (2, below normal): `Enrich`.
//! - **Reader lane** (1): value reads on both paths, 8.3 expansion, and the
//!   expansion cache's invalidations, in arrival order (D4).
//! - **Seeder** (1, below normal): `Seed`, when `SeDebugPrivilege` is available.
//!
//! Lanes are bounded and never block the caller: a full lane drops the
//! request (counted), and its event waits out its deadline (clarification 25).
//! Replies come back on one channel, which the driver (plan 1b-3c) feeds to
//! `Pipeline::reply`.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};

use super::expand::{Expander, NtDir};
use super::hash::{Enricher, HashCache};
use super::seeder::{self, Seeder};
use super::{privilege, value};
use crate::config::ServiceConfig;
use crate::counters::ServiceCounters;
use crate::intake::FastRead;
use crate::services::{EarlyKey, HandleKind, Reply, Request, ValueRead};

enum ReaderJob {
    Read(crate::completion::PendingId, ValueRead),
    Early(EarlyKey, ValueRead),
    Expand(crate::completion::PendingId, u8, String),
    Invalidate(String),
}

struct HashJob {
    id: crate::completion::PendingId,
    nt_path: String,
}

pub struct Services {
    hash: SyncSender<HashJob>,
    cache: Arc<Mutex<HashCache>>,
    reader: SyncSender<ReaderJob>,
    seeder: Option<SyncSender<(HandleKind, Vec<u64>)>>,
    replies: Receiver<Reply>,
    counters: Arc<ServiceCounters>,
}

impl Services {
    /// Starts the threads. Enables `SeBackupPrivilege` for value reads and
    /// `SeDebugPrivilege` for the seeder; without them reads use a normal open
    /// and seeding is off ([`Services::seeding`]).
    pub fn start(cfg: &ServiceConfig) -> Services {
        let counters = Arc::new(ServiceCounters::default());
        let (reply_tx, replies) = channel();
        let cache = Arc::new(Mutex::new(HashCache::new(cfg.hash_cache_cap)));

        let (hash, hash_rx) = sync_channel::<HashJob>(cfg.lane_cap);
        let hash_rx = Arc::new(Mutex::new(hash_rx));
        for i in 0..cfg.hash_workers.max(1) {
            let (rx, tx, cache, counters, cap) =
                (hash_rx.clone(), reply_tx.clone(), cache.clone(), counters.clone(), cfg.hash_size_cap);
            std::thread::Builder::new()
                .name(format!("atlas-hash-{i}"))
                .spawn(move || hash_worker(&rx, &tx, &cache, &counters, cap))
                .expect("spawn a hash worker");
        }

        let backup = privilege::enable(privilege::BACKUP);
        let (reader, reader_rx) = sync_channel::<ReaderJob>(cfg.lane_cap);
        let tx = reply_tx.clone();
        std::thread::Builder::new()
            .name("atlas-reader".into())
            .spawn(move || reader_lane(reader_rx, &tx, backup))
            .expect("spawn the reader lane");

        let seeder = Seeder::new(cfg, counters.clone()).map(|s| {
            let (seed_tx, seed_rx) = sync_channel(cfg.lane_cap);
            let (tx, cfg) = (reply_tx.clone(), cfg.clone());
            std::thread::Builder::new()
                .name("atlas-seeder".into())
                .spawn(move || seeder::run(s, seed_rx, tx, &cfg))
                .expect("spawn the seeder");
            seed_tx
        });

        Services { hash, cache, reader, seeder, replies, counters }
    }

    /// Whether the seeder runs. If not, the driver sets `Config::seed_on_start`
    /// and `seed_on_miss` to false (§7.4) and Sensor Health reports it.
    pub fn seeding(&self) -> bool {
        self.seeder.is_some()
    }

    pub fn counters(&self) -> Arc<ServiceCounters> {
        self.counters.clone()
    }

    /// The replies so far, without waiting.
    pub fn replies(&self) -> impl Iterator<Item = Reply> + '_ {
        self.replies.try_iter()
    }

    /// Hands a request to its lane. Never blocks.
    pub fn submit(&self, r: Request) {
        let sent = match r {
            Request::Enrich { id, nt_path, .. } => ok(self.hash.try_send(HashJob { id, nt_path })),
            Request::InvalidateHash { nt_path } => {
                self.cache.lock().expect("hash cache").invalidate(&nt_path);
                ok(self.reader.try_send(ReaderJob::Invalidate(nt_path)))
            }
            Request::ReadValue { id, read } => ok(self.reader.try_send(ReaderJob::Read(id, read))),
            Request::Expand { id, slot, nt_path } => ok(self.reader.try_send(ReaderJob::Expand(id, slot, nt_path))),
            Request::Seed { kind, addresses } => match &self.seeder {
                Some(s) => ok(s.try_send((kind, addresses))),
                None => true,
            },
        };
        if !sent {
            self.counters.service_queue_drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The fast path's value reads (§7.5), for Session A's `Intake`. Called
    /// from the ETW callback: never blocks.
    pub fn fast_read(&self) -> FastRead {
        let (tx, counters) = (self.reader.clone(), self.counters.clone());
        Box::new(move |key, read| {
            if !ok(tx.try_send(ReaderJob::Early(key, read))) {
                counters.service_queue_drops.fetch_add(1, Ordering::Relaxed);
            }
        })
    }
}

fn ok<T>(r: Result<(), TrySendError<T>>) -> bool {
    r.is_ok()
}

fn hash_worker(
    jobs: &Mutex<Receiver<HashJob>>,
    replies: &Sender<Reply>,
    cache: &Mutex<HashCache>,
    counters: &ServiceCounters,
    size_cap: u64,
) {
    // SAFETY: lowers this thread's priority.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    let enricher = Enricher::new(size_cap);
    loop {
        let job = match jobs.lock().expect("hash lane").recv() {
            Ok(j) => j,
            Err(_) => return,
        };
        let r = enricher.enrich(&job.nt_path, cache);
        counters.hash_cache_evictions.store(cache.lock().expect("hash cache").evictions, Ordering::Relaxed);
        let reply = Reply::Enriched { id: job.id, hashes: r.hashes, signature: r.signature, error: r.error };
        if replies.send(reply).is_err() {
            return;
        }
    }
}

fn reader_lane(jobs: Receiver<ReaderJob>, replies: &Sender<Reply>, backup: bool) {
    let mut expander = Expander::new(NtDir);
    for job in jobs {
        let reply = match job {
            ReaderJob::Read(id, read) => Reply::ValueRead { id, result: value::read(&read, backup) },
            ReaderJob::Early(event, read) => {
                let result = value::read(&read, backup);
                Reply::EarlyRead { event, read, result }
            }
            ReaderJob::Expand(id, slot, nt) => {
                Reply::Expanded { id, slot, long_path: expander.expand(&nt, Instant::now()) }
            }
            ReaderJob::Invalidate(nt) => {
                expander.invalidate(&nt);
                continue;
            }
        };
        if replies.send(reply).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::EnrichTarget;
    use std::time::Duration;
    use windows::Win32::System::Registry::REG_DWORD;

    use super::super::value::tests::{TestKey, units};

    fn wait(s: &Services, n: usize) -> Vec<Reply> {
        let until = Instant::now() + Duration::from_secs(10);
        let mut out = Vec::new();
        while out.len() < n && Instant::now() < until {
            out.extend(s.replies());
            std::thread::sleep(Duration::from_millis(5));
        }
        out
    }

    fn nt(dos: &str) -> String {
        let (dev, drive) =
            super::super::lookups::query_drives().into_iter().find(|(_, d)| dos[..2].eq_ignore_ascii_case(d)).unwrap();
        format!("{dev}{}", &dos[drive.len()..])
    }

    #[test]
    fn every_request_kind_is_answered() {
        let s = Services::start(&ServiceConfig::default());
        let k = TestKey::new("services");
        k.set(k.hkey, &units("v"), REG_DWORD.0, &9u32.to_le_bytes());
        let read = ValueRead { key_path: k.path(), value_name: units("v") };
        let notepad = nt(&format!(r"{}\System32\notepad.exe", std::env::var("SystemRoot").unwrap()));
        s.submit(Request::Enrich { id: 1, target: EnrichTarget::LaunchImage, nt_path: notepad.clone() });
        s.submit(Request::ReadValue { id: 2, read: read.clone() });
        s.submit(Request::Expand { id: 3, slot: 1, nt_path: notepad.to_uppercase() });
        let key = EarlyKey { ts: 5, tid: 6, key_object: 7 };
        (s.fast_read())(key, read.clone());
        let mut replies = wait(&s, 4);
        replies.sort_by_key(|r| format!("{r:?}").chars().take(8).collect::<String>());
        assert_eq!(replies.len(), 4, "{replies:?}");
        let nine =
            Some(crate::services::ValueData { value_type: REG_DWORD.0, size: 4, data: 9u32.to_le_bytes().to_vec() });
        for r in replies {
            match r {
                Reply::Enriched { id, hashes, signature, error } => {
                    assert_eq!(id, 1);
                    assert!(hashes.is_some() && !error);
                    assert_eq!(signature.map(|s| s.status), Some(atlas_schema::SignatureStatus::Valid));
                }
                Reply::ValueRead { id, result } => assert_eq!((id, result), (2, nine.clone())),
                Reply::Expanded { id, slot, long_path } => {
                    assert_eq!((id, slot), (3, 1));
                    assert_eq!(long_path, Some(notepad.to_uppercase()), "no short components: unchanged");
                }
                Reply::EarlyRead { event, read: r, result } => {
                    assert_eq!((event, r, result), (key, read.clone(), nine.clone()));
                }
                Reply::Snapshot(_) => panic!("no seed was asked"),
            }
        }
        assert_eq!(s.counters().service_queue_drops.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn full_lanes_drop_and_count() {
        let cfg = ServiceConfig { lane_cap: 1, hash_workers: 1, ..ServiceConfig::default() };
        let s = Services::start(&cfg);
        let big = nt(&format!(r"{}\System32\ntoskrnl.exe", std::env::var("SystemRoot").unwrap()));
        for id in 0..50 {
            s.submit(Request::Enrich { id, target: EnrichTarget::Module, nt_path: big.clone() });
        }
        let drops = s.counters().service_queue_drops.load(Ordering::Relaxed);
        assert!(drops > 0, "a lane of one cannot take 50 at once");
        let answered = wait(&s, (50 - drops) as usize).len() as u64;
        assert_eq!(answered + drops, 50, "each request is answered or counted, never both");
    }

    #[test]
    fn seeding_needs_the_debug_privilege() {
        let s = Services::start(&ServiceConfig::default());
        assert_eq!(s.seeding(), privilege::enable(privilege::DEBUG));
        if !s.seeding() {
            s.submit(Request::Seed { kind: HandleKind::Key, addresses: vec![] });
            std::thread::sleep(Duration::from_millis(200));
            assert_eq!(s.replies().count(), 0);
            assert_eq!(s.counters().service_queue_drops.load(Ordering::Relaxed), 0);
        }
    }
}
```

- [ ] **Step 2: The `agent-live` job**

`.github/workflows/ci.yml`:
```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -83,6 +83,24 @@ jobs:
           path: ${{ runner.temp }}/etw/
           if-no-files-found: ignore
 
+  # Tier 3 for atlas-agent's Windows services (plan 1b-3b): the tests that need
+  # SeDebugPrivilege or SeBackupPrivilege (seeding from the handle table, value
+  # reads with backup semantics, the System process's creation time). They are
+  # #[ignore]d locally; the hosted runner's account is an administrator. Plan
+  # 1b-3c adds the agent-level live test here.
+  agent-live:
+    runs-on: windows-latest
+    steps:
+      - uses: actions/checkout@v7
+      - name: Install Rust ${{ env.RUST_TOOLCHAIN }}
+        shell: bash
+        run: |
+          rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal
+          rustup default "$RUST_TOOLCHAIN"
+      - uses: Swatinem/rust-cache@v2
+      - name: Elevated service tests
+        run: cargo test -p atlas-agent --lib -- --ignored --test-threads=1 --nocapture
+
   proto:
     runs-on: ubuntu-latest
     steps:
```

- [ ] **Step 3: Check and commit**

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p atlas-agent -p atlas-etw --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
cargo test --workspace
actionlint .github/workflows/ci.yml
```
Expected: clean, with no `dead_code` allowance; 322 tests pass and 6 are ignored (139 and 5 in `atlas-agent`).
```powershell
git add crates/atlas-agent .github
git commit -m "feat(agent): Services: hash workers, reader lane and seeder behind bounded lanes; agent-live CI job"
```

### Task 7: The elevated run on the host (the user)

The `#[ignore]`d tests need `SeDebugPrivilege` or `SeBackupPrivilege`. Claude writes the script; the user runs it in an elevated PowerShell, because Claude never runs elevated on the host.

- [ ] **Step 1: The script** (git-ignored, in `spikes/`). It runs on the host, and changes nothing lasting: volatile `HKCU\Software\AtlasTest-*` keys and temp files that the tests delete, a short-lived `ping.exe` child, and build output.

`spikes/run-1b3b-elevated.ps1`:
```powershell
# Plan 1b-3b, verify-first: atlas-agent's Windows-service tests in an elevated
# shell, including the #[ignore]d ones that need SeDebugPrivilege/SeBackupPrivilege.
# Runs on the HOST. Changes nothing lasting: volatile HKCU test keys and temp
# files (deleted by the tests), a short-lived `cmd /c ping` child, and build
# output in the repo's target directory.
# Run from an elevated PowerShell (Windows PowerShell 5.1 or pwsh 7):
#   & C:\Users\jakef\Desktop\atlas-edr\spikes\run-1b3b-elevated.ps1

$ErrorActionPreference = 'Stop'
try { Stop-Transcript | Out-Null } catch { }

$wt = 'C:\Users\jakef\Desktop\atlas-edr'
$out = 'C:\Users\jakef\Desktop\atlas-edr\spikes\results\1b3b-elevated'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
New-Item -ItemType Directory -Force -Path $out | Out-Null
Start-Transcript -Path (Join-Path $out "transcript-$stamp.txt") | Out-Null

try {
    $principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Not elevated: open PowerShell with "Run as administrator" and run this line again.'
    }
    if (-not (Test-Path (Join-Path $wt 'Cargo.toml'))) { throw "Worktree not found: $wt" }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'cargo is not on PATH in this window.' }

    Write-Host "Windows build: $([Environment]::OSVersion.Version)"
    Set-Location $wt
    $log = Join-Path $out "cargo-$stamp.txt"
    # cargo reports progress on stderr; don't let PowerShell treat it as an error.
    $ErrorActionPreference = 'Continue'
    & cargo test -p atlas-agent --lib -- --include-ignored --test-threads=1 --nocapture 2>&1 |
        ForEach-Object { $line = "$_"; Write-Host $line; Add-Content -Path $log -Value $line -Encoding UTF8 }
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'

    $summary = Select-String -Path $log -Pattern '^test result:' | Select-Object -Last 1
    if ($code -ne 0) { throw "cargo test failed (exit $code). Log: $log" }
    if (-not $summary) { throw "No test summary in $log" }
    if ($summary.Line -notmatch ' 0 ignored') { throw "Some tests were still ignored: $($summary.Line)" }
    Write-Host "PASS: $($summary.Line)" -ForegroundColor Green
}
catch {
    Write-Host "FAIL: $_" -ForegroundColor Red
}
finally {
    Stop-Transcript | Out-Null
    Write-Host "Results in $out"
}
```

Check that it parses in both `powershell.exe` and `pwsh` before handing it over.

- [ ] **Step 2: The user runs it** from an elevated PowerShell: `& C:\Users\jakef\Desktop\atlas-edr\spikes\run-1b3b-elevated.ps1`.

Expected: `PASS: test result: ok. 144 passed; 0 failed; 0 ignored`, and in the log the start-up pass counts for keys and files and `compared N key handles` (N > 100, no differences). Claude reads the log from `spikes\results\1b3b-elevated\` and records the numbers in the PR.

### Task 8: Documentation

- [ ] **Step 1: Spec, schema reference, decision log**

In `docs/specs/2026-10-01-etw-sensor-design.md`:
- Status line: plan 1b-3b done; 1b-3c next.
- §3.2: clarification 11. §5.3: clarification 8. §5.5: clarification 9.
- §6.1: clarifications 5 and 6. §6.3: clarification 7.
- §7.2: clarification 3. §7.4: clarifications 2 and 4. §7.5: clarification 10.
- §9.3, §10.3: clarification 12. §12.3: clarification 13. §16: clarification 14.
- §15.3: a "Plan 1b-3b (2026-10-06)" addendum with F1–F7 and the elevated run's numbers.
- §17: a "Plan 1b-3b clarifications" paragraph listing 1–14.

In `docs/schema-reference.md`, Sensor Health: `housekeeping` gains "service queue drops".

In `docs/architecture-overview.md`:
- Roadmap row 1: "plans 1b-1, 1b-2, 1b-3a and 1b-3b done; plan 1b-3c (driver and agent-level live test) next", with a link to this plan.
- Decision log: the row for this plan's approval (written when it is approved) and a row for the build.

```powershell
git add docs
git commit -m "docs(1): plan 1b-3b clarifications, findings and schema reference"
```

### Task 9: PR and merge

- [ ] **Step 1:** Push `feat/1b-3b-services` and open the PR. The body summarises the services, D1–D5, F1–F7, the test counts and the elevated run's numbers, and ends with the attribution line.
- [ ] **Step 2:** CI runs `rust-linux`, `rust-windows`, `etw-live`, `agent-live`, `proto` (with `buf breaking`), `powershell` and `audit`. This is the first run on the runner's build (26100): if a test fails only there (the `KUSER_SHARED_DATA` BootId offset, an embedded-signed binary missing, 8.3 names off on its temp volume), fix it with a test, never by skipping.
- [ ] **Step 3:** When every check on the PR's current head is green, merge, then update the decision log's build row if anything changed on the way.

## Review Log

(Filled in after the independent review.)
