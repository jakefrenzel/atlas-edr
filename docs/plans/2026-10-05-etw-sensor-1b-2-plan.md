# Sub-project 1b-2 — `atlas-etw` Implementation Plan

> **Status:** Approved 2026-10-05, after an independent review (no blockers; 3 major and 10 minor findings folded in, Review Log at the end); the user chose D1 A and D2 A. **For agentic workers:** steps use checkbox (`- [ ]`) syntax for tracking. Tasks 1–4 and 6–7 need no elevation. Task 5's live test needs administrator rights: CI runs it on its Windows runner, and on the host only the user runs it, from an elevated window (plan 1a decision D2). Nothing here touches the VM or a kernel driver.

**Goal:** Build `atlas-etw`, the sensor's ETW crate (sensor spec §3.1, §4):
- hand-written, portable parsers for every event the sensor consumes;
- the Windows session layer that starts, enables, queries, consumes and stops the two sessions;
- TDH as an oracle: a runtime check for newer event versions, and field-by-field comparison in tests;
- replay fixtures, a live test on real sessions, two fuzz targets and the CI wiring (§12).

Plans 1b-3 (pipeline) and 1b-4 (health, service, CLI) build on this crate.

**Architecture:**
- **`parse`** (portable, `#![forbid(unsafe_code)]`). `parse(&EventMeta, &[u8]) -> Result<RawEvent, ParseError>`.
  - One hand-written parser per (provider, event, version), driven by a bounds-checked `Reader`.
  - Pointer size comes from each event's header.
  - Strings stay UTF-16 (`WStr`) until the agent emits an event.
  - Registry names are read as counted strings, so embedded NULs survive (finding F4).
  - `parse_query_results` turns DNS 3008's `QueryResults` into answers.
- **`layout`** (portable). The manifest layout (field names and TDH in-types) of every version we parse. It is the single source for three checks:
  - the manifest test, which compares it with the installed manifests, without elevation;
  - the runtime check of newer versions (`is_prefix`, §4.3);
  - the live test's layout comparison.
- **`providers`** (portable). The providers, their GUIDs, and Session A's keywords and event-ID allow-lists (§4.2).
- **`session`** (Windows only). All of the sensor's ETW `unsafe` code, behind a safe API:
  - `Session` (start, enable, disable, query, flush, stop; stopped on drop);
  - `query_by_name` and `stop_by_name`;
  - `provider_state` (§9.1);
  - `consume`, a real-time consumer thread calling a closure with an `EventRecord`, whose accessors include the start key;
  - `VersionGate`, which asks TDH once per newer version;
  - `tdh`: the oracle's decode, layout lookups, and offline lookups from a rebuilt record.
- **Tests**:
  - Tier 1: unit and property tests.
  - Tier 2: `tests/replay.rs` replays text fixtures (decision D2) and compares every field with the TDH decoding stored at capture time. It is portable.
  - Tier 3: `tests/live.rs`, real sessions. The test runs itself a second time as an *actor* process that performs the scenario, so the observer's own TDH lookups stay out of what it records.
  - `tests/manifest.rs` (Windows, unelevated): the layout table against the installed manifests and the classic MOF class.

**Tech stack:** Rust 1.97 (edition 2024).
- New: `windows` 0.62.2 (Microsoft, MIT/Apache-2.0). Library features: `Win32_Foundation`, `Win32_System_Diagnostics_Etw`, `Win32_System_Time`. The live test adds `Wdk_Foundation`, `Wdk_System_Registry`, `Win32_NetworkManagement_Dns`, `Win32_Security`, `Win32_Storage_FileSystem`, `Win32_System_Performance`, `Win32_System_Registry` and `Win32_System_Threading`.
- Existing: `proptest` 1.11, `serde_json` 1 (dev), `libfuzzer-sys` 0.4.
- CI tools: `actionlint` 1.7.12.

**Spec:** `docs/specs/2026-10-01-etw-sensor-design.md`, revision 3. Section numbers (§) refer to it.

**Verification note (2026-10-05, host, Windows 11 build 26200):** every code block below was compiled and run in a scratch worktree of `main` (86a3b06).
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean. `atlas-etw` is also clippy-clean for `x86_64-unknown-linux-gnu`, checked from Windows.
- `cargo test --workspace`: 179 pass and 1 is ignored (the live test). The manifest tests ran unelevated.
- **Replay against local recordings,** which are never committed (§4.3): every event agreed with TDH field by field.
  - The spike recordings: 6,407 events of 23 kinds.
  - The host live runs below: 15,502 events (run 1), 1,235 (run 3) and 1,264 (run 4), covering every kind in the layout table.
  - Run 4's recording also passes the strict replay that the committed fixtures get (Task 4), with every kind present.
- **The live test, elevated on the host** (`spikes\run-1b2-live.ps1`, run by the user; results in the git-ignored `spikes\results\1b2*`):
  - Run 1: 37 of 38 checks passed. It found the classic `ExitStatus` in-type (`Int32`), and a feedback loop that led to the observer/actor split.
  - Run 2 stopped in the test harness: the actor's result line was printed mid-line.
  - Run 3: 41 of 42 passed. The one failure was in the oracle comparison (TDH stops a string at its first NUL). It was fixed, and the run's events were re-verified by replay.
  - Run 4, after the review's fixes (Review Log): **all 45 checks passed** with the code in this plan. That includes the new checks for the event-ID filter, a stale session, the QPC frequency, and TDH decode failures.
- **Not run yet:**
  - `cargo fuzz`, which needs nightly on Linux; it first runs on the build PR.
  - The Linux build of the fuzz crate and of `alloca`, whose build scripts need a Linux C compiler.
  - `cargo audit`, which CI runs.
  - CI's `etw-live` job, which first runs on the build PR (Task 7).
- `actionlint` passes on both workflows. The fuzz job's selection of targets for a PR was simulated with five changed-file lists.

## Global Constraints

- **Branches.** This plan is reviewed on `docs/1b-2-plan`. The build runs on a new branch, `feat/1b-2-atlas-etw`, created from `main` after this plan merges. Never commit to `main`.
- **`unsafe` lives only in `session`.** `parse`, `layout` and `providers` carry `#![forbid(unsafe_code)]`. Every `unsafe` block has a `// SAFETY:` comment.
- **Parsers never panic** on any input: every read is bounds-checked and returns a `ParseError` (§4.3). The property tests and `parse_any` check this.
- **Elevation.** Claude never runs anything elevated on the host. Steps that need administrator rights run in CI, or the user runs them with one pasted line.
- **Formatting and lint:** the repo's `rustfmt.toml`. Each commit passes `cargo fmt --all --check`. Clippy with `-D warnings` must pass at the end of every task.
- **Shell:** commands are given for PowerShell 7. `cargo` commands are the same in bash.
- **Line endings:** `core.autocrlf` is on. The parsers and the fixture reader accept CRLF (`str::lines`).
- Every commit message ends with the attribution lines the session's system reminder specifies.

## Decisions (chosen 2026-10-05: D1 A, D2 A)

### D1, how plan 1b-2's code is verified before the plan is written

Plan 1b-1's code was compiled and run before the plan. Here the parsers can be verified unelevated, but starting sessions cannot, and that code holds the crate's riskiest `unsafe`.
- **(A) Chosen: verify unelevated in a scratch worktree, plus one elevated host run by the user.** The parsers were checked against the spike recordings, which stay local. A script ran the live test (sessions for about 15 s, a scenario, a comparison with TDH). It also answered the open questions, findings F1–F9. In practice it took four runs; see the verification note.
- (B) Unelevated only; the session code would first run in CI on the build PR.
- (C) Push a scratch branch so the CI runner verifies during planning: an outward-facing push before the plan exists.

### D2, the form of the replay fixtures (refines §12.2)

§12.2 described `.etl` files replayed through `OpenTrace` on a file, with TDH run at test time. A session records the whole machine, so an `.etl` from even an idle runner is megabytes of binary nobody can review.
- **(A) Chosen: text fixtures filtered at capture.** The live test keeps only the actor's process tree, plus the observer's own classic rundown events. It writes one JSON line per event: header fields, start key, payload hex, and TDH's decoding *on the recording machine*.
  - Replay compares every parsed field with that stored decoding, on Linux too.
  - The real consumer is still exercised by the live test. The host recording is about 1,200 events, 0.5 MB.
- (B) `.etl` filtered with the `ITraceRelogger` COM API: binary fixtures, Windows-only replay, and more `unsafe` (COM callbacks).
- (C) Unfiltered `.etl`: megabytes of everything the runner did.

Also verified (GitHub documentation, 2026-10-05): a `workflow_dispatch` workflow runs only if its file is on the default branch. So the recording comes from the PR's own CI run: the `etw-live` job uploads it as an artifact (Task 7).

## Findings from the live runs (host, build 26200, 2026-10-05)

These are facts for plans 1b-3 and 1b-4. Task 6 records them in spec §15.3 as a "Plan 1b-2" addendum.

- **F1, `CloseKey` fires only when the last handle closes** (it closes the key object). Closing a duplicated handle logged nothing. Closing the original logged one CloseKey (§7.4's open question). The rules in §7.4 hold either way.
- **F2, `Microsoft-Windows-Kernel-EventTracing` reports changes to our session** ({B675EC37-BDB6-4648-BC92-F3FDC74D3CA2}). Event 14 is logged on every enable, including a re-apply with the same settings and a change of event-ID filter. It carries the session name, provider, level, keywords and enable property, but not the filter. Event 15 is logged on a disable; events 8 and 11 on a session stop. The provider also logs other processes' provider activity (events 8, 9 and 29 by the dozen), so a watchdog filters by session name.
  - The header PID and start key are the **caller's**, so the process that changed our session can be named.
  - This answers the gap in §9.1: `TraceGuidQueryInfo` cannot return the event-ID filter (finding F3). Input for plan 1b-4.
- **F3, the event-ID filter cannot be read back.** `TraceGuidQueryInfo` returns level, keywords, enable property and LoggerId only, and Windows documents no other query. A narrowed filter left `provider_state` unchanged.
  - ETW also adds bits we did not ask for: we enabled property 0x80 and it reported 0xC0.
  - For Kernel-Process, Kernel-EventTracing reported keywords 0x850, level 255 and property 0x3C1, where we asked for 0x50, 5 and 0x80.
  - So the watchdog must check that our bits are *present*, not that the values are equal.
  - Kernel-File showed three enable entries for our session.
- **F4, registry names are counted strings.**
  - A value name with an embedded NUL (`a`, NUL, `b`, set with `NtSetValueKey`) is logged whole, followed by a terminator. This is a known way to hide Run-key values.
  - The same holds for `DeleteValueKey` and for a key created with `NtCreateKey`. TDH stops at the first NUL (`a`), and cannot decode the SetValueKey event at all.
  - The parser keeps the whole name: clarification 5.
- **F5, which end `saddr` is (§7.3).** `saddr`/`sport` is the local end for TCP connect, TCP accept and UDP send. For **UDP receive** it is the remote sender, and `daddr`/`dport` is the local end. The ports confirmed it for IPv4 and IPv6. Plan 1b-3's flow table keys on this.
- **F6, the classic process class.** `ExitStatus` is a signed `Int32`. Session B also logs opcode 11 (version 2, `ProcessId` only) and version 5 opcode 39 events; we do not parse them (`UnknownEvent`, not counted as errors).
- **F7, elevation.** Unelevated, `StartTraceW` succeeded, but enabling a kernel provider returned error 5 (access denied).
- **F8, TDH works offline.** It describes manifest and classic MOF events from a record built by hand, with no session and no elevation. The manifest test and the gate tests use this.
- **F9, other observations.**
  - The two halves of a Launch arrived 14–15 µs apart (§5.2).
  - With the allow-lists in place, no event of an ID we did not request reached Session A, for example Kernel-Process 6 (ImageUnload) under the enabled IMAGE keyword. So the filter works as §4.2 assumes.
  - DNS-Client had 75 registered instances, and our session's enable reached each one.
  - `ProcessTrace` returned 0 after both a normal and an external stop, and no events were lost.

## Deliberate clarifications of the spec (applied to the spec text in Task 6)

1. **Replay fixtures** are text, filtered at capture and recorded by CI's `etw-live` job (D2; refines §12.2). Replay through the full pipeline comes with plan 1b-3, where the pipeline exists.
2. **The crate boundary** (refines §3.1, §3.2 [1]).
   - `atlas-etw` provides parsing and the session primitives.
   - What the callback *does* is plan 1b-3's: the queues, the DNS rate limit, the OperationEnd failure filter and the early registry key map. The agent passes it to `consume` as a closure.
   - The watchdog's ETW primitives are here: `query_by_name`, `provider_state`, `stop_by_name`, `Session::disable`. The blinding-test helper that disables a provider in *another* process's session waits for plan 1b-4, where it can be tested.
3. **Versions parsed** (§4.3).

   | Provider | Versions |
   |---|---|
   | Kernel-Process | ProcessStart 3 and 4; ProcessStop 2; ImageLoad 0 |
   | Kernel-File | 1 (OperationEnd 0) |
   | Kernel-Registry | 0 |
   | Kernel-Network | 0 |
   | DNS-Client | 3008 v0 |
   | Classic process | opcodes 1–4, v4 |

   A version older than these is `UnsupportedVersion`, and the agent counts it as `unknown_version`; ProcessStart before v3 has no sequence number, so no uid could be computed. Newer versions go through the prefix check. **Windows 10's versions are unverified:** there is no Windows 10 machine. Running the manifest test there would show any difference.
4. **The layout table is checked against Windows** (refines §4.3). `tests/manifest.rs` compares every manifest layout with the installed manifest, and the classic layout with its MOF class (F8). The live test compares the layout of every captured event.
5. **Counted registry names** (refines §7.5, which already reads values "by the name as counted in the event").
   - In the exact version, `ValueName` (SetValueKey) ends at the last NUL after which the remaining fields parse exactly to the end of the payload.
   - `ValueName` (DeleteValueKey) and `RelativeName` (CreateKey, OpenKey), the last fields, run to the final terminator.
   - A newer version whose installed layout only starts with ours falls back to the first NUL, because it may append fields. One whose layout equals ours stays exact (clarification 11).
   - SetValueKey's choice is unique while `CapturedData` and `PreviousData` are empty, as S4 found. If Windows ever fills them, a NUL inside them could also fit. The parser then still takes the last fit, but sets `value_name_ambiguous`; the agent treats the name, and a value read by it, as unreliable.
6. **CloseKey is verified** (§7.4): it fires on the last handle close (F1). This replaces the note that "plan 1b-2 verifies" it.
7. **Address ends** (§7.3): F5.
8. **The provider check** (§9.1) cannot see the event-ID filter (F3). It compares our bits as a subset. Kernel-EventTracing (F2) is the candidate for detecting changes; plan 1b-4 decides.
9. **Tier 3 is split** (refines §12.3). `atlas-etw`'s live test covers the ETW-level scenario: process, files, failed operations, delete-on-close, registry including embedded NULs and the CloseKey handle test, TCP and UDP over IPv4 and IPv6, and DNS. Tier-3 tests of the agent (watchlist and 8.3, undelete, seeding, value reads) come with plans 1b-3 and 1b-4. Test sessions are named `Atlas-Test-*`, so a test never touches the agent's sessions.
10. **A `Session` handle can go stale** (new; for §4.1 and §9.1). The handle is the LoggerId, which Windows reuses once the session stops. So every call first checks that the session with our name still has our LoggerId. Otherwise it does nothing and returns `STALE`, and dropping a stale `Session` stops nothing. After an external stop, the agent starts a new `Session` and `abandon()`s the old one.
11. **The version check reads the installed description, never the event** (refines §4.3).
    - It uses the manifest, or Session B's MOF class. A forged user-mode event (DNS-Client, §4.4) could carry its own schema and so decide the cached verdict for every later genuine event.
    - "Strict prefix" in §4.3 becomes "prefix or equal". An equal layout (only the number changed) is parsed exactly as ours; a strict prefix is parsed without the counted-name reading of the last field.
12. **A failed OperationEnd** (§5.5) is a status with **error** severity: its top two bits are set. That is narrower than `!NT_SUCCESS`, which also counts warnings such as `STATUS_BUFFER_OVERFLOW`; §5.5's wording is corrected.

## Review Focus

These would slip past a plain unit test, so each has a pinned check:
1. **An embedded NUL in a registry name** must not misalign the fields after it. See `set_value_with_an_embedded_nul_in_the_value_name`, which uses a host-recorded payload, and `last_field_names_keep_embedded_nuls`. The live checks `embedded_nul_*` cover the real kernel.
2. **A payload cut at any byte** is an error, not a panic or a wrong value. See `every_layout_parses_and_every_strict_prefix_fails` (both pointer sizes, every layout), the property tests, and `parse_any`.
3. **A newer version** parses only if TDH confirms our layout is a prefix, and the verdict is cached. See the gate tests (rejection, offline) and `layout::tests` (`is_prefix`).
4. **A panic in the agent's callback** must not unwind into ETW. The trampoline catches it and counts it (`Consumer::panics`).
5. **The consumer's context** must stay alive until `ProcessTrace` returns, and the trace handle must be closed exactly once. See `consume` and `Consumer::close_inner`. The live test stops one session from outside and one normally.
6. **Network events** are attributed by payload PID, never header PID (§5.3). The live test's tree filter does the same.
7. **The fixtures leak nothing from the host.** Committed fixtures come only from the CI runner (Task 7). Host recordings stay in the git-ignored `spikes\`.
8. **A stale `Session` never acts on someone else's session** (clarification 10). See the live check `an_externally_stopped_session_is_stale`.
9. **A forged event cannot decide a version verdict** (clarification 11). `VersionGate` never reads the record's schema. See `the_verdict_uses_the_installed_layout`, which uses a real installed pair, and the rejection test.
10. **Replay cannot pass by checking nothing.** A committed fixture line that does not read fails, and so do fixture files that yield no events.

## File Structure

```
Cargo.toml                                   + windows in [workspace.dependencies]
crates/atlas-etw/                            NEW crate
  Cargo.toml
  src/lib.rs
  src/providers.rs                           Provider, Enable, session_a(udp), LEVEL
  src/layout.rs                              InType, Layout, LAYOUTS, find, newest, is_prefix
  src/parse/mod.rs                           EventMeta, PointerSize, ParseError, WStr, RawEvent + payload structs, parse, parse_as
  src/parse/reader.rs                        Reader (bounds-checked), Sid
  src/parse/dns.rs                           DnsAnswer, DnsAnswers, parse_query_results
  src/parse/tests.rs                         unit and property tests
  src/session/mod.rs                         EtwError, Kind, Config, SessionInfo, Session, query_by_name, stop_by_name, ProviderEnable, provider_state
  src/session/consumer.rs                    EventRecord, Consumer, consume
  src/session/gate.rs                        VersionGate
  src/session/tdh.rs                         manifest_layout, classic_layout, event_layout, decode
  tests/common/mod.rs                        fixture lines, TDH oracle comparison
  tests/replay.rs                            tier 2
  tests/manifest.rs                          layout table vs installed manifests (Windows)
  tests/live.rs                              tier 3 (Windows, elevated, #[ignore])
  tests/fixtures/scenario.jsonl              recorded by CI (Task 7)
  fuzz/                                      own workspace: parse_any, dns_query_results
.github/workflows/ci.yml                     + etw-live job; etw fuzz crate check and audit
.github/workflows/fuzz.yml                   + parse_any, dns_query_results
docs/specs/…, docs/architecture-overview.md  Task 6
```

## Interfaces for later plans

- **Plan 1b-3 (pipeline) consumes events with:**
  - `session::{Session, Config::{session_a, session_b}, SESSION_A, SESSION_B}`. `Session::start` stops a leftover session of the same name first.
  - `providers::session_a(network_udp)` for `Session::enable`.
  - `session::consume(name, closure)`. The closure is Session A's or B's callback ([1] in §3.2): `VersionGate::parse(&rec)`, then the agent's own filtering and queueing.
  - `EventRecord::{pid, tid, timestamp (raw QPC), start_key, pointer_size, meta, payload}`, and `Consumer::qpc_frequency()` for §3.3's time conversion.
  - **The callback must not be able to panic repeatedly.** A panic is caught and counted (`Consumer::panics`), but a `Mutex` poisoned by it would make every later event panic too. So the callback holds no lock across code that can panic, or recovers from poisoning. Plan 1b-4 treats a rising `panics()` like a stopped session.
  - `ParseError` maps to the counters: `UnknownEvent` → not counted (events outside our set); `UnsupportedVersion` and `NewerVersion` → `unknown_version`; `Truncated`, `Unterminated` and `Malformed` → parse errors.
- **Payload types** (plus `parse_as(meta, as_version, payload, exact)`, used by the gate): `parse::{ProcessStart, ProcessStop, ImageLoad, FileCreate (+ delete_on_close()), FileHandle, FileWrite, FileSetInfo, FileOpEnd (+ failed()), FilePath, RegOpen, RegKey, RegSetValue (+ value_name_ambiguous), RegDeleteValue, NetEvent, DnsQuery, ClassicProcess, ClassicKind, Sid (+ to_string, rid, is_mandatory_label), WStr (+ to_string_lossy, as_units)}`, and `parse_query_results`.
  - `NetEvent` uses the manifest's names. Map them with F5: for UDP receive, local is `daddr`.
- **Plan 1b-4 (health, watchdog, service):**
  - `session::query_by_name` (`SessionInfo::{logger_id, events_lost, realtime_buffers_lost}`).
  - `session::provider_state(provider)` (`ProviderEnable`; compare our bits as a subset, F3).
  - `session::stop_by_name`; `Consumer::{is_finished, panics, close}`; `Session::{flush, stop, disable, enable}`; `Session::enable_guid` (for example Kernel-EventTracing, F2).
  - `VersionGate::verdicts()` (`Verdict::{Rejected, Prefix, Same}`) for logs.
  - `Session::{is_current, abandon}` and `session::STALE` (clarification 10). The restart logic starts a new `Session` and abandons the old one.

---

### Task 1: Crate, providers and layout table

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/atlas-etw/Cargo.toml`, `src/lib.rs`, `src/providers.rs`, `src/layout.rs`

**Interfaces:**
- Produces: `Provider::{ALL, guid, from_guid, name, is_user_mode}`, `providers::{Enable, LEVEL, session_a}`, `layout::{InType, Field, Layout, LAYOUTS, find, newest, is_prefix}`.

- [ ] **Step 1: Workspace dependency and crate manifest**

`Cargo.toml`:
```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -22,3 +22,4 @@ serde_json = "1"
 tempfile = "3.27"
 thiserror = "2"
 uuid = { version = "1.26", features = ["v7"] }
+windows = "0.62"
```

`crates/atlas-etw/Cargo.toml`. It has the dev-dependencies the later tasks' tests need:
```toml
[package]
name = "atlas-etw"
version = "0.1.0"
description = "ETW sessions (Windows) and hand-written, portable event parsers for the Atlas sensor."
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[target.'cfg(windows)'.dependencies.windows]
workspace = true
features = ["Win32_Foundation", "Win32_System_Diagnostics_Etw", "Win32_System_Time"]

[dev-dependencies]
proptest.workspace = true
serde_json.workspace = true

[target.'cfg(windows)'.dev-dependencies.windows]
workspace = true
features = [
    "Wdk_Foundation",
    "Wdk_System_Registry",
    "Win32_NetworkManagement_Dns",
    "Win32_Security",
    "Win32_Storage_FileSystem",
    "Win32_System_Performance",
    "Win32_System_Registry",
    "Win32_System_Threading",
]
```

`crates/atlas-etw/src/lib.rs` (Tasks 2 and 3 add `parse` and `session`):
```rust
//! ETW for the Atlas sensor (sensor spec §3.1, §4).
//!
//! - [`parse`]: pure parsers, payload bytes → [`parse::RawEvent`]. Portable and
//!   free of `unsafe`; unit-tested and fuzzed on Linux.
//! - [`layout`]: the manifest layout of every event version we parse.
//! - [`providers`]: the providers, and how Session A enables them (§4.2).
//! - `session` (Windows only): start, enable, consume, query and stop the
//!   real-time sessions. All of the sensor's ETW `unsafe` lives there, behind
//!   a safe API.

pub mod layout;
pub mod providers;

pub use providers::Provider;
```

- [ ] **Step 2: Providers**

`crates/atlas-etw/src/providers.rs`:
```rust
//! The providers the sensor consumes and how Session A enables them (sensor spec §4.2).
#![forbid(unsafe_code)]

/// A provider whose events we parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Provider {
    KernelProcess,
    KernelFile,
    KernelRegistry,
    KernelNetwork,
    DnsClient,
    /// The classic kernel `Process` event class (Session B, system logger).
    ClassicProcess,
}

impl Provider {
    pub const ALL: [Provider; 6] = [
        Provider::KernelProcess,
        Provider::KernelFile,
        Provider::KernelRegistry,
        Provider::KernelNetwork,
        Provider::DnsClient,
        Provider::ClassicProcess,
    ];

    /// The provider GUID (for the classic process events, the event class GUID).
    pub const fn guid(self) -> u128 {
        match self {
            Provider::KernelProcess => 0x22fb2cd6_0e7b_422b_a0c7_2fad1fd0e716,
            Provider::KernelFile => 0xedd08927_9cc4_4e65_b970_c2560fb5c289,
            Provider::KernelRegistry => 0x70eb4f03_c1de_4f73_a051_33d13d5413bd,
            Provider::KernelNetwork => 0x7dd42a49_5329_4832_8dfd_43d979153a88,
            Provider::DnsClient => 0x1c95126e_7eea_49a9_a3fe_a378b03ddb4d,
            Provider::ClassicProcess => 0x3d6fa8d0_fe05_11d0_9dda_00c04fd7ba7c,
        }
    }

    pub fn from_guid(guid: u128) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.guid() == guid)
    }

    /// The registered provider name (for logs and Event Log Activity's `log_provider`).
    pub const fn name(self) -> &'static str {
        match self {
            Provider::KernelProcess => "Microsoft-Windows-Kernel-Process",
            Provider::KernelFile => "Microsoft-Windows-Kernel-File",
            Provider::KernelRegistry => "Microsoft-Windows-Kernel-Registry",
            Provider::KernelNetwork => "Microsoft-Windows-Kernel-Network",
            Provider::DnsClient => "Microsoft-Windows-DNS-Client",
            Provider::ClassicProcess => "Windows Kernel Trace (Process)",
        }
    }

    /// Logged from user mode, so any process can forge it (§4.4).
    pub const fn is_user_mode(self) -> bool {
        matches!(self, Provider::DnsClient)
    }
}

/// How one provider is enabled in Session A.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enable {
    pub provider: Provider,
    /// `MatchAnyKeyword`.
    pub keywords: u64,
    /// The `EVENT_FILTER_TYPE_EVENT_ID` allow-list.
    pub event_ids: Vec<u16>,
}

/// `TRACE_LEVEL_VERBOSE`: every level; the keywords and IDs do the selecting.
pub const LEVEL: u8 = 5;

/// Session A's providers, keywords and event IDs (sensor spec §4.2).
/// `udp`: the `network.udp` setting (§7.3), on by default.
pub fn session_a(udp: bool) -> Vec<Enable> {
    let mut net = vec![12, 13, 15, 28, 29, 31];
    if udp {
        net.extend([42, 43, 58, 59]);
    }
    vec![
        // WINEVENT_KEYWORD_PROCESS 0x10, WINEVENT_KEYWORD_IMAGE 0x40.
        Enable { provider: Provider::KernelProcess, keywords: 0x50, event_ids: vec![1, 2, 5] },
        // FILEIO 0x20, OP_END 0x40, CREATE 0x80, WRITE 0x200, DELETE_PATH 0x400,
        // RENAME_SETLINK_PATH 0x800, CREATE_NEW_FILE 0x1000.
        Enable {
            provider: Provider::KernelFile,
            keywords: 0x1EE0,
            event_ids: vec![12, 13, 14, 16, 17, 24, 26, 27, 30],
        },
        // CloseKey 0x1, SetValueKey 0x100, DeleteValueKey 0x200, CreateKey 0x1000,
        // OpenKey 0x2000, DeleteKey 0x4000.
        Enable { provider: Provider::KernelRegistry, keywords: 0x7301, event_ids: vec![1, 2, 3, 5, 6, 13] },
        // IPV4 0x10, IPV6 0x20.
        Enable { provider: Provider::KernelNetwork, keywords: 0x30, event_ids: net },
        // The Operational channel keyword, the only one that delivers 3008 (S3).
        Enable { provider: Provider::DnsClient, keywords: 0x8000_0000_0000_0000, event_ids: vec![3008] },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout;

    #[test]
    fn guids_round_trip() {
        for p in Provider::ALL {
            assert_eq!(Provider::from_guid(p.guid()), Some(p));
        }
        assert_eq!(Provider::from_guid(0), None);
    }

    #[test]
    fn every_enabled_event_has_a_parser() {
        for e in session_a(true) {
            for id in e.event_ids {
                assert!(layout::newest(e.provider, id).is_some(), "{:?} {id}", e.provider);
            }
        }
    }

    #[test]
    fn udp_off_drops_only_the_datagram_events() {
        let on = session_a(true);
        let off = session_a(false);
        let net = |v: &[Enable]| v.iter().find(|e| e.provider == Provider::KernelNetwork).unwrap().event_ids.clone();
        assert_eq!(net(&on), [12, 13, 15, 28, 29, 31, 42, 43, 58, 59]);
        assert_eq!(net(&off), [12, 13, 15, 28, 29, 31]);
    }
}
```

- [ ] **Step 3: Layout table**

The field lists are the installed manifests' (Task 3's manifest test checks them), except the classic class, which comes from its MOF description (F6, F8). `Status`, `CreateOptions` and the like are `UInt32` in-types; their *display* type is hex.

`crates/atlas-etw/src/layout.rs`:
```rust
//! The payload layout of every (provider, event, version) we parse, as the
//! provider's manifest declares it: field names and TDH in-types, in order.
//!
//! The hand-written parsers in [`crate::parse`] follow these tables. Three
//! checks tie the tables to Windows:
//! - a Windows test compares each manifest entry with the installed manifest
//!   (`TdhGetManifestEventInformation`, no elevation needed);
//! - at run time, an event with a higher version than we know is accepted only
//!   if our newest layout is a prefix of its TDH layout (sensor spec §4.3);
//! - the replay fixtures compare parsed values with TDH's decoding.
//!
//! Session B's classic process events have no manifest; their layout comes
//! from the kernel's MOF class and spike S8 (§15.3), and the version check reads
//! it through TDH the same way.
#![forbid(unsafe_code)]

use crate::Provider;

/// A TDH in-type (`TDH_INTYPE_*` in tdh.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum InType {
    UnicodeString = 1,
    AnsiString = 2,
    UInt16 = 6,
    Int32 = 7,
    UInt32 = 8,
    UInt64 = 10,
    Binary = 14,
    Pointer = 16,
    FileTime = 17,
    Sid = 19,
    HexInt32 = 20,
    HexInt64 = 21,
    /// A `TOKEN_USER` followed by a SID (classic MOF events).
    WbemSid = 310,
}

impl InType {
    pub fn from_raw(v: u16) -> Option<Self> {
        use InType::*;
        [
            UnicodeString,
            AnsiString,
            UInt16,
            Int32,
            UInt32,
            UInt64,
            Binary,
            Pointer,
            FileTime,
            Sid,
            HexInt32,
            HexInt64,
            WbemSid,
        ]
        .into_iter()
        .find(|t| *t as u16 == v)
    }
}

/// One field: its manifest name and in-type.
pub type Field = (&'static str, InType);

/// The layout of one event version.
#[derive(Debug)]
pub struct Layout {
    pub provider: Provider,
    /// The event ID; for classic events (Session B), the opcode.
    pub id: u16,
    pub version: u8,
    pub fields: &'static [Field],
}

use InType::*;

const PROCESS_START_V3: &[Field] = &[
    ("ProcessID", UInt32),
    ("ProcessSequenceNumber", UInt64),
    ("CreateTime", FileTime),
    ("ParentProcessID", UInt32),
    ("ParentProcessSequenceNumber", UInt64),
    ("SessionID", UInt32),
    ("Flags", UInt32),
    ("ProcessTokenElevationType", UInt32),
    ("ProcessTokenIsElevated", UInt32),
    ("MandatoryLabel", Sid),
    ("ImageName", UnicodeString),
    ("ImageChecksum", UInt32),
    ("TimeDateStamp", UInt32),
    ("PackageFullName", UnicodeString),
    ("PackageRelativeAppId", UnicodeString),
];
const PROCESS_START_V4: &[Field] = &[
    ("ProcessID", UInt32),
    ("ProcessSequenceNumber", UInt64),
    ("CreateTime", FileTime),
    ("ParentProcessID", UInt32),
    ("ParentProcessSequenceNumber", UInt64),
    ("SessionID", UInt32),
    ("Flags", UInt32),
    ("ProcessTokenElevationType", UInt32),
    ("ProcessTokenIsElevated", UInt32),
    ("MandatoryLabel", Sid),
    ("ImageName", UnicodeString),
    ("ImageChecksum", UInt32),
    ("TimeDateStamp", UInt32),
    ("PackageFullName", UnicodeString),
    ("PackageRelativeAppId", UnicodeString),
    ("SecurityMitigations", UInt32),
];
const PROCESS_STOP_V2: &[Field] = &[
    ("ProcessID", UInt32),
    ("ProcessSequenceNumber", UInt64),
    ("CreateTime", FileTime),
    ("ExitTime", FileTime),
    ("ExitCode", UInt32),
    ("TokenElevationType", UInt32),
    ("HandleCount", UInt32),
    ("CommitCharge", UInt64),
    ("CommitPeak", UInt64),
    ("CPUCycleCount", UInt64),
    ("ReadOperationCount", UInt32),
    ("WriteOperationCount", UInt32),
    ("ReadTransferKiloBytes", UInt32),
    ("WriteTransferKiloBytes", UInt32),
    ("HardFaultCount", UInt32),
    ("ImageName", AnsiString),
];
const IMAGE_LOAD_V0: &[Field] = &[
    ("ImageBase", Pointer),
    ("ImageSize", Pointer),
    ("ProcessID", UInt32),
    ("ImageCheckSum", UInt32),
    ("TimeDateStamp", UInt32),
    ("DefaultBase", Pointer),
    ("ImageName", UnicodeString),
];

const FILE_CREATE_V1: &[Field] = &[
    ("Irp", Pointer),
    ("FileObject", Pointer),
    ("IssuingThreadId", UInt32),
    ("CreateOptions", UInt32),
    ("CreateAttributes", UInt32),
    ("ShareAccess", UInt32),
    ("FileName", UnicodeString),
];
const FILE_HANDLE_V1: &[Field] =
    &[("Irp", Pointer), ("FileObject", Pointer), ("FileKey", Pointer), ("IssuingThreadId", UInt32)];
const FILE_WRITE_V1: &[Field] = &[
    ("ByteOffset", UInt64),
    ("Irp", Pointer),
    ("FileObject", Pointer),
    ("FileKey", Pointer),
    ("IssuingThreadId", UInt32),
    ("IOSize", UInt32),
    ("IOFlags", UInt32),
    ("ExtraFlags", UInt32),
];
const FILE_SET_INFO_V1: &[Field] = &[
    ("Irp", Pointer),
    ("FileObject", Pointer),
    ("FileKey", Pointer),
    ("ExtraInformation", Pointer),
    ("IssuingThreadId", UInt32),
    ("InfoClass", UInt32),
];
const FILE_OP_END_V0: &[Field] = &[("Irp", Pointer), ("ExtraInformation", Pointer), ("Status", UInt32)];
const FILE_PATH_V1: &[Field] = &[
    ("Irp", Pointer),
    ("FileObject", Pointer),
    ("FileKey", Pointer),
    ("ExtraInformation", Pointer),
    ("IssuingThreadId", UInt32),
    ("InfoClass", UInt32),
    ("FilePath", UnicodeString),
];

const REG_OPEN_V0: &[Field] = &[
    ("BaseObject", Pointer),
    ("KeyObject", Pointer),
    ("Status", UInt32),
    ("Disposition", UInt32),
    ("BaseName", UnicodeString),
    ("RelativeName", UnicodeString),
];
const REG_KEY_V0: &[Field] = &[("KeyObject", Pointer), ("Status", UInt32), ("KeyName", UnicodeString)];
const REG_SET_VALUE_V0: &[Field] = &[
    ("KeyObject", Pointer),
    ("Status", UInt32),
    ("Type", UInt32),
    ("DataSize", UInt32),
    ("KeyName", UnicodeString),
    ("ValueName", UnicodeString),
    ("CapturedDataSize", UInt16),
    ("CapturedData", Binary),
    ("PreviousDataType", UInt32),
    ("PreviousDataSize", UInt32),
    ("PreviousDataCapturedSize", UInt16),
    ("PreviousData", Binary),
];
const REG_DELETE_VALUE_V0: &[Field] =
    &[("KeyObject", Pointer), ("Status", UInt32), ("KeyName", UnicodeString), ("ValueName", UnicodeString)];

const TCP4_OPEN: &[Field] = &[
    ("PID", UInt32),
    ("size", UInt32),
    ("daddr", UInt32),
    ("saddr", UInt32),
    ("dport", UInt16),
    ("sport", UInt16),
    ("mss", UInt16),
    ("sackopt", UInt16),
    ("tsopt", UInt16),
    ("wsopt", UInt16),
    ("rcvwin", UInt32),
    ("rcvwinscale", UInt16),
    ("sndwinscale", UInt16),
    ("seqnum", UInt32),
    ("connid", UInt32),
];
const TCP6_OPEN: &[Field] = &[
    ("PID", UInt32),
    ("size", UInt32),
    ("daddr", Binary),
    ("saddr", Binary),
    ("dport", UInt16),
    ("sport", UInt16),
    ("mss", UInt16),
    ("sackopt", UInt16),
    ("tsopt", UInt16),
    ("wsopt", UInt16),
    ("rcvwin", UInt32),
    ("rcvwinscale", UInt16),
    ("sndwinscale", UInt16),
    ("seqnum", UInt32),
    ("connid", UInt32),
];
/// TCP disconnect and all UDP datagram events (IPv4).
const NET4_SIMPLE: &[Field] = &[
    ("PID", UInt32),
    ("size", UInt32),
    ("daddr", UInt32),
    ("saddr", UInt32),
    ("dport", UInt16),
    ("sport", UInt16),
    ("seqnum", UInt32),
    ("connid", UInt32),
];
const NET6_SIMPLE: &[Field] = &[
    ("PID", UInt32),
    ("size", UInt32),
    ("daddr", Binary),
    ("saddr", Binary),
    ("dport", UInt16),
    ("sport", UInt16),
    ("seqnum", UInt32),
    ("connid", UInt32),
];

const DNS_QUERY_V0: &[Field] = &[
    ("QueryName", UnicodeString),
    ("QueryType", UInt32),
    ("QueryOptions", UInt64),
    ("QueryStatus", UInt32),
    ("QueryResults", UnicodeString),
];

/// Classic `Process` event class, version 4 (Start, End, DCStart, DCEnd).
const CLASSIC_PROCESS_V4: &[Field] = &[
    ("UniqueProcessKey", Pointer),
    ("ProcessId", UInt32),
    ("ParentId", UInt32),
    ("SessionId", UInt32),
    ("ExitStatus", Int32),
    ("DirectoryTableBase", Pointer),
    ("Flags", UInt32),
    ("UserSID", WbemSid),
    ("ImageFileName", AnsiString),
    ("CommandLine", UnicodeString),
    ("PackageFullName", UnicodeString),
    ("ApplicationId", UnicodeString),
];

const fn l(provider: Provider, id: u16, version: u8, fields: &'static [Field]) -> Layout {
    Layout { provider, id, version, fields }
}

/// Every layout we parse, ordered by (provider, id, version).
pub const LAYOUTS: &[Layout] = &[
    l(Provider::KernelProcess, 1, 3, PROCESS_START_V3),
    l(Provider::KernelProcess, 1, 4, PROCESS_START_V4),
    l(Provider::KernelProcess, 2, 2, PROCESS_STOP_V2),
    l(Provider::KernelProcess, 5, 0, IMAGE_LOAD_V0),
    l(Provider::KernelFile, 12, 1, FILE_CREATE_V1),
    l(Provider::KernelFile, 13, 1, FILE_HANDLE_V1),
    l(Provider::KernelFile, 14, 1, FILE_HANDLE_V1),
    l(Provider::KernelFile, 16, 1, FILE_WRITE_V1),
    l(Provider::KernelFile, 17, 1, FILE_SET_INFO_V1),
    l(Provider::KernelFile, 24, 0, FILE_OP_END_V0),
    l(Provider::KernelFile, 26, 1, FILE_PATH_V1),
    l(Provider::KernelFile, 27, 1, FILE_PATH_V1),
    l(Provider::KernelFile, 30, 1, FILE_CREATE_V1),
    l(Provider::KernelRegistry, 1, 0, REG_OPEN_V0),
    l(Provider::KernelRegistry, 2, 0, REG_OPEN_V0),
    l(Provider::KernelRegistry, 3, 0, REG_KEY_V0),
    l(Provider::KernelRegistry, 5, 0, REG_SET_VALUE_V0),
    l(Provider::KernelRegistry, 6, 0, REG_DELETE_VALUE_V0),
    l(Provider::KernelRegistry, 13, 0, REG_KEY_V0),
    l(Provider::KernelNetwork, 12, 0, TCP4_OPEN),
    l(Provider::KernelNetwork, 13, 0, NET4_SIMPLE),
    l(Provider::KernelNetwork, 15, 0, TCP4_OPEN),
    l(Provider::KernelNetwork, 28, 0, TCP6_OPEN),
    l(Provider::KernelNetwork, 29, 0, NET6_SIMPLE),
    l(Provider::KernelNetwork, 31, 0, TCP6_OPEN),
    l(Provider::KernelNetwork, 42, 0, NET4_SIMPLE),
    l(Provider::KernelNetwork, 43, 0, NET4_SIMPLE),
    l(Provider::KernelNetwork, 58, 0, NET6_SIMPLE),
    l(Provider::KernelNetwork, 59, 0, NET6_SIMPLE),
    l(Provider::DnsClient, 3008, 0, DNS_QUERY_V0),
    l(Provider::ClassicProcess, 1, 4, CLASSIC_PROCESS_V4),
    l(Provider::ClassicProcess, 2, 4, CLASSIC_PROCESS_V4),
    l(Provider::ClassicProcess, 3, 4, CLASSIC_PROCESS_V4),
    l(Provider::ClassicProcess, 4, 4, CLASSIC_PROCESS_V4),
];

/// The layout of exactly this version, if we parse it.
pub fn find(provider: Provider, id: u16, version: u8) -> Option<&'static Layout> {
    LAYOUTS.iter().find(|l| l.provider == provider && l.id == id && l.version == version)
}

/// The newest version of this event that we parse.
pub fn newest(provider: Provider, id: u16) -> Option<&'static Layout> {
    LAYOUTS.iter().filter(|l| l.provider == provider && l.id == id).max_by_key(|l| l.version)
}

/// Sensor spec §4.3: a newer version is parsed with our newest layout only if
/// that layout is a prefix of the newer one (same names and in-types, in order).
pub fn is_prefix(known: &[Field], newer: &[(String, u16)]) -> bool {
    known.len() <= newer.len() && known.iter().zip(newer).all(|((n, t), (m, u))| *n == m && *t as u16 == *u)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_are_sorted_and_unique() {
        let keys: Vec<_> = LAYOUTS.iter().map(|l| (l.provider as u8, l.id, l.version)).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(keys, sorted);
    }

    #[test]
    fn newest_picks_the_highest_version() {
        assert_eq!(newest(Provider::KernelProcess, 1).map(|l| l.version), Some(4));
        assert!(newest(Provider::KernelProcess, 99).is_none());
    }

    #[test]
    fn v4_process_start_extends_v3() {
        let v4: Vec<_> = PROCESS_START_V4.iter().map(|(n, t)| (n.to_string(), *t as u16)).collect();
        assert!(is_prefix(PROCESS_START_V3, &v4));
        assert!(!is_prefix(PROCESS_START_V4, &v4[..v4.len() - 1]));
    }

    #[test]
    fn a_renamed_or_retyped_field_is_not_a_prefix() {
        let mut newer: Vec<_> = REG_KEY_V0.iter().map(|(n, t)| (n.to_string(), *t as u16)).collect();
        newer.push(("Extra".into(), UInt32 as u16));
        assert!(is_prefix(REG_KEY_V0, &newer));
        newer[1].0 = "NtStatus".into();
        assert!(!is_prefix(REG_KEY_V0, &newer));
        newer[1] = ("Status".into(), HexInt32 as u16);
        assert!(!is_prefix(REG_KEY_V0, &newer));
    }
}
```

- [ ] **Step 4: Check and commit**

```powershell
cargo test -p atlas-etw --lib
cargo clippy -p atlas-etw --all-targets -- -D warnings
```
Expected: 7 tests pass and clippy is clean. (The module doc's link to `parse` resolves once Task 2 adds it.)
```powershell
git add Cargo.toml Cargo.lock crates/atlas-etw
git commit -m "feat(etw): atlas-etw crate with providers and layout table"
```

### Task 2: Parsers

**Files:**
- Modify: `crates/atlas-etw/src/lib.rs`
- Create: `crates/atlas-etw/src/parse/mod.rs`, `reader.rs`, `dns.rs`, `tests.rs`

**Interfaces:**
- Produces: `parse::{EventMeta, PointerSize, ParseError, WStr, Sid, RawEvent, parse, parse_as, parse_query_results, DnsAnswer, DnsAnswers}` and the payload structs listed under "Interfaces for later plans".

- [ ] **Step 1: Wire the module**

`crates/atlas-etw/src/lib.rs`:
```diff
 pub mod layout;
+pub mod parse;
 pub mod providers;
```

- [ ] **Step 2: The reader**

`crates/atlas-etw/src/parse/reader.rs`. Every read checks the remaining length first, and a failed read consumes nothing. `counted_wstr` reads a registry name that may embed NULs (clarification 5):
```rust
//! A bounds-checked cursor over an event payload. Every read checks the
//! remaining length first, so a short or malformed payload is an error, never a
//! panic (sensor spec §4.3).

use super::{ParseError, PointerSize, WStr};

pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    /// Name of the field being read, for error messages.
    field: &'static str,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0, field: "" }
    }

    /// Names the next field; errors from the reads that follow carry it.
    pub(crate) fn field(&mut self, name: &'static str) -> &mut Self {
        self.field = name;
        self
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    /// The bytes read since position `start`.
    pub(crate) fn consumed_since(&self, start: usize) -> &'a [u8] {
        &self.data[start..self.pos]
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        match end {
            Some(end) => {
                let s = &self.data[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            None => Err(ParseError::Truncated { field: self.field, offset: self.pos }),
        }
    }

    pub(crate) fn skip(&mut self, n: usize) -> Result<(), ParseError> {
        self.take(n).map(|_| ())
    }

    pub(crate) fn bytes<const N: usize>(&mut self) -> Result<[u8; N], ParseError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub(crate) fn u16(&mut self) -> Result<u16, ParseError> {
        self.bytes().map(u16::from_le_bytes)
    }

    /// A 16-bit value in network byte order (manifest out-type `win:Port`).
    pub(crate) fn u16_be(&mut self) -> Result<u16, ParseError> {
        self.bytes().map(u16::from_be_bytes)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, ParseError> {
        self.bytes().map(u32::from_le_bytes)
    }

    pub(crate) fn u64(&mut self) -> Result<u64, ParseError> {
        self.bytes().map(u64::from_le_bytes)
    }

    /// A pointer-sized value, widened to 64 bits.
    pub(crate) fn ptr(&mut self, size: PointerSize) -> Result<u64, ParseError> {
        match size {
            PointerSize::P32 => self.u32().map(u64::from),
            PointerSize::P64 => self.u64(),
        }
    }

    /// A NUL-terminated UTF-16 string (`win:UnicodeString`). The terminator is
    /// consumed and not kept. A missing terminator is an error: the field would
    /// otherwise silently swallow the fields after it.
    pub(crate) fn wstr(&mut self) -> Result<WStr, ParseError> {
        let rest = self.rest();
        let units = rest.len() / 2;
        let end = (0..units).find(|&i| rest[2 * i] == 0 && rest[2 * i + 1] == 0);
        match end {
            Some(n) => {
                let s = WStr::from_le_bytes(&rest[..2 * n]);
                self.pos += 2 * n + 2;
                Ok(s)
            }
            None => Err(ParseError::Unterminated { field: self.field, offset: self.pos }),
        }
    }

    /// A UTF-16 name that may contain embedded NULs, followed by a terminator and
    /// then fields of variable size. The terminator is the **last** NUL unit after
    /// which `trailer_len` accounts for exactly the rest of the payload. Registry
    /// names are counted strings and the kernel logs them whole (`a`, NUL, `b`,
    /// then the terminator), so stopping at the first NUL would misread the
    /// fields after it. Falls back to [`Reader::wstr`] when no NUL fits.
    ///
    /// Also returns whether **another** NUL fits too. Then the split is ambiguous:
    /// the trailer's own data could be read as part of the name. That cannot
    /// happen while the trailer's variable parts are empty (as S4 found for
    /// SetValueKey), but the caller is told rather than guessing silently.
    pub(crate) fn counted_wstr(
        &mut self,
        trailer_len: impl Fn(&[u8]) -> Option<usize>,
    ) -> Result<(WStr, bool), ParseError> {
        let rest = self.rest();
        let mut fits = (0..rest.len() / 2)
            .rev()
            .filter(|&i| rest[2 * i] == 0 && rest[2 * i + 1] == 0)
            .filter(|&i| trailer_len(&rest[2 * i + 2..]) == Some(rest.len() - 2 * i - 2));
        match fits.next() {
            Some(n) => {
                let ambiguous = fits.next().is_some();
                let s = WStr::from_le_bytes(&rest[..2 * n]);
                self.pos += 2 * n + 2;
                Ok((s, ambiguous))
            }
            None => self.wstr().map(|s| (s, false)),
        }
    }

    /// A NUL-terminated 8-bit string (`win:AnsiString`), kept as bytes.
    pub(crate) fn astr(&mut self) -> Result<Box<[u8]>, ParseError> {
        let rest = self.rest();
        match rest.iter().position(|&b| b == 0) {
            Some(n) => {
                let s: Box<[u8]> = rest[..n].into();
                self.pos += n + 1;
                Ok(s)
            }
            None => Err(ParseError::Unterminated { field: self.field, offset: self.pos }),
        }
    }

    /// A SID (`win:SID`): revision, sub-authority count, 6-byte authority, then
    /// 4 bytes per sub-authority. Returned as its raw bytes.
    pub(crate) fn sid(&mut self) -> Result<Sid, ParseError> {
        let start = self.pos;
        let head = self.bytes::<8>()?;
        let count = usize::from(head[1]);
        if head[0] != 1 || count > Sid::MAX_SUB_AUTHORITIES {
            return Err(ParseError::Malformed { field: self.field, offset: start });
        }
        self.skip(4 * count)?;
        Ok(Sid(self.data[start..self.pos].into()))
    }
}

/// A security identifier in its binary form (`SID` structure).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Sid(Box<[u8]>);

impl Sid {
    /// `SID_MAX_SUB_AUTHORITIES` in winnt.h.
    pub const MAX_SUB_AUTHORITIES: usize = 15;

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn authority(&self) -> u64 {
        self.0[2..8].iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b))
    }

    /// The sub-authorities, in order.
    pub fn sub_authorities(&self) -> impl Iterator<Item = u32> + '_ {
        self.0[8..].chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
    }

    /// The last sub-authority (the RID), if any. For a mandatory label
    /// (`S-1-16-X`) this is the integrity level.
    pub fn rid(&self) -> Option<u32> {
        self.sub_authorities().last()
    }

    /// The SID's authority and the first sub-authority, used to recognise
    /// mandatory labels (`S-1-16-…`).
    pub fn is_mandatory_label(&self) -> bool {
        self.authority() == 16 && self.0[1] == 1
    }
}

/// `S-1-5-21-…` form, as `ConvertSidToStringSidW` writes it.
impl std::fmt::Display for Sid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let auth = self.authority();
        if auth < (1 << 32) {
            write!(f, "S-{}-{}", self.0[0], auth)?;
        } else {
            write!(f, "S-{}-0x{:012X}", self.0[0], auth)?;
        }
        for s in self.sub_authorities() {
            write!(f, "-{s}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for Sid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sid({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_past_the_end_are_errors_with_the_field_name() {
        let mut r = Reader::new(&[1, 2, 3]);
        assert_eq!(r.field("Status").u16(), Ok(0x0201));
        assert_eq!(r.field("Disposition").u32(), Err(ParseError::Truncated { field: "Disposition", offset: 2 }));
        // A failed read consumes nothing.
        assert_eq!(r.bytes::<1>(), Ok([3]));
    }

    #[test]
    fn a_huge_skip_does_not_overflow() {
        let mut r = Reader::new(&[0; 4]);
        r.skip(2).unwrap();
        assert!(r.skip(usize::MAX).is_err());
    }

    #[test]
    fn wide_strings_need_their_terminator() {
        // "ab\0" then one more unit.
        let mut r = Reader::new(&[b'a', 0, b'b', 0, 0, 0, b'c', 0]);
        assert_eq!(r.wstr().unwrap().to_string_lossy(), "ab");
        assert_eq!(r.pos(), 6);
        assert_eq!(r.field("Name").wstr(), Err(ParseError::Unterminated { field: "Name", offset: 6 }));
    }

    #[test]
    fn a_terminator_must_be_unit_aligned() {
        // Bytes 1..3 are zero but straddle two units: not a terminator.
        let mut r = Reader::new(&[b'a', 0, 0, b'b', 0, 0]);
        assert_eq!(r.wstr().unwrap().as_units(), &[u16::from(b'a'), u16::from(b'b') << 8]);
    }

    #[test]
    fn counted_names_keep_embedded_nuls() {
        // "a", NUL, "b", the terminator, then a 2-byte trailer that must end the payload.
        let b = [b'a', 0, 0, 0, b'b', 0, 0, 0, 0xEE, 0xEE];
        let two = |t: &[u8]| (t.len() >= 2).then_some(2);
        let mut r = Reader::new(&b);
        assert_eq!(r.counted_wstr(two).unwrap(), (WStr::from_units(&[u16::from(b'a'), 0, u16::from(b'b')]), false));
        assert_eq!(r.rest(), &[0xEE, 0xEE]); // the trailer is left for the fields after the name
        // Without embedded NULs it is the ordinary string.
        let mut r = Reader::new(&[b'a', 0, 0, 0, 0xEE, 0xEE]);
        assert_eq!(r.counted_wstr(two).unwrap(), (WStr::from("a"), false));
        // No NUL fits the trailer: the first NUL ends the string, as for wstr().
        let mut r = Reader::new(&[b'a', 0, 0, 0, b'b', 0, 0, 0]);
        assert_eq!(r.counted_wstr(|_| None).unwrap(), (WStr::from("a"), false));
        // Two NULs fit a trailer of any length up to the end: ambiguous, last one wins.
        let mut r = Reader::new(&[b'a', 0, 0, 0, b'b', 0, 0, 0]);
        let any = |t: &[u8]| Some(t.len());
        assert_eq!(r.counted_wstr(any).unwrap(), (WStr::from_units(&[u16::from(b'a'), 0, u16::from(b'b')]), true));
    }

    #[test]
    fn ansi_strings_stop_at_nul() {
        let mut r = Reader::new(b"cmd.exe\0rest");
        assert_eq!(&*r.astr().unwrap(), b"cmd.exe");
        assert_eq!(r.rest(), b"rest");
    }

    #[test]
    fn sids_format_like_windows() {
        // S-1-5-21-1-2-3-1001
        let mut b = vec![1, 5, 0, 0, 0, 0, 0, 5];
        for s in [21u32, 1, 2, 3, 1001] {
            b.extend(s.to_le_bytes());
        }
        let sid = Reader::new(&b).sid().unwrap();
        assert_eq!(sid.to_string(), "S-1-5-21-1-2-3-1001");
        assert_eq!(sid.rid(), Some(1001));
        assert!(!sid.is_mandatory_label());
        // S-1-16-12288 (High integrity)
        let label = Reader::new(&[1, 1, 0, 0, 0, 0, 0, 16, 0, 0x30, 0, 0]).sid().unwrap();
        assert_eq!(label.to_string(), "S-1-16-12288");
        assert!(label.is_mandatory_label());
        assert_eq!(label.rid(), Some(12288));
    }

    #[test]
    fn bad_sids_are_rejected() {
        // Revision 2.
        assert!(Reader::new(&[2, 0, 0, 0, 0, 0, 0, 5]).sid().is_err());
        // 16 sub-authorities.
        assert!(Reader::new(&[1, 16, 0, 0, 0, 0, 0, 5]).sid().is_err());
        // Count says 2, data has 1.
        assert!(Reader::new(&[1, 2, 0, 0, 0, 0, 0, 5, 1, 0, 0, 0]).sid().is_err());
    }
}
```

- [ ] **Step 3: DNS answers**

`crates/atlas-etw/src/parse/dns.rs`:
```rust
//! DNS-Client 3008's `QueryResults` string (sensor spec §5.4, S3).
//!
//! Entries are separated by `;` (the last one is followed by one too).
//! Addresses are in text form, IPv4 or IPv6 (including IPv4-mapped IPv6 such
//! as `::ffff:192.0.2.1`). Other records appear as `type: N <data>`; CNAMEs
//! (`type: 5`) come before the addresses. A failed query has an empty string.

use std::net::IpAddr;

/// One entry of `QueryResults`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsAnswer {
    /// An address: an A record if IPv4, an AAAA record if IPv6.
    Address(IpAddr),
    /// A `type: N <data>` entry.
    Record { rtype: u16, data: String },
    /// An entry in neither form, kept as logged.
    Unrecognized(String),
}

/// The parsed entries, at most [`DnsAnswers::MAX`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DnsAnswers {
    pub answers: Vec<DnsAnswer>,
    /// More than [`DnsAnswers::MAX`] entries were present; the rest were dropped.
    pub truncated: bool,
}

impl DnsAnswers {
    /// 0a's limit on `answers[]` (`DNS_ANSWERS_MAX`).
    pub const MAX: usize = 64;
}

/// Parses `QueryResults`. Never fails: an entry it does not understand is kept
/// as [`DnsAnswer::Unrecognized`].
pub fn parse_query_results(units: &[u16]) -> DnsAnswers {
    let text = String::from_utf16_lossy(units);
    let mut out = DnsAnswers::default();
    for entry in text.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        if out.answers.len() == DnsAnswers::MAX {
            out.truncated = true;
            break;
        }
        out.answers.push(parse_entry(entry));
    }
    out
}

fn parse_entry(entry: &str) -> DnsAnswer {
    if let Some(rest) = entry.strip_prefix("type:") {
        let rest = rest.trim_start();
        let (num, data) = rest.split_once(' ').unwrap_or((rest, ""));
        if let Ok(rtype) = num.parse::<u16>() {
            return DnsAnswer::Record { rtype, data: data.to_string() };
        }
    } else if let Ok(ip) = entry.parse::<IpAddr>() {
        return DnsAnswer::Address(ip);
    }
    DnsAnswer::Unrecognized(entry.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> DnsAnswers {
        parse_query_results(&s.encode_utf16().collect::<Vec<_>>())
    }

    #[test]
    fn addresses_and_cnames_as_logged_in_s3() {
        let a = parse("type: 5 www.example.com-c-3.edgekey.net;type: 5 e1.dscb.akamaiedge.net;23.62.177.155;");
        assert_eq!(
            a.answers,
            vec![
                DnsAnswer::Record { rtype: 5, data: "www.example.com-c-3.edgekey.net".into() },
                DnsAnswer::Record { rtype: 5, data: "e1.dscb.akamaiedge.net".into() },
                DnsAnswer::Address("23.62.177.155".parse().unwrap()),
            ]
        );
        assert!(!a.truncated);
    }

    #[test]
    fn ipv6_including_mapped_ipv4() {
        let a = parse("2606:4700::6810:179a;::ffff:172.66.157.237;");
        assert_eq!(
            a.answers,
            vec![
                DnsAnswer::Address("2606:4700::6810:179a".parse().unwrap()),
                DnsAnswer::Address("::ffff:172.66.157.237".parse().unwrap()),
            ]
        );
    }

    #[test]
    fn a_failed_query_has_no_answers() {
        assert_eq!(parse(""), DnsAnswers::default());
        assert_eq!(parse(";;"), DnsAnswers::default());
    }

    #[test]
    fn odd_entries_are_kept_not_dropped() {
        assert_eq!(
            parse("type: x y;not-an-address;type: 16 \"v=spf1 -all\";").answers,
            vec![
                DnsAnswer::Unrecognized("type: x y".into()),
                DnsAnswer::Unrecognized("not-an-address".into()),
                DnsAnswer::Record { rtype: 16, data: "\"v=spf1 -all\"".into() },
            ]
        );
    }

    #[test]
    fn at_most_64_entries() {
        let s: String = (0..70).map(|i| format!("10.0.0.{i};")).collect();
        let a = parse(&s);
        assert_eq!(a.answers.len(), 64);
        assert!(a.truncated);
        let exactly: String = (0..64).map(|i| format!("10.0.0.{i};")).collect();
        assert!(!parse(&exactly).truncated);
    }

    #[test]
    fn unpaired_surrogates_do_not_panic() {
        let a = parse_query_results(&[0xD800, u16::from(b';'), u16::from(b'1')]);
        assert_eq!(a.answers, vec![DnsAnswer::Unrecognized("\u{FFFD}".into()), DnsAnswer::Unrecognized("1".into())]);
    }
}
```

- [ ] **Step 4: The parsers**

`crates/atlas-etw/src/parse/mod.rs`:
```rust
//! Pure event parsers: an event's identity and payload bytes in, a typed
//! [`RawEvent`] out (sensor spec §4.3). No `unsafe`, no Windows dependency, so
//! the parsers are unit-tested and fuzzed on Linux.
//!
//! Each parser follows the matching table in [`crate::layout`]. Pointer-sized
//! fields take their size from the event's own header ([`EventMeta::pointer_size`]).
//! Strings stay UTF-16 ([`WStr`]) until the agent emits an event.
//! A payload longer than the layout is accepted (trailing bytes are ignored);
//! one shorter than the layout is an error.
#![forbid(unsafe_code)]

mod dns;
mod reader;

use reader::Reader;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub use dns::{DnsAnswer, DnsAnswers, parse_query_results};
pub use reader::Sid;

use crate::{Provider, layout};

/// Pointer size of the process that logged the event (from its header flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerSize {
    P32,
    P64,
}

/// What the event header says about an event, all a parser needs besides the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventMeta {
    pub provider: Provider,
    /// The event ID; for classic events (Session B), the opcode.
    pub id: u16,
    pub version: u8,
    pub pointer_size: PointerSize,
}

/// Why a payload did not parse. Parsers never panic; the agent counts these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Not an event we consume.
    UnknownEvent,
    /// A version we have no layout for and that is not newer than our newest:
    /// counted as `unknown_version` and dropped.
    UnsupportedVersion { version: u8 },
    /// A version newer than our newest layout. The session layer decides, once
    /// per (provider, event, version), whether to parse it with `newest` via
    /// [`parse_as`] (sensor spec §4.3).
    NewerVersion { version: u8, newest: u8 },
    /// The payload ended inside a field.
    Truncated { field: &'static str, offset: usize },
    /// A string field without its NUL terminator.
    Unterminated { field: &'static str, offset: usize },
    /// A field whose content is impossible (for example a SID with a bad revision).
    Malformed { field: &'static str, offset: usize },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnknownEvent => write!(f, "not an event we parse"),
            ParseError::UnsupportedVersion { version } => write!(f, "unsupported version {version}"),
            ParseError::NewerVersion { version, newest } => write!(f, "version {version} is newer than {newest}"),
            ParseError::Truncated { field, offset } => write!(f, "{field}: payload ends at offset {offset}"),
            ParseError::Unterminated { field, offset } => write!(f, "{field}: no terminator after offset {offset}"),
            ParseError::Malformed { field, offset } => write!(f, "{field}: malformed at offset {offset}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// A UTF-16 string as logged, without its terminator. Converted to UTF-8 only
/// when an event is emitted (lossily: unpaired surrogates become U+FFFD, 0a §6.1).
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct WStr(Box<[u16]>);

impl WStr {
    pub(crate) fn from_le_bytes(b: &[u8]) -> Self {
        WStr(b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
    }

    pub fn from_units(units: &[u16]) -> Self {
        WStr(units.into())
    }

    pub fn as_units(&self) -> &[u16] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn to_string_lossy(&self) -> String {
        String::from_utf16_lossy(&self.0)
    }
}

impl std::fmt::Debug for WStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.to_string_lossy())
    }
}

impl From<&str> for WStr {
    fn from(s: &str) -> Self {
        WStr(s.encode_utf16().collect())
    }
}

/// Kernel-Process 1 (v3+): a new process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessStart {
    pub pid: u32,
    pub sequence_number: u64,
    /// FILETIME (100 ns since 1601, UTC).
    pub create_time: u64,
    pub parent_pid: u32,
    pub parent_sequence_number: u64,
    pub session_id: u32,
    pub flags: u32,
    pub token_elevation_type: u32,
    pub token_is_elevated: u32,
    /// `S-1-16-X`, the integrity level (sensor spec §5.2).
    pub mandatory_label: Sid,
    /// NT path of the image.
    pub image_name: WStr,
    pub image_checksum: u32,
    pub time_date_stamp: u32,
    pub package_full_name: WStr,
    pub package_relative_app_id: WStr,
    /// v4 and later.
    pub security_mitigations: Option<u32>,
}

/// Kernel-Process 2 (v2+): a process exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessStop {
    pub pid: u32,
    pub sequence_number: u64,
    pub create_time: u64,
    pub exit_time: u64,
    pub exit_code: u32,
    /// The image's file name, 8-bit (manifest `win:AnsiString`).
    pub image_name: Box<[u8]>,
}

/// Kernel-Process 5: an image mapped into a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageLoad {
    pub image_base: u64,
    pub image_size: u64,
    /// The process the image was mapped into (authoritative, §5.3).
    pub pid: u32,
    pub image_checksum: u32,
    pub time_date_stamp: u32,
    pub default_base: u64,
    pub image_name: WStr,
}

/// Kernel-File 12 `Create` and 30 `CreateNewFile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCreate {
    pub irp: u64,
    pub file_object: u64,
    pub issuing_tid: u32,
    /// `CreateOptions` from `NtCreateFile`, with the create disposition in the high byte.
    pub create_options: u32,
    pub create_attributes: u32,
    pub share_access: u32,
    /// NT path as opened (may contain 8.3 components, §7.2).
    pub file_name: WStr,
}

impl FileCreate {
    /// `FILE_DELETE_ON_CLOSE` (sensor spec §7.1).
    pub const DELETE_ON_CLOSE: u32 = 0x1000;

    pub fn delete_on_close(&self) -> bool {
        self.create_options & Self::DELETE_ON_CLOSE != 0
    }
}

/// Kernel-File 13 `Cleanup` and 14 `Close`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHandle {
    pub irp: u64,
    pub file_object: u64,
    pub file_key: u64,
    pub issuing_tid: u32,
}

/// Kernel-File 16 `Write`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    pub byte_offset: u64,
    pub irp: u64,
    pub file_object: u64,
    pub file_key: u64,
    pub issuing_tid: u32,
    pub io_size: u32,
    pub io_flags: u32,
    pub extra_flags: u32,
}

/// Kernel-File 17 `SetInformation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSetInfo {
    pub irp: u64,
    pub file_object: u64,
    pub file_key: u64,
    pub extra_information: u64,
    pub issuing_tid: u32,
    /// `FILE_INFORMATION_CLASS`: 4 basic (timestamps, attributes), 19 end of file.
    pub info_class: u32,
}

/// Kernel-File 24 `OperationEnd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOpEnd {
    pub irp: u64,
    pub extra_information: u64,
    /// The operation's NTSTATUS.
    pub status: u32,
}

impl FileOpEnd {
    /// The status has **error** severity (its top two bits are set; sensor spec
    /// §5.5). Success, informational (`STATUS_REPARSE`) and warning
    /// (`STATUS_BUFFER_OVERFLOW`) codes are not failures. This is narrower than
    /// `!NT_SUCCESS`, which also counts warnings.
    pub fn failed(&self) -> bool {
        self.status >> 30 == 3
    }
}

/// Kernel-File 26 `DeletePath` and 27 `RenamePath`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePath {
    pub irp: u64,
    pub file_object: u64,
    pub file_key: u64,
    pub extra_information: u64,
    pub issuing_tid: u32,
    pub info_class: u32,
    /// For a delete, the file's path; for a rename, the **new** path (S6).
    pub file_path: WStr,
}

/// Kernel-Registry 1 `CreateKey` and 2 `OpenKey`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegOpen {
    pub base_object: u64,
    pub key_object: u64,
    pub status: u32,
    /// CreateKey: 1 created, 2 opened.
    pub disposition: u32,
    /// Always empty in practice (S7).
    pub base_name: WStr,
    /// Relative to `base_object` unless it starts with `\REGISTRY\`.
    pub relative_name: WStr,
}

/// Kernel-Registry 3 `DeleteKey` and 13 `CloseKey`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegKey {
    pub key_object: u64,
    pub status: u32,
    /// Always empty in practice (S7).
    pub key_name: WStr,
}

/// Kernel-Registry 5 `SetValueKey`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegSetValue {
    pub key_object: u64,
    pub status: u32,
    pub value_type: u32,
    pub data_size: u32,
    pub key_name: WStr,
    /// Counted: it may contain NULs (sensor spec §7.5).
    pub value_name: WStr,
    /// More than one terminator fitted (only possible if the captured buffers
    /// are not empty, which S4 never saw); `value_name` used the last one. The
    /// agent should treat the name, and a value read by it, as unreliable.
    pub value_name_ambiguous: bool,
    /// Never filled in practice (S4).
    pub captured_data: Box<[u8]>,
    pub previous_data_type: u32,
    pub previous_data_size: u32,
    pub previous_data: Box<[u8]>,
}

/// Kernel-Registry 6 `DeleteValueKey`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegDeleteValue {
    pub key_object: u64,
    pub status: u32,
    pub key_name: WStr,
    pub value_name: WStr,
}

/// A Kernel-Network event. Field names follow the manifest; the live tests pin
/// which end each address is (sensor spec §7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetEvent {
    /// The owning process (the header PID is not the owner, §5.3).
    pub pid: u32,
    /// Not a byte total (S5).
    pub size: u32,
    pub daddr: IpAddr,
    pub saddr: IpAddr,
    pub dport: u16,
    pub sport: u16,
    pub seqnum: u32,
    pub connid: u32,
}

/// DNS-Client 3008: a query completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsQuery {
    pub query_name: WStr,
    pub query_type: u32,
    pub query_options: u64,
    /// Win32 / DNS status; 0 on success.
    pub query_status: u32,
    /// `;`-separated answers; see [`parse_query_results`].
    pub query_results: WStr,
}

/// Which classic process event (Session B), by opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassicKind {
    Start = 1,
    End = 2,
    /// Rundown of a process running when the session started.
    DcStart = 3,
    DcEnd = 4,
}

/// Session B's classic `Process` event, version 4 (S8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassicProcess {
    pub kind: ClassicKind,
    pub unique_process_key: u64,
    pub pid: u32,
    pub parent_pid: u32,
    pub session_id: u32,
    /// Signed in the MOF class (`Int32`); 259 (`STILL_ACTIVE`) for a running process.
    pub exit_status: i32,
    pub directory_table_base: u64,
    pub flags: u32,
    /// The process's user. `None` when the event carries a null SID.
    pub user_sid: Option<Sid>,
    pub image_file_name: Box<[u8]>,
    pub command_line: WStr,
    pub package_full_name: WStr,
    pub application_id: WStr,
}

/// One parsed event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawEvent {
    ProcessStart(ProcessStart),
    ProcessStop(ProcessStop),
    ImageLoad(ImageLoad),
    FileCreate(FileCreate),
    FileCreateNew(FileCreate),
    FileCleanup(FileHandle),
    FileClose(FileHandle),
    FileWrite(FileWrite),
    FileSetInfo(FileSetInfo),
    FileOpEnd(FileOpEnd),
    FileDeletePath(FilePath),
    FileRenamePath(FilePath),
    RegCreateKey(RegOpen),
    RegOpenKey(RegOpen),
    RegDeleteKey(RegKey),
    RegSetValue(RegSetValue),
    RegDeleteValue(RegDeleteValue),
    RegCloseKey(RegKey),
    TcpConnect(NetEvent),
    TcpAccept(NetEvent),
    TcpDisconnect(NetEvent),
    UdpSend(NetEvent),
    UdpRecv(NetEvent),
    DnsQuery(DnsQuery),
    ClassicProcess(ClassicProcess),
}

/// Parses one event. A version newer than our newest layout returns
/// [`ParseError::NewerVersion`]; the caller may then use [`parse_as`].
pub fn parse(meta: &EventMeta, payload: &[u8]) -> Result<RawEvent, ParseError> {
    if layout::find(meta.provider, meta.id, meta.version).is_some() {
        return parse_known(meta, payload, true);
    }
    match layout::newest(meta.provider, meta.id) {
        None => Err(ParseError::UnknownEvent),
        Some(n) if meta.version > n.version => {
            Err(ParseError::NewerVersion { version: meta.version, newest: n.version })
        }
        Some(_) => Err(ParseError::UnsupportedVersion { version: meta.version }),
    }
}

/// Parses an event with the layout of version `as_version`, which must be one we
/// know. Used for newer versions that passed the prefix check (§4.3). `exact`:
/// the newer version's layout **equals** ours (only the number changed), so a
/// name that ends the layout may run to the end of the payload. When the newer
/// layout only starts with ours, fields may follow, and such names stop at
/// their first NUL instead.
pub fn parse_as(meta: &EventMeta, as_version: u8, payload: &[u8], exact: bool) -> Result<RawEvent, ParseError> {
    if layout::find(meta.provider, meta.id, as_version).is_none() {
        return Err(ParseError::UnsupportedVersion { version: as_version });
    }
    parse_known(&EventMeta { version: as_version, ..*meta }, payload, exact)
}

/// `exact`: the payload is exactly this layout's version, so a name can be read
/// to the end of the payload. A newer version may append fields after it.
fn parse_known(meta: &EventMeta, payload: &[u8], exact: bool) -> Result<RawEvent, ParseError> {
    let r = &mut Reader::new(payload);
    let p = meta.pointer_size;
    Ok(match (meta.provider, meta.id) {
        (Provider::KernelProcess, 1) => RawEvent::ProcessStart(process_start(r, meta.version)?),
        (Provider::KernelProcess, 2) => RawEvent::ProcessStop(process_stop(r)?),
        (Provider::KernelProcess, 5) => RawEvent::ImageLoad(image_load(r, p)?),
        (Provider::KernelFile, 12) => RawEvent::FileCreate(file_create(r, p)?),
        (Provider::KernelFile, 30) => RawEvent::FileCreateNew(file_create(r, p)?),
        (Provider::KernelFile, 13) => RawEvent::FileCleanup(file_handle(r, p)?),
        (Provider::KernelFile, 14) => RawEvent::FileClose(file_handle(r, p)?),
        (Provider::KernelFile, 16) => RawEvent::FileWrite(file_write(r, p)?),
        (Provider::KernelFile, 17) => RawEvent::FileSetInfo(file_set_info(r, p)?),
        (Provider::KernelFile, 24) => RawEvent::FileOpEnd(file_op_end(r, p)?),
        (Provider::KernelFile, 26) => RawEvent::FileDeletePath(file_path(r, p)?),
        (Provider::KernelFile, 27) => RawEvent::FileRenamePath(file_path(r, p)?),
        (Provider::KernelRegistry, 1) => RawEvent::RegCreateKey(reg_open(r, p, exact)?),
        (Provider::KernelRegistry, 2) => RawEvent::RegOpenKey(reg_open(r, p, exact)?),
        (Provider::KernelRegistry, 3) => RawEvent::RegDeleteKey(reg_key(r, p)?),
        (Provider::KernelRegistry, 5) => RawEvent::RegSetValue(reg_set_value(r, p, exact)?),
        (Provider::KernelRegistry, 6) => RawEvent::RegDeleteValue(reg_delete_value(r, p, exact)?),
        (Provider::KernelRegistry, 13) => RawEvent::RegCloseKey(reg_key(r, p)?),
        (Provider::KernelNetwork, 12) => RawEvent::TcpConnect(net(r, false, true)?),
        (Provider::KernelNetwork, 28) => RawEvent::TcpConnect(net(r, true, true)?),
        (Provider::KernelNetwork, 15) => RawEvent::TcpAccept(net(r, false, true)?),
        (Provider::KernelNetwork, 31) => RawEvent::TcpAccept(net(r, true, true)?),
        (Provider::KernelNetwork, 13) => RawEvent::TcpDisconnect(net(r, false, false)?),
        (Provider::KernelNetwork, 29) => RawEvent::TcpDisconnect(net(r, true, false)?),
        (Provider::KernelNetwork, 42) => RawEvent::UdpSend(net(r, false, false)?),
        (Provider::KernelNetwork, 58) => RawEvent::UdpSend(net(r, true, false)?),
        (Provider::KernelNetwork, 43) => RawEvent::UdpRecv(net(r, false, false)?),
        (Provider::KernelNetwork, 59) => RawEvent::UdpRecv(net(r, true, false)?),
        (Provider::DnsClient, 3008) => RawEvent::DnsQuery(dns_query(r)?),
        (Provider::ClassicProcess, op @ 1..=4) => RawEvent::ClassicProcess(classic_process(r, p, op)?),
        _ => return Err(ParseError::UnknownEvent),
    })
}

fn process_start(r: &mut Reader, version: u8) -> Result<ProcessStart, ParseError> {
    Ok(ProcessStart {
        pid: r.field("ProcessID").u32()?,
        sequence_number: r.field("ProcessSequenceNumber").u64()?,
        create_time: r.field("CreateTime").u64()?,
        parent_pid: r.field("ParentProcessID").u32()?,
        parent_sequence_number: r.field("ParentProcessSequenceNumber").u64()?,
        session_id: r.field("SessionID").u32()?,
        flags: r.field("Flags").u32()?,
        token_elevation_type: r.field("ProcessTokenElevationType").u32()?,
        token_is_elevated: r.field("ProcessTokenIsElevated").u32()?,
        mandatory_label: r.field("MandatoryLabel").sid()?,
        image_name: r.field("ImageName").wstr()?,
        image_checksum: r.field("ImageChecksum").u32()?,
        time_date_stamp: r.field("TimeDateStamp").u32()?,
        package_full_name: r.field("PackageFullName").wstr()?,
        package_relative_app_id: r.field("PackageRelativeAppId").wstr()?,
        security_mitigations: if version >= 4 { Some(r.field("SecurityMitigations").u32()?) } else { None },
    })
}

fn process_stop(r: &mut Reader) -> Result<ProcessStop, ParseError> {
    let pid = r.field("ProcessID").u32()?;
    let sequence_number = r.field("ProcessSequenceNumber").u64()?;
    let create_time = r.field("CreateTime").u64()?;
    let exit_time = r.field("ExitTime").u64()?;
    let exit_code = r.field("ExitCode").u32()?;
    // TokenElevationType .. HardFaultCount: 2 × u32, 3 × u64, 5 × u32.
    r.field("TokenElevationType..HardFaultCount").skip(4 * 2 + 8 * 3 + 4 * 5)?;
    let image_name = r.field("ImageName").astr()?;
    Ok(ProcessStop { pid, sequence_number, create_time, exit_time, exit_code, image_name })
}

fn image_load(r: &mut Reader, p: PointerSize) -> Result<ImageLoad, ParseError> {
    Ok(ImageLoad {
        image_base: r.field("ImageBase").ptr(p)?,
        image_size: r.field("ImageSize").ptr(p)?,
        pid: r.field("ProcessID").u32()?,
        image_checksum: r.field("ImageCheckSum").u32()?,
        time_date_stamp: r.field("TimeDateStamp").u32()?,
        default_base: r.field("DefaultBase").ptr(p)?,
        image_name: r.field("ImageName").wstr()?,
    })
}

fn file_create(r: &mut Reader, p: PointerSize) -> Result<FileCreate, ParseError> {
    Ok(FileCreate {
        irp: r.field("Irp").ptr(p)?,
        file_object: r.field("FileObject").ptr(p)?,
        issuing_tid: r.field("IssuingThreadId").u32()?,
        create_options: r.field("CreateOptions").u32()?,
        create_attributes: r.field("CreateAttributes").u32()?,
        share_access: r.field("ShareAccess").u32()?,
        file_name: r.field("FileName").wstr()?,
    })
}

fn file_handle(r: &mut Reader, p: PointerSize) -> Result<FileHandle, ParseError> {
    Ok(FileHandle {
        irp: r.field("Irp").ptr(p)?,
        file_object: r.field("FileObject").ptr(p)?,
        file_key: r.field("FileKey").ptr(p)?,
        issuing_tid: r.field("IssuingThreadId").u32()?,
    })
}

fn file_write(r: &mut Reader, p: PointerSize) -> Result<FileWrite, ParseError> {
    Ok(FileWrite {
        byte_offset: r.field("ByteOffset").u64()?,
        irp: r.field("Irp").ptr(p)?,
        file_object: r.field("FileObject").ptr(p)?,
        file_key: r.field("FileKey").ptr(p)?,
        issuing_tid: r.field("IssuingThreadId").u32()?,
        io_size: r.field("IOSize").u32()?,
        io_flags: r.field("IOFlags").u32()?,
        extra_flags: r.field("ExtraFlags").u32()?,
    })
}

fn file_set_info(r: &mut Reader, p: PointerSize) -> Result<FileSetInfo, ParseError> {
    Ok(FileSetInfo {
        irp: r.field("Irp").ptr(p)?,
        file_object: r.field("FileObject").ptr(p)?,
        file_key: r.field("FileKey").ptr(p)?,
        extra_information: r.field("ExtraInformation").ptr(p)?,
        issuing_tid: r.field("IssuingThreadId").u32()?,
        info_class: r.field("InfoClass").u32()?,
    })
}

fn file_op_end(r: &mut Reader, p: PointerSize) -> Result<FileOpEnd, ParseError> {
    Ok(FileOpEnd {
        irp: r.field("Irp").ptr(p)?,
        extra_information: r.field("ExtraInformation").ptr(p)?,
        status: r.field("Status").u32()?,
    })
}

fn file_path(r: &mut Reader, p: PointerSize) -> Result<FilePath, ParseError> {
    Ok(FilePath {
        irp: r.field("Irp").ptr(p)?,
        file_object: r.field("FileObject").ptr(p)?,
        file_key: r.field("FileKey").ptr(p)?,
        extra_information: r.field("ExtraInformation").ptr(p)?,
        issuing_tid: r.field("IssuingThreadId").u32()?,
        info_class: r.field("InfoClass").u32()?,
        file_path: r.field("FilePath").wstr()?,
    })
}

/// A registry name that is the event's last field: for the exact version it
/// runs to the final terminator, embedded NULs included (sensor spec §7.5).
fn last_name(r: &mut Reader, field: &'static str, exact: bool) -> Result<WStr, ParseError> {
    // Only the final NUL can leave an empty trailer, so this is never ambiguous.
    if exact {
        r.field(field).counted_wstr(|t| t.is_empty().then_some(0)).map(|(s, _)| s)
    } else {
        r.field(field).wstr()
    }
}

fn reg_open(r: &mut Reader, p: PointerSize, exact: bool) -> Result<RegOpen, ParseError> {
    Ok(RegOpen {
        base_object: r.field("BaseObject").ptr(p)?,
        key_object: r.field("KeyObject").ptr(p)?,
        status: r.field("Status").u32()?,
        disposition: r.field("Disposition").u32()?,
        base_name: r.field("BaseName").wstr()?,
        relative_name: last_name(r, "RelativeName", exact)?,
    })
}

fn reg_key(r: &mut Reader, p: PointerSize) -> Result<RegKey, ParseError> {
    Ok(RegKey {
        key_object: r.field("KeyObject").ptr(p)?,
        status: r.field("Status").u32()?,
        key_name: r.field("KeyName").wstr()?,
    })
}

/// Bytes taken by SetValueKey v0's fields after `ValueName`, if `b` holds them:
/// CapturedDataSize + data, PreviousDataType, PreviousDataSize,
/// PreviousDataCapturedSize + data.
fn set_value_trailer_len(b: &[u8]) -> Option<usize> {
    let size_at = |at: usize| Some(usize::from(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?)));
    let previous_at = 2 + size_at(0)? + 8;
    let total = previous_at + 2 + size_at(previous_at)?;
    (total <= b.len()).then_some(total)
}

fn reg_set_value(r: &mut Reader, p: PointerSize, exact: bool) -> Result<RegSetValue, ParseError> {
    let key_object = r.field("KeyObject").ptr(p)?;
    let status = r.field("Status").u32()?;
    let value_type = r.field("Type").u32()?;
    let data_size = r.field("DataSize").u32()?;
    let key_name = r.field("KeyName").wstr()?;
    // A value name may embed NULs, a known way to hide Run values (§7.5).
    let (value_name, value_name_ambiguous) = if exact {
        r.field("ValueName").counted_wstr(set_value_trailer_len)?
    } else {
        (r.field("ValueName").wstr()?, false)
    };
    let captured_data = captured(r, "CapturedDataSize", "CapturedData")?;
    let previous_data_type = r.field("PreviousDataType").u32()?;
    let previous_data_size = r.field("PreviousDataSize").u32()?;
    let previous_data = captured(r, "PreviousDataCapturedSize", "PreviousData")?;
    Ok(RegSetValue {
        key_object,
        status,
        value_type,
        data_size,
        key_name,
        value_name,
        value_name_ambiguous,
        captured_data,
        previous_data_type,
        previous_data_size,
        previous_data,
    })
}

/// A `u16` byte count followed by that many bytes.
fn captured(r: &mut Reader, size_field: &'static str, data_field: &'static str) -> Result<Box<[u8]>, ParseError> {
    let n = usize::from(r.field(size_field).u16()?);
    let start = r.pos();
    r.field(data_field).skip(n)?;
    Ok(r.consumed_since(start).into())
}

fn reg_delete_value(r: &mut Reader, p: PointerSize, exact: bool) -> Result<RegDeleteValue, ParseError> {
    Ok(RegDeleteValue {
        key_object: r.field("KeyObject").ptr(p)?,
        status: r.field("Status").u32()?,
        key_name: r.field("KeyName").wstr()?,
        value_name: last_name(r, "ValueName", exact)?,
    })
}

fn addr(r: &mut Reader, v6: bool, name: &'static str) -> Result<IpAddr, ParseError> {
    r.field(name);
    Ok(if v6 { IpAddr::V6(Ipv6Addr::from(r.bytes::<16>()?)) } else { IpAddr::V4(Ipv4Addr::from(r.bytes::<4>()?)) })
}

/// `with_tcp_options`: connect and accept carry mss .. sndwinscale between the
/// ports and `seqnum` (16 bytes: 4 × u16, u32, 2 × u16).
fn net(r: &mut Reader, v6: bool, with_tcp_options: bool) -> Result<NetEvent, ParseError> {
    let pid = r.field("PID").u32()?;
    let size = r.field("size").u32()?;
    let daddr = addr(r, v6, "daddr")?;
    let saddr = addr(r, v6, "saddr")?;
    let dport = r.field("dport").u16_be()?;
    let sport = r.field("sport").u16_be()?;
    if with_tcp_options {
        r.field("mss..sndwinscale").skip(16)?;
    }
    let seqnum = r.field("seqnum").u32()?;
    let connid = r.field("connid").u32()?;
    Ok(NetEvent { pid, size, daddr, saddr, dport, sport, seqnum, connid })
}

fn dns_query(r: &mut Reader) -> Result<DnsQuery, ParseError> {
    Ok(DnsQuery {
        query_name: r.field("QueryName").wstr()?,
        query_type: r.field("QueryType").u32()?,
        query_options: r.field("QueryOptions").u64()?,
        query_status: r.field("QueryStatus").u32()?,
        query_results: r.field("QueryResults").wstr()?,
    })
}

/// `UserSID` (MOF `WbemSid`): a null SID is 4 zero bytes; otherwise a
/// `TOKEN_USER` (pointer + attributes, padded to two pointers) then the SID (S8).
fn wbem_sid(r: &mut Reader, p: PointerSize) -> Result<Option<Sid>, ParseError> {
    r.field("UserSID");
    let first = r.rest().get(..4).ok_or(ParseError::Truncated { field: "UserSID", offset: r.pos() })?;
    if first == [0, 0, 0, 0] {
        r.skip(4)?;
        return Ok(None);
    }
    r.skip(match p {
        PointerSize::P32 => 8,
        PointerSize::P64 => 16,
    })?;
    r.sid().map(Some)
}

fn classic_process(r: &mut Reader, p: PointerSize, opcode: u16) -> Result<ClassicProcess, ParseError> {
    let kind = match opcode {
        1 => ClassicKind::Start,
        2 => ClassicKind::End,
        3 => ClassicKind::DcStart,
        _ => ClassicKind::DcEnd,
    };
    Ok(ClassicProcess {
        kind,
        unique_process_key: r.field("UniqueProcessKey").ptr(p)?,
        pid: r.field("ProcessId").u32()?,
        parent_pid: r.field("ParentId").u32()?,
        session_id: r.field("SessionId").u32()?,
        exit_status: r.field("ExitStatus").u32()? as i32,
        directory_table_base: r.field("DirectoryTableBase").ptr(p)?,
        flags: r.field("Flags").u32()?,
        user_sid: wbem_sid(r, p)?,
        image_file_name: r.field("ImageFileName").astr()?,
        command_line: r.field("CommandLine").wstr()?,
        package_full_name: r.field("PackageFullName").wstr()?,
        application_id: r.field("ApplicationId").wstr()?,
    })
}

#[cfg(test)]
mod tests;
```

- [ ] **Step 5: Tests**

`crates/atlas-etw/src/parse/tests.rs`. `payload_for` builds a valid payload for any layout from the table alone. The generic test then checks every layout at both pointer sizes, cut at every byte. The embedded-NUL test uses a payload recorded on the host; it holds only a kernel address:
```rust
use super::*;
use crate::layout::{InType, LAYOUTS};
use proptest::prelude::*;

/// Builds payloads field by field, little-endian, as ETW logs them.
#[derive(Default)]
struct B(Vec<u8>);

impl B {
    fn u16(mut self, v: u16) -> Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u16_be(mut self, v: u16) -> Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    fn u32(mut self, v: u32) -> Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u64(mut self, v: u64) -> Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn ptr(self, p: PointerSize, v: u64) -> Self {
        match p {
            PointerSize::P32 => self.u32(v as u32),
            PointerSize::P64 => self.u64(v),
        }
    }
    fn raw(mut self, b: &[u8]) -> Self {
        self.0.extend(b);
        self
    }
    fn wstr(mut self, s: &str) -> Self {
        for u in s.encode_utf16().chain([0]) {
            self.0.extend(u.to_le_bytes());
        }
        self
    }
    fn astr(mut self, s: &[u8]) -> Self {
        self.0.extend(s);
        self.0.push(0);
        self
    }
    /// S-1-<authority>-<subs...>
    fn sid(mut self, authority: u8, subs: &[u32]) -> Self {
        self.0.extend([1, subs.len() as u8, 0, 0, 0, 0, 0, authority]);
        for s in subs {
            self.0.extend(s.to_le_bytes());
        }
        self
    }
}

fn meta(provider: Provider, id: u16, version: u8, pointer_size: PointerSize) -> EventMeta {
    EventMeta { provider, id, version, pointer_size }
}

/// A valid payload for any layout, driven by the layout table alone.
fn payload_for(fields: &[layout::Field], p: PointerSize) -> Vec<u8> {
    let mut b = B::default();
    let mut last_u16 = None;
    for (name, ty) in fields {
        b = match ty {
            InType::UInt16 => {
                // A size field (CapturedDataSize) is followed by that many bytes.
                let v = if name.ends_with("Size") { 3 } else { 0x1234 };
                last_u16 = Some(v);
                b.u16(v)
            }
            InType::UInt32 | InType::Int32 | InType::HexInt32 => b.u32(0x0102_0304),
            InType::UInt64 | InType::HexInt64 | InType::FileTime => b.u64(0x0102_0304_0506_0708),
            InType::Pointer => b.ptr(p, 0xffff_8000_1234_5678),
            InType::UnicodeString => b.wstr("ab"),
            InType::AnsiString => b.astr(b"ab"),
            InType::Sid => b.sid(16, &[8192]),
            InType::WbemSid => b.ptr(p, 0xffff_8000_0000_0001).ptr(p, 0).sid(5, &[18]),
            InType::Binary => match last_u16.take() {
                Some(n) => b.raw(&vec![0xAB; usize::from(n)]),
                None => b.raw(&[0xFE; 16]), // an IPv6 address
            },
        };
    }
    b.0
}

#[test]
fn every_layout_parses_and_every_strict_prefix_fails() {
    for l in LAYOUTS {
        for p in [PointerSize::P32, PointerSize::P64] {
            let m = meta(l.provider, l.id, l.version, p);
            let full = payload_for(l.fields, p);
            assert!(parse(&m, &full).is_ok(), "{l:?} {p:?}: {:?}", parse(&m, &full));
            for cut in 0..full.len() {
                assert!(parse(&m, &full[..cut]).is_err(), "{l:?} {p:?} parsed with {cut} of {} bytes", full.len());
            }
            // Trailing bytes from a longer, newer layout are ignored.
            let mut longer = full.clone();
            longer.extend([9; 7]);
            assert_eq!(parse(&m, &longer), parse(&m, &full), "{l:?}");
        }
    }
}

#[test]
fn process_start_v4_and_v3() {
    let body = |b: B| {
        b.u32(4242)
            .u64(17)
            .u64(133_000_000_000_000_000)
            .u32(1000)
            .u64(9)
            .u32(1)
            .u32(0)
            .u32(3)
            .u32(1)
            .sid(16, &[12288])
            .wstr(r"\Device\HarddiskVolume3\Windows\System32\cmd.exe")
            .u32(0xAABB)
            .u32(0x5F00_0000)
            .wstr("")
            .wstr("")
    };
    let v4 = body(B::default()).u32(0x10).0;
    let RawEvent::ProcessStart(s) = parse(&meta(Provider::KernelProcess, 1, 4, PointerSize::P64), &v4).unwrap() else {
        panic!()
    };
    assert_eq!((s.pid, s.sequence_number, s.parent_pid, s.parent_sequence_number), (4242, 17, 1000, 9));
    assert_eq!(s.create_time, 133_000_000_000_000_000);
    assert_eq!(s.mandatory_label.to_string(), "S-1-16-12288");
    assert_eq!(s.image_name.to_string_lossy(), r"\Device\HarddiskVolume3\Windows\System32\cmd.exe");
    assert_eq!(s.security_mitigations, Some(0x10));

    let v3 = body(B::default()).0;
    let RawEvent::ProcessStart(s) = parse(&meta(Provider::KernelProcess, 1, 3, PointerSize::P64), &v3).unwrap() else {
        panic!()
    };
    assert_eq!(s.security_mitigations, None);
    assert_eq!(s.time_date_stamp, 0x5F00_0000);
}

#[test]
fn process_stop_skips_the_counters() {
    let mut b = B::default().u32(77).u64(5).u64(1).u64(2).u32(0xC000_0005);
    for _ in 0..2 {
        b = b.u32(0xEE);
    }
    for _ in 0..3 {
        b = b.u64(0xEE);
    }
    for _ in 0..5 {
        b = b.u32(0xEE);
    }
    let b = b.astr(b"notepad.exe");
    let RawEvent::ProcessStop(s) = parse(&meta(Provider::KernelProcess, 2, 2, PointerSize::P64), &b.0).unwrap() else {
        panic!()
    };
    assert_eq!((s.pid, s.sequence_number, s.create_time, s.exit_time), (77, 5, 1, 2));
    assert_eq!(s.exit_code, 0xC000_0005);
    assert_eq!(&*s.image_name, b"notepad.exe");
}

#[test]
fn image_load_with_32_bit_pointers() {
    let p = PointerSize::P32;
    let b = B::default().ptr(p, 0x7700_0000).ptr(p, 0x1000).u32(99).u32(1).u32(2).ptr(p, 0x7700_0000).wstr(r"\x.dll");
    let RawEvent::ImageLoad(i) = parse(&meta(Provider::KernelProcess, 5, 0, p), &b.0).unwrap() else { panic!() };
    assert_eq!((i.image_base, i.image_size, i.pid, i.default_base), (0x7700_0000, 0x1000, 99, 0x7700_0000));
    assert_eq!(i.image_name.to_string_lossy(), r"\x.dll");
}

#[test]
fn file_create_and_delete_on_close() {
    let p = PointerSize::P64;
    let b = B::default().ptr(p, 1).ptr(p, 0xffff_a001).u32(12).u32(0x0100_1000).u32(0x80).u32(7).wstr(r"\a\b.txt");
    for (id, wrap) in [(12, RawEvent::FileCreate as fn(_) -> _), (30, RawEvent::FileCreateNew)] {
        let e = parse(&meta(Provider::KernelFile, id, 1, p), &b.0).unwrap();
        let c = FileCreate {
            irp: 1,
            file_object: 0xffff_a001,
            issuing_tid: 12,
            create_options: 0x0100_1000,
            create_attributes: 0x80,
            share_access: 7,
            file_name: r"\a\b.txt".into(),
        };
        assert!(c.delete_on_close());
        assert_eq!(e, wrap(c));
    }
}

#[test]
fn operation_end_failure_means_error_severity_only() {
    let op = |status| FileOpEnd { irp: 0, extra_information: 0, status };
    assert!(op(0xC000_0035).failed()); // STATUS_OBJECT_NAME_COLLISION
    assert!(op(0xC000_0121).failed()); // STATUS_CANNOT_DELETE
    assert!(!op(0).failed());
    assert!(!op(0x104).failed()); // STATUS_REPARSE
    assert!(!op(0x108).failed()); // STATUS_OPLOCK_BREAK_IN_PROGRESS
    assert!(!op(0x8000_0005).failed()); // STATUS_BUFFER_OVERFLOW: a warning
}

#[test]
fn rename_path_carries_the_new_name() {
    let p = PointerSize::P64;
    let b = B::default().ptr(p, 1).ptr(p, 2).ptr(p, 3).ptr(p, 4).u32(5).u32(65).wstr(r"\new.txt");
    let RawEvent::FileRenamePath(r) = parse(&meta(Provider::KernelFile, 27, 1, p), &b.0).unwrap() else { panic!() };
    assert_eq!((r.irp, r.file_object, r.file_key, r.extra_information, r.issuing_tid), (1, 2, 3, 4, 5));
    assert_eq!((r.info_class, r.file_path.to_string_lossy()), (65, r"\new.txt".to_string()));
}

#[test]
fn set_value_reads_both_captured_buffers() {
    let p = PointerSize::P64;
    let b = B::default()
        .ptr(p, 0xffff_c001)
        .u32(0)
        .u32(4)
        .u32(4)
        .wstr("")
        .wstr("Run")
        .u16(4)
        .raw(&[1, 0, 0, 0])
        .u32(1)
        .u32(6)
        .u16(2)
        .raw(&[b'h', 0]);
    let RawEvent::RegSetValue(v) = parse(&meta(Provider::KernelRegistry, 5, 0, p), &b.0).unwrap() else { panic!() };
    assert_eq!((v.key_object, v.status, v.value_type, v.data_size), (0xffff_c001, 0, 4, 4));
    assert_eq!(v.value_name.to_string_lossy(), "Run");
    assert_eq!((&*v.captured_data, v.previous_data_type, v.previous_data_size), (&[1u8, 0, 0, 0][..], 1, 6));
    assert_eq!(&*v.previous_data, &[b'h', 0]);
}

#[test]
fn a_captured_size_beyond_the_payload_is_truncated() {
    let p = PointerSize::P64;
    let b = B::default().ptr(p, 1).u32(0).u32(1).u32(1).wstr("").wstr("v").u16(0xFFFF).raw(&[1, 2]);
    assert_eq!(
        parse(&meta(Provider::KernelRegistry, 5, 0, p), &b.0),
        // KeyObject 8, Status + Type + DataSize 12, "" 2, "v" 4, CapturedDataSize 2.
        Err(ParseError::Truncated { field: "CapturedData", offset: 28 })
    );
}

#[test]
fn set_value_with_an_embedded_nul_in_the_value_name() {
    // Recorded on the host (plan 1b-2 live run): NtSetValueKey with the counted
    // name "a", NUL, "b". The kernel logs all three units, then the terminator.
    let raw = "e099d2f00e9affff00000000010000000400000000006100000062000000000000000000000000000000";
    let b: Vec<u8> = (0..raw.len()).step_by(2).map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap()).collect();
    let RawEvent::RegSetValue(v) = parse(&meta(Provider::KernelRegistry, 5, 0, PointerSize::P64), &b).unwrap() else {
        panic!()
    };
    assert_eq!(v.value_name.as_units(), &[u16::from(b'a'), 0, u16::from(b'b')]);
    assert_eq!((v.value_type, v.data_size, v.captured_data.len(), v.previous_data.len()), (1, 4, 0, 0));
    assert!(!v.value_name_ambiguous);
    // A newer version whose layout only starts with ours may append fields, so
    // its names stop at the first NUL; one whose layout equals ours stays exact.
    let newer = meta(Provider::KernelRegistry, 5, 1, PointerSize::P64);
    let prefix = parse_as(&newer, 0, &b, false);
    assert!(
        prefix.is_err() || matches!(prefix, Ok(RawEvent::RegSetValue(ref v)) if v.value_name.to_string_lossy() == "a")
    );
    assert!(
        matches!(parse_as(&newer, 0, &b, true), Ok(RawEvent::RegSetValue(ref v)) if v.value_name.as_units().len() == 3)
    );
}

#[test]
fn set_value_flags_a_name_that_could_end_in_two_places() {
    // Previous data that ends in zeros lets a NUL inside it end the name too:
    // "v", terminator, CapturedDataSize 0, PreviousDataType 3, PreviousDataSize 16,
    // PreviousDataCapturedSize 16, then 16 zero bytes.
    let p = PointerSize::P64;
    let b = B::default().ptr(p, 1).u32(0).u32(3).u32(4).wstr("").wstr("v").u16(0).u32(3).u32(16).u16(16).raw(&[0; 16]);
    let RawEvent::RegSetValue(v) = parse(&meta(Provider::KernelRegistry, 5, 0, p), &b.0).unwrap() else { panic!() };
    assert!(v.value_name_ambiguous);
}

#[test]
fn last_field_names_keep_embedded_nuls() {
    let p = PointerSize::P64;
    let units = |s: &str| s.encode_utf16().collect::<Vec<_>>();
    let del = B::default().ptr(p, 1).u32(0).wstr("").wstr("a\0b");
    let RawEvent::RegDeleteValue(d) = parse(&meta(Provider::KernelRegistry, 6, 0, p), &del.0).unwrap() else {
        panic!()
    };
    assert_eq!(d.value_name.as_units(), &units("a\0b")[..]);
    let open = B::default().ptr(p, 0).ptr(p, 2).u32(0).u32(1).wstr("").wstr("k\0x");
    let RawEvent::RegCreateKey(o) = parse(&meta(Provider::KernelRegistry, 1, 0, p), &open.0).unwrap() else { panic!() };
    assert_eq!(o.relative_name.as_units(), &units("k\0x")[..]);
}

#[test]
fn registry_open_close_and_delete_value() {
    let p = PointerSize::P64;
    let open = B::default().ptr(p, 0).ptr(p, 0xffff_d001).u32(0).u32(2).wstr("").wstr(r"\REGISTRY\MACHINE\SOFTWARE");
    let RawEvent::RegOpenKey(o) = parse(&meta(Provider::KernelRegistry, 2, 0, p), &open.0).unwrap() else { panic!() };
    assert_eq!((o.base_object, o.key_object, o.status, o.disposition), (0, 0xffff_d001, 0, 2));
    assert_eq!(o.relative_name.to_string_lossy(), r"\REGISTRY\MACHINE\SOFTWARE");

    let close = B::default().ptr(p, 0xffff_d001).u32(0).wstr("");
    assert_eq!(
        parse(&meta(Provider::KernelRegistry, 13, 0, p), &close.0),
        Ok(RawEvent::RegCloseKey(RegKey { key_object: 0xffff_d001, status: 0, key_name: WStr::default() }))
    );

    let del = B::default().ptr(p, 0xffff_d001).u32(0xC000_0034).wstr("").wstr("Gone");
    let RawEvent::RegDeleteValue(d) = parse(&meta(Provider::KernelRegistry, 6, 0, p), &del.0).unwrap() else {
        panic!()
    };
    assert_eq!((d.status, d.value_name.to_string_lossy()), (0xC000_0034, "Gone".to_string()));
}

#[test]
fn network_ports_are_big_endian_and_v6_addresses_are_16_bytes() {
    let p = PointerSize::P64;
    // TCP connect over IPv4: daddr 93.184.216.34:443 from 10.0.0.5:50000.
    let v4 = B::default()
        .u32(321)
        .u32(0)
        .raw(&[93, 184, 216, 34])
        .raw(&[10, 0, 0, 5])
        .u16_be(443)
        .u16_be(50000)
        .raw(&[0; 16])
        .u32(7)
        .u32(8);
    let RawEvent::TcpConnect(n) = parse(&meta(Provider::KernelNetwork, 12, 0, p), &v4.0).unwrap() else { panic!() };
    assert_eq!(n.pid, 321);
    assert_eq!((n.daddr, n.dport), ("93.184.216.34".parse().unwrap(), 443));
    assert_eq!((n.saddr, n.sport), ("10.0.0.5".parse().unwrap(), 50000));
    assert_eq!((n.seqnum, n.connid), (7, 8));

    // UDP receive over IPv6 (no TCP options).
    let lo = std::net::Ipv6Addr::LOCALHOST.octets();
    let v6 = B::default().u32(55).u32(1200).raw(&lo).raw(&lo).u16_be(53).u16_be(60000).u32(0).u32(0);
    let RawEvent::UdpRecv(n) = parse(&meta(Provider::KernelNetwork, 59, 0, p), &v6.0).unwrap() else { panic!() };
    assert_eq!((n.pid, n.size, n.daddr, n.dport, n.sport), (55, 1200, "::1".parse().unwrap(), 53, 60000));
}

#[test]
fn dns_query_completed() {
    let b = B::default().wstr("example.com").u32(28).u64(0x4000_0000).u32(9003).wstr("");
    let RawEvent::DnsQuery(q) = parse(&meta(Provider::DnsClient, 3008, 0, PointerSize::P32), &b.0).unwrap() else {
        panic!()
    };
    assert_eq!((q.query_name.to_string_lossy(), q.query_type, q.query_status), ("example.com".to_string(), 28, 9003));
    assert!(q.query_results.is_empty());
}

fn classic(p: PointerSize, sid: impl FnOnce(B) -> B) -> Vec<u8> {
    let b = B::default().ptr(p, 0xffff_e001).u32(4242).u32(1000).u32(1).u32(0x103).ptr(p, 0x1aa000).u32(0);
    sid(b).astr(b"cmd.exe").wstr(r#""C:\Windows\system32\cmd.exe" /c echo hi"#).wstr("").wstr("").0
}

#[test]
fn classic_process_with_a_user_sid_both_pointer_sizes() {
    for p in [PointerSize::P32, PointerSize::P64] {
        let b = classic(p, |b| b.ptr(p, 0xffff_9000).ptr(p, 0).sid(5, &[21, 1, 2, 3, 1001]));
        let RawEvent::ClassicProcess(c) = parse(&meta(Provider::ClassicProcess, 1, 4, p), &b).unwrap() else {
            panic!()
        };
        assert_eq!(c.kind, ClassicKind::Start);
        assert_eq!((c.pid, c.parent_pid, c.session_id, c.exit_status), (4242, 1000, 1, 0x103));
        assert_eq!(c.user_sid.unwrap().to_string(), "S-1-5-21-1-2-3-1001");
        assert_eq!(&*c.image_file_name, b"cmd.exe");
        assert_eq!(c.command_line.to_string_lossy(), r#""C:\Windows\system32\cmd.exe" /c echo hi"#);
    }
}

#[test]
fn classic_process_with_a_null_sid() {
    let b = classic(PointerSize::P64, |b| b.u32(0));
    let RawEvent::ClassicProcess(c) = parse(&meta(Provider::ClassicProcess, 3, 4, PointerSize::P64), &b).unwrap()
    else {
        panic!()
    };
    assert_eq!((c.kind, c.user_sid), (ClassicKind::DcStart, None));
    assert_eq!(&*c.image_file_name, b"cmd.exe");
}

#[test]
fn versions_we_do_not_know() {
    let m = |id, v| meta(Provider::KernelProcess, id, v, PointerSize::P64);
    let v4 = payload_for(layout::find(Provider::KernelProcess, 1, 4).unwrap().fields, PointerSize::P64);
    assert_eq!(parse(&m(1, 5), &v4), Err(ParseError::NewerVersion { version: 5, newest: 4 }));
    assert_eq!(parse(&m(1, 2), &v4), Err(ParseError::UnsupportedVersion { version: 2 }));
    assert_eq!(parse(&m(9, 0), &v4), Err(ParseError::UnknownEvent));
    assert_eq!(parse(&meta(Provider::ClassicProcess, 39, 5, PointerSize::P64), &v4), Err(ParseError::UnknownEvent));
    // After a prefix check, a v5 parses with the v4 layout.
    assert!(matches!(parse_as(&m(1, 5), 4, &v4, false), Ok(RawEvent::ProcessStart(_))));
    assert_eq!(parse_as(&m(1, 5), 9, &v4, false), Err(ParseError::UnsupportedVersion { version: 9 }));
}

fn any_meta() -> impl Strategy<Value = EventMeta> {
    (0..LAYOUTS.len(), 0u8..3, any::<bool>()).prop_map(|(i, bump, p64)| {
        let l = &LAYOUTS[i];
        let pointer_size = if p64 { PointerSize::P64 } else { PointerSize::P32 };
        EventMeta { provider: l.provider, id: l.id, version: l.version.saturating_add(bump), pointer_size }
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    /// Arbitrary bytes never panic, for every event and both pointer sizes.
    #[test]
    fn arbitrary_payloads_never_panic(m in any_meta(), bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = parse(&m, &bytes);
        let _ = parse_as(&m, m.version, &bytes, true);
        let _ = parse_as(&m, m.version, &bytes, false);
    }

    /// Flipping bytes of a valid payload never panics either (reaches deeper fields).
    #[test]
    fn mutated_payloads_never_panic(m in any_meta(), flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..8)) {
        let l = layout::newest(m.provider, m.id).unwrap();
        let mut bytes = payload_for(l.fields, m.pointer_size);
        for (i, v) in flips {
            let n = bytes.len();
            bytes[i % n] = v;
        }
        let _ = parse_as(&m, l.version, &bytes, true);
    }
}
```

- [ ] **Step 6: Check and commit**

```powershell
cargo test -p atlas-etw --lib
cargo clippy -p atlas-etw --all-targets -- -D warnings
```
Expected: 41 tests pass (the property tests run 4,096 cases each) and clippy is clean.
```powershell
git add crates/atlas-etw
git commit -m "feat(etw): hand-written event parsers, counted registry names, DNS answers"
```

### Task 3: Sessions, consumer, version gate and TDH

**Files:**
- Modify: `crates/atlas-etw/src/lib.rs`
- Create: `crates/atlas-etw/src/session/mod.rs`, `consumer.rs`, `gate.rs`, `tdh.rs`, `crates/atlas-etw/tests/manifest.rs`

**Interfaces:**
- Consumes: `layout`, `parse`, `providers`.
- Produces: the `session` API listed under "Interfaces for later plans".

- [ ] **Step 1: Wire the module**

`crates/atlas-etw/src/lib.rs`:
```diff
 pub mod layout;
 pub mod parse;
 pub mod providers;
+#[cfg(windows)]
+pub mod session;
```

- [ ] **Step 2: Session control**

`crates/atlas-etw/src/session/mod.rs`. `provider_state` parses `EnumerateTraceGuidsEx`'s output with bounds-checked reads, not pointer casts:
```rust
//! ETW sessions on Windows (sensor spec §4.1, §9.1): start, enable, query and
//! stop them, and consume their events in real time. All of the sensor's ETW
//! `unsafe` code lives in this module, behind a safe API. Starting a session
//! needs administrator rights; the TDH manifest queries do not.

mod consumer;
mod gate;
pub mod tdh;

pub use consumer::{Consumer, EventRecord, consume};
pub use gate::{Verdict, VersionGate, verdict};

use crate::Provider;
use crate::providers::{Enable, LEVEL};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, ERROR_WMI_INSTANCE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Diagnostics::Etw::*;
use windows::core::{GUID, PCWSTR};

/// Session A: the manifest providers (§4.1).
pub const SESSION_A: &str = "Atlas-Sensor";
/// Session B: the system logger, process events only (§4.1).
pub const SESSION_B: &str = "Atlas-Process";

/// A failed ETW call: which function, and the Win32 error it returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EtwError {
    pub op: &'static str,
    pub code: u32,
}

impl std::fmt::Display for EtwError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = windows::core::HRESULT::from_win32(self.code).message();
        write!(f, "{}: error {} ({})", self.op, self.code, msg.trim_end())
    }
}

impl std::error::Error for EtwError {}

fn check(op: &'static str, st: WIN32_ERROR) -> Result<(), EtwError> {
    if st == ERROR_SUCCESS { Ok(()) } else { Err(EtwError { op, code: st.0 }) }
}

/// `EVENT_TRACE_USE_MS_FLUSH_TIMER` (evntrace.h); missing from the `windows` crate.
const EVENT_TRACE_USE_MS_FLUSH_TIMER: u32 = 0x10;

/// Which kind of session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Real-time; providers are enabled with [`Session::enable`].
    Manifest,
    /// Real-time system logger with `EVENT_TRACE_FLAG_PROCESS` only (Session B).
    ProcessLogger,
}

/// Session settings (sensor spec §4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub name: String,
    pub kind: Kind,
    pub buffer_kb: u32,
    pub min_buffers: u32,
    pub max_buffers: u32,
    pub flush_timer_ms: u32,
}

impl Config {
    /// Session A's settings for this machine: 64 KB buffers, min 2 × CPUs,
    /// max max(256, 4 × CPUs), 250 ms flush.
    pub fn session_a(name: &str) -> Config {
        let cpus = std::thread::available_parallelism().map_or(4, |n| n.get() as u32);
        Config {
            name: name.into(),
            kind: Kind::Manifest,
            buffer_kb: 64,
            min_buffers: 2 * cpus,
            max_buffers: (4 * cpus).max(256),
            flush_timer_ms: 250,
        }
    }

    /// Session B's settings: 64 KB buffers, min 4, max 16, 250 ms flush.
    pub fn session_b(name: &str) -> Config {
        Config {
            name: name.into(),
            kind: Kind::ProcessLogger,
            buffer_kb: 64,
            min_buffers: 4,
            max_buffers: 16,
            flush_timer_ms: 250,
        }
    }
}

/// A session's state, from `ControlTraceW` (query or stop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionInfo {
    /// Identifies this instance of the session; a session recreated under the
    /// same name gets a new one (§9.1).
    pub logger_id: u16,
    pub events_lost: u32,
    pub realtime_buffers_lost: u32,
    pub buffers_written: u32,
    pub buffers: u32,
    pub free_buffers: u32,
    pub log_file_mode: u32,
}

/// `EVENT_TRACE_PROPERTIES` followed by room for the logger name, 8-byte aligned.
struct Props(Vec<u64>);

impl Props {
    fn new() -> Props {
        let mut p = Props(vec![0u64; (size_of::<EVENT_TRACE_PROPERTIES>() + 2 * 1024).div_ceil(8)]);
        let len = (p.0.len() * 8) as u32;
        let h = p.get_mut();
        h.Wnode.BufferSize = len;
        h.LoggerNameOffset = size_of::<EVENT_TRACE_PROPERTIES>() as u32;
        p
    }

    fn get_mut(&mut self) -> &mut EVENT_TRACE_PROPERTIES {
        // SAFETY: the buffer is larger than EVENT_TRACE_PROPERTIES and 8-byte aligned;
        // all-zero bytes are a valid value of the struct.
        unsafe { &mut *self.0.as_mut_ptr().cast::<EVENT_TRACE_PROPERTIES>() }
    }

    fn ptr(&mut self) -> *mut EVENT_TRACE_PROPERTIES {
        self.0.as_mut_ptr().cast()
    }

    fn info(&mut self) -> SessionInfo {
        let p = self.get_mut();
        SessionInfo {
            // SAFETY: after a query, HistoricalContext holds the session handle,
            // whose low 16 bits are the LoggerId.
            logger_id: unsafe { p.Wnode.Anonymous1.HistoricalContext } as u16,
            events_lost: p.EventsLost,
            realtime_buffers_lost: p.RealTimeBuffersLost,
            buffers_written: p.BuffersWritten,
            buffers: p.NumberOfBuffers,
            free_buffers: p.FreeBuffers,
            log_file_mode: p.LogFileMode,
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Queries, flushes or stops a session by name. Returns `None` if no session
/// has that name.
fn control_by_name(name: &str, code: EVENT_TRACE_CONTROL, op: &'static str) -> Result<Option<SessionInfo>, EtwError> {
    let name = wide(name);
    let mut props = Props::new();
    // SAFETY: `props` is a properties buffer with room for the name; `name` is NUL-terminated.
    let st = unsafe { ControlTraceW(CONTROLTRACE_HANDLE { Value: 0 }, PCWSTR(name.as_ptr()), props.ptr(), code) };
    if st == ERROR_WMI_INSTANCE_NOT_FOUND {
        return Ok(None);
    }
    check(op, st)?;
    Ok(Some(props.info()))
}

/// The session's state, or `None` if no session has that name (§9.1).
pub fn query_by_name(name: &str) -> Result<Option<SessionInfo>, EtwError> {
    control_by_name(name, EVENT_TRACE_CONTROL_QUERY, "ControlTraceW(query)")
}

/// Stops a session by name (a leftover from a crash, or a blinding test).
/// Returns its final state, or `None` if it did not exist.
pub fn stop_by_name(name: &str) -> Result<Option<SessionInfo>, EtwError> {
    control_by_name(name, EVENT_TRACE_CONTROL_STOP, "ControlTraceW(stop)")
}

fn event_id_filter(ids: &[u16]) -> Vec<u8> {
    // EVENT_FILTER_EVENT_ID: FilterIn (BOOLEAN), Reserved (UCHAR), Count (USHORT), Events[Count].
    let mut f = vec![1u8, 0];
    f.extend((ids.len() as u16).to_le_bytes());
    for id in ids {
        f.extend(id.to_le_bytes());
    }
    f
}

/// Enables `guid` with the start-key property and, if `ids` is non-empty, an
/// event-ID allow-list.
fn enable_raw(handle: CONTROLTRACE_HANDLE, guid: u128, keywords: u64, ids: &[u16]) -> Result<(), EtwError> {
    let filter = event_id_filter(ids);
    let mut desc = EVENT_FILTER_DESCRIPTOR {
        Ptr: filter.as_ptr() as u64,
        Size: filter.len() as u32,
        Type: EVENT_FILTER_TYPE_EVENT_ID,
    };
    let params = ENABLE_TRACE_PARAMETERS {
        Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
        EnableProperty: EVENT_ENABLE_PROPERTY_PROCESS_START_KEY,
        EnableFilterDesc: if ids.is_empty() { std::ptr::null_mut() } else { &mut desc },
        FilterDescCount: u32::from(!ids.is_empty()),
        ..Default::default()
    };
    let guid = GUID::from_u128(guid);
    // SAFETY: `params` and the filter it points to outlive the call.
    let st = unsafe {
        EnableTraceEx2(handle, &guid, EVENT_CONTROL_CODE_ENABLE_PROVIDER.0, LEVEL, keywords, 0, 0, Some(&params))
    };
    check("EnableTraceEx2(enable)", st)
}

fn disable_raw(handle: CONTROLTRACE_HANDLE, provider: Provider) -> Result<(), EtwError> {
    let guid = GUID::from_u128(provider.guid());
    // SAFETY: plain values; no parameters.
    let st = unsafe { EnableTraceEx2(handle, &guid, EVENT_CONTROL_CODE_DISABLE_PROVIDER.0, 0, 0, 0, 0, None) };
    check("EnableTraceEx2(disable)", st)
}

/// `ERROR_WMI_INSTANCE_NOT_FOUND`: the code a stale [`Session`] reports.
pub const STALE: u32 = 4201;

/// A running session. Dropping it stops the session: ETW sessions outlive the
/// process that started them.
///
/// The handle is the session's LoggerId, a small number Windows reuses. If the
/// session was stopped from outside (a crash recovery, a blinding attack, §9.1),
/// another session may hold that number by now. So every call first checks that
/// the session with our name still has our LoggerId, and otherwise does nothing
/// and returns [`STALE`]; dropping a stale session stops nothing. A replacement
/// is a new `Session`; the old one should be [`Session::abandon`]ed.
pub struct Session {
    name: String,
    handle: CONTROLTRACE_HANDLE,
    logger_id: u16,
    stopped: bool,
}

impl Session {
    /// Starts a session. A leftover session with the same name (from a crash)
    /// is stopped first (§4.1).
    pub fn start(cfg: &Config) -> Result<Session, EtwError> {
        stop_by_name(&cfg.name)?;
        let name = wide(&cfg.name);
        let mut props = Props::new();
        let p = props.get_mut();
        p.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        p.Wnode.ClientContext = 1; // raw QPC timestamps (§3.3)
        p.Wnode.Guid = GUID::new().map_err(|e| EtwError { op: "CoCreateGuid", code: e.code().0 as u32 })?;
        p.BufferSize = cfg.buffer_kb;
        p.MinimumBuffers = cfg.min_buffers;
        p.MaximumBuffers = cfg.max_buffers;
        p.FlushTimer = cfg.flush_timer_ms;
        p.LogFileMode = EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_USE_MS_FLUSH_TIMER;
        if cfg.kind == Kind::ProcessLogger {
            p.LogFileMode |= EVENT_TRACE_SYSTEM_LOGGER_MODE;
            p.EnableFlags = EVENT_TRACE_FLAG_PROCESS;
        }
        let mut handle = CONTROLTRACE_HANDLE::default();
        // SAFETY: `props` is a properties buffer with room for the name; `name` is NUL-terminated.
        check("StartTraceW", unsafe { StartTraceW(&mut handle, PCWSTR(name.as_ptr()), props.ptr()) })?;
        let logger_id = handle.Value as u16;
        Ok(Session { name: cfg.name.clone(), handle, logger_id, stopped: false })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The LoggerId this session got at creation (§9.1).
    pub fn logger_id(&self) -> u16 {
        self.logger_id
    }

    /// Enables a provider with its keywords, the event-ID allow-list and the
    /// process start key (§4.2). Enabling again replaces the previous settings.
    pub fn enable(&self, e: &Enable) -> Result<(), EtwError> {
        self.check_current("EnableTraceEx2(enable)")?;
        enable_raw(self.handle, e.provider.guid(), e.keywords, &e.event_ids)
    }

    /// Enables a provider outside the sensor's set, every event ID (diagnostics
    /// and tests).
    pub fn enable_guid(&self, guid: u128, keywords: u64) -> Result<(), EtwError> {
        self.check_current("EnableTraceEx2(enable)")?;
        enable_raw(self.handle, guid, keywords, &[])
    }

    pub fn disable(&self, provider: Provider) -> Result<(), EtwError> {
        self.check_current("EnableTraceEx2(disable)")?;
        disable_raw(self.handle, provider)
    }

    /// Whether the session with our name is still the one we started.
    pub fn is_current(&self) -> bool {
        query_by_name(&self.name).is_ok_and(|i| i.is_some_and(|i| i.logger_id == self.logger_id))
    }

    fn check_current(&self, op: &'static str) -> Result<(), EtwError> {
        if self.is_current() { Ok(()) } else { Err(EtwError { op, code: STALE }) }
    }

    /// Forgets a session that was stopped or replaced from outside, without
    /// touching whatever now holds its LoggerId.
    pub fn abandon(mut self) {
        self.stopped = true;
    }

    /// The session's current state and loss counters.
    pub fn query(&self) -> Result<SessionInfo, EtwError> {
        self.control(EVENT_TRACE_CONTROL_QUERY, "ControlTraceW(query)")
    }

    /// Delivers the buffers' contents now instead of at the next flush tick.
    pub fn flush(&self) -> Result<SessionInfo, EtwError> {
        self.control(EVENT_TRACE_CONTROL_FLUSH, "ControlTraceW(flush)")
    }

    /// Stops the session and returns its final counters. Consumers receive the
    /// remaining events, then `ProcessTrace` returns.
    pub fn stop(mut self) -> Result<SessionInfo, EtwError> {
        self.stopped = true;
        self.control(EVENT_TRACE_CONTROL_STOP, "ControlTraceW(stop)")
    }

    fn control(&self, code: EVENT_TRACE_CONTROL, op: &'static str) -> Result<SessionInfo, EtwError> {
        self.check_current(op)?;
        let mut props = Props::new();
        // SAFETY: `props` is a properties buffer; the handle came from StartTraceW.
        check(op, unsafe { ControlTraceW(self.handle, PCWSTR::null(), props.ptr(), code) })?;
        Ok(props.info())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.control(EVENT_TRACE_CONTROL_STOP, "ControlTraceW(stop)");
        }
    }
}

/// How one session has a provider enabled, from `EnumerateTraceGuidsEx`.
/// The event-ID filter cannot be read back: Windows has no query for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderEnable {
    /// The registered provider instance (0 for kernel providers).
    pub pid: u32,
    pub logger_id: u16,
    pub level: u8,
    pub match_any_keyword: u64,
    pub match_all_keyword: u64,
    pub enable_property: u32,
}

/// Every session's enablement of `provider` (`TraceGuidQueryInfo`, §9.1).
/// Empty if the provider is not registered.
pub fn provider_state(provider: Provider) -> Result<Vec<ProviderEnable>, EtwError> {
    let guid = GUID::from_u128(provider.guid());
    let mut buf = vec![0u64; 64];
    loop {
        let mut needed = 0u32;
        // SAFETY: in = one GUID; out = `buf`, whose byte size is passed.
        let st = unsafe {
            EnumerateTraceGuidsEx(
                TraceGuidQueryInfo,
                Some((&raw const guid).cast()),
                size_of::<GUID>() as u32,
                Some(buf.as_mut_ptr().cast()),
                (buf.len() * 8) as u32,
                &mut needed,
            )
        };
        match st {
            s if s == ERROR_SUCCESS => {
                // SAFETY: viewing initialised u64s as bytes.
                let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len() * 8) };
                return Ok(parse_guid_info(&bytes[..(needed as usize).min(bytes.len())]));
            }
            s if s == ERROR_INSUFFICIENT_BUFFER => buf = vec![0u64; (needed as usize).div_ceil(8)],
            // ERROR_WMI_GUID_NOT_FOUND: no instance of the provider is registered.
            s if s.0 == 4200 => return Ok(Vec::new()),
            s => return Err(EtwError { op: "EnumerateTraceGuidsEx", code: s.0 }),
        }
    }
}

fn le_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn le_u64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Parses `TRACE_GUID_INFO` → `TRACE_PROVIDER_INSTANCE_INFO`s, each followed by
/// its `TRACE_ENABLE_INFO`s, with bounds checks instead of pointer casts.
fn parse_guid_info(b: &[u8]) -> Vec<ProviderEnable> {
    const GUID_INFO: usize = 8; // InstanceCount, Reserved
    const INSTANCE: usize = 16; // NextOffset, EnableCount, Pid, Flags
    const ENABLE: usize = 32; // IsEnabled, Level, Reserved1, LoggerId, EnableProperty, Reserved2, MatchAny, MatchAll
    let mut out = Vec::new();
    let Some(instances) = le_u32(b, 0) else { return out };
    let mut at = GUID_INFO;
    for _ in 0..instances {
        let (Some(next), Some(count), Some(pid)) = (le_u32(b, at), le_u32(b, at + 4), le_u32(b, at + 8)) else {
            break;
        };
        for i in 0..count as usize {
            let e = at + INSTANCE + i * ENABLE;
            let (Some(level), Some(logger_id), Some(prop), Some(any), Some(all)) =
                (b.get(e + 4).copied(), le_u16(b, e + 6), le_u32(b, e + 8), le_u64(b, e + 16), le_u64(b, e + 24))
            else {
                break;
            };
            out.push(ProviderEnable {
                pid,
                logger_id,
                level,
                match_any_keyword: any,
                match_all_keyword: all,
                enable_property: prop,
            });
        }
        if next == 0 {
            break;
        }
        at += next as usize;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_info_parses_instances_and_enables() {
        let mut b = Vec::new();
        b.extend(2u32.to_le_bytes()); // two instances
        b.extend(0u32.to_le_bytes());
        // Instance 1: pid 0, one enable, NextOffset = 16 + 32.
        b.extend(48u32.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(1u32.to_le_bytes()); // IsEnabled
        b.extend([5u8, 0]); // Level, Reserved1
        b.extend(42u16.to_le_bytes()); // LoggerId
        b.extend(0x40u32.to_le_bytes()); // EnableProperty
        b.extend(0u32.to_le_bytes());
        b.extend(0x1EE0u64.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        // Instance 2: pid 1234, no enables, last.
        b.extend(0u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(1234u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        assert_eq!(
            parse_guid_info(&b),
            vec![ProviderEnable {
                pid: 0,
                logger_id: 42,
                level: 5,
                match_any_keyword: 0x1EE0,
                match_all_keyword: 0,
                enable_property: 0x40
            }]
        );
        // Truncated input yields what was whole, never a panic.
        for cut in 0..b.len() {
            let _ = parse_guid_info(&b[..cut]);
        }
    }

    #[test]
    fn event_id_filter_layout() {
        assert_eq!(event_id_filter(&[12, 3008]), vec![1, 0, 2, 0, 12, 0, 0xC0, 0x0B]);
    }
}
```

- [ ] **Step 3: The consumer**

`crates/atlas-etw/src/session/consumer.rs`. The closure lives in a leaked box that the consumer thread reclaims once `ProcessTrace` returns, so no callback can outlive it (review focus 5):
```rust
//! The real-time consumer: `OpenTraceW` + `ProcessTrace` on a dedicated thread,
//! calling a Rust closure for every event (sensor spec §3.2 [1]).

use super::EtwError;
use crate::Provider;
use crate::parse::{EventMeta, PointerSize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use windows::Win32::Foundation::GetLastError;
use windows::Win32::System::Diagnostics::Etw::*;
use windows::core::PWSTR;

/// One event, valid only inside the callback.
pub struct EventRecord<'a> {
    rec: &'a EVENT_RECORD,
}

impl<'a> EventRecord<'a> {
    /// Wraps a record built by a test (the gate's tests rebuild records offline).
    #[cfg(test)]
    pub(crate) fn from_raw(rec: &'a EVENT_RECORD) -> Self {
        EventRecord { rec }
    }

    pub(crate) fn raw(&self) -> *const EVENT_RECORD {
        self.rec
    }

    pub fn provider_guid(&self) -> u128 {
        self.rec.EventHeader.ProviderId.to_u128()
    }

    /// The provider, if it is one we parse.
    pub fn provider(&self) -> Option<Provider> {
        Provider::from_guid(self.provider_guid())
    }

    pub fn id(&self) -> u16 {
        self.rec.EventHeader.EventDescriptor.Id
    }

    pub fn version(&self) -> u8 {
        self.rec.EventHeader.EventDescriptor.Version
    }

    pub fn opcode(&self) -> u8 {
        self.rec.EventHeader.EventDescriptor.Opcode
    }

    /// `EVENT_HEADER.Flags`.
    pub fn flags(&self) -> u16 {
        self.rec.EventHeader.Flags
    }

    /// The logging process (for network events this is not the owner, §5.3).
    pub fn pid(&self) -> u32 {
        self.rec.EventHeader.ProcessId
    }

    pub fn tid(&self) -> u32 {
        self.rec.EventHeader.ThreadId
    }

    /// Raw QPC ticks: sessions use `ClientContext = 1` and the consumer asks
    /// for raw timestamps (§3.3).
    pub fn timestamp(&self) -> i64 {
        self.rec.EventHeader.TimeStamp
    }

    pub fn pointer_size(&self) -> PointerSize {
        if u32::from(self.flags()) & EVENT_HEADER_FLAG_32_BIT_HEADER != 0 { PointerSize::P32 } else { PointerSize::P64 }
    }

    pub fn payload(&self) -> &'a [u8] {
        if self.rec.UserData.is_null() || self.rec.UserDataLength == 0 {
            return &[];
        }
        // SAFETY: ETW guarantees UserData points to UserDataLength bytes for the
        // duration of the callback, which bounds 'a.
        unsafe { std::slice::from_raw_parts(self.rec.UserData.cast::<u8>(), usize::from(self.rec.UserDataLength)) }
    }

    /// The process start key from the extended data, when the provider was
    /// enabled with `EVENT_ENABLE_PROPERTY_PROCESS_START_KEY` (§5.2).
    pub fn start_key(&self) -> Option<u64> {
        let n = usize::from(self.rec.ExtendedDataCount);
        if n == 0 || self.rec.ExtendedData.is_null() {
            return None;
        }
        // SAFETY: ExtendedData points to ExtendedDataCount items during the callback.
        let items = unsafe { std::slice::from_raw_parts(self.rec.ExtendedData, n) };
        items.iter().find_map(|item| {
            (u32::from(item.ExtType) == EVENT_HEADER_EXT_TYPE_PROCESS_START_KEY
                && item.DataSize >= 8
                && item.DataPtr != 0)
                // SAFETY: the item's DataPtr holds DataSize (≥ 8) bytes; read unaligned.
                .then(|| unsafe { (item.DataPtr as *const u64).read_unaligned() })
        })
    }

    /// What the parsers need, if this is a provider we parse. Classic events
    /// are identified by opcode (their ID is 0).
    pub fn meta(&self) -> Option<EventMeta> {
        let provider = self.provider()?;
        let id = if provider == Provider::ClassicProcess { u16::from(self.opcode()) } else { self.id() };
        Some(EventMeta { provider, id, version: self.version(), pointer_size: self.pointer_size() })
    }
}

type Callback = Box<dyn FnMut(&EventRecord) + Send>;

/// A consumer thread. Close it after stopping its session; dropping it closes it too.
pub struct Consumer {
    handle: PROCESSTRACE_HANDLE,
    thread: Option<JoinHandle<u32>>,
    panics: Arc<AtomicU64>,
    qpc_frequency: i64,
    closed: bool,
}

/// `INVALID_PROCESSTRACE_HANDLE`.
const INVALID: u64 = u64::MAX;

struct Context {
    callback: Callback,
    panics: Arc<AtomicU64>,
}

unsafe extern "system" fn trampoline(rec: *mut EVENT_RECORD) {
    // A panic must not unwind into ETW: catch it and count it.
    // SAFETY: ETW passes a valid record whose UserContext is the `Context` we
    // registered, alive until ProcessTrace returns.
    unsafe {
        let rec = &*rec;
        let ctx = &mut *rec.UserContext.cast::<Context>();
        let cb = &mut ctx.callback;
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(&EventRecord { rec }))).is_err() {
            ctx.panics.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Opens a real-time consumer on the session `name` and runs `ProcessTrace` on
/// a new thread, calling `callback` for every event, the session's own header
/// event included (its `meta()` is `None`). The callback must stay fast (§3.2).
pub fn consume(name: &str, callback: impl FnMut(&EventRecord) + Send + 'static) -> Result<Consumer, EtwError> {
    let panics = Arc::new(AtomicU64::new(0));
    let ctx = Box::into_raw(Box::new(Context { callback: Box::new(callback), panics: panics.clone() }));
    let mut name_w: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: an all-zero EVENT_TRACE_LOGFILEW is valid; the fields set below are
    // the ones a real-time EVENT_RECORD consumer needs.
    let mut lf: EVENT_TRACE_LOGFILEW = unsafe { std::mem::zeroed() };
    lf.LoggerName = PWSTR(name_w.as_mut_ptr());
    lf.Anonymous1.ProcessTraceMode =
        PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
    lf.Anonymous2.EventRecordCallback = Some(trampoline);
    lf.Context = ctx.cast();
    // SAFETY: `lf` is initialised as above; OpenTraceW copies what it keeps.
    let handle = unsafe { OpenTraceW(&mut lf) };
    if handle.Value == INVALID {
        // SAFETY: reclaims the context we leaked above; nothing else holds it.
        drop(unsafe { Box::from_raw(ctx) });
        // SAFETY: reads the calling thread's last error.
        return Err(EtwError { op: "OpenTraceW", code: unsafe { GetLastError() }.0 });
    }
    let qpc_frequency = lf.LogfileHeader.PerfFreq;
    let ctx_addr = ctx as usize;
    let thread = std::thread::Builder::new()
        .name(format!("etw-{name}"))
        .spawn(move || {
            // SAFETY: the handle is open; ProcessTrace calls the trampoline on this thread.
            let st = unsafe { ProcessTrace(&[handle], None, None) };
            // SAFETY: ProcessTrace has returned, so no callback can still use the context.
            drop(unsafe { Box::from_raw(ctx_addr as *mut Context) });
            st.0
        })
        .map_err(|_| {
            // SAFETY: the thread never started, so the handle and context are ours to release.
            unsafe {
                let _ = CloseTrace(handle);
                drop(Box::from_raw(ctx_addr as *mut Context));
            }
            EtwError { op: "spawn consumer thread", code: 0 }
        })?;
    Ok(Consumer { handle, thread: Some(thread), panics, qpc_frequency, closed: false })
}

impl Consumer {
    /// The QPC frequency from the session's log file header, for converting
    /// timestamps to wall time (§3.3).
    pub fn qpc_frequency(&self) -> i64 {
        self.qpc_frequency
    }

    /// Callbacks that panicked (each was caught and the event skipped).
    pub fn panics(&self) -> u64 {
        self.panics.load(Ordering::Relaxed)
    }

    /// `ProcessTrace` has returned: the session stopped or was destroyed (§9.1).
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Closes the trace and waits for the thread. Returns `ProcessTrace`'s status.
    /// Called after the session stopped, every event has been delivered first.
    pub fn close(mut self) -> u32 {
        self.close_inner()
    }

    fn close_inner(&mut self) -> u32 {
        if !self.closed {
            self.closed = true;
            // SAFETY: the handle came from OpenTraceW and is closed exactly once.
            let _ = unsafe { CloseTrace(self.handle) };
        }
        self.thread.take().map_or(0, |t| t.join().unwrap_or(u32::MAX))
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        self.close_inner();
    }
}
```

- [ ] **Step 4: TDH**

`crates/atlas-etw/src/session/tdh.rs`:
```rust
//! TDH (Trace Data Helper), used only as an oracle (sensor spec §4.3): once
//! per new (provider, event, version) for the version check, and in tests.
//! Never per event in the agent.

use super::{EtwError, EventRecord};
use crate::Provider;
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
use windows::Win32::System::Diagnostics::Etw::*;
use windows::core::{GUID, PWSTR};

/// A field as TDH describes it: name and in-type.
pub type TdhField = (String, u16);

/// A `TRACE_EVENT_INFO` in an 8-byte-aligned buffer.
struct Info(Vec<u64>);

impl Info {
    /// Calls `f(buffer, size)` until the buffer is large enough.
    fn fetch(
        op: &'static str,
        mut f: impl FnMut(Option<*mut TRACE_EVENT_INFO>, &mut u32) -> u32,
    ) -> Result<Info, EtwError> {
        let mut size = 0u32;
        let st = f(None, &mut size);
        if st != ERROR_INSUFFICIENT_BUFFER.0 {
            return Err(EtwError { op, code: st });
        }
        loop {
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let st = f(Some(buf.as_mut_ptr().cast()), &mut size);
            match st {
                s if s == ERROR_SUCCESS.0 => return Ok(Info(buf)),
                s if s == ERROR_INSUFFICIENT_BUFFER.0 => continue,
                s => return Err(EtwError { op, code: s }),
            }
        }
    }

    fn header(&self) -> &TRACE_EVENT_INFO {
        // SAFETY: TDH filled the buffer with a TRACE_EVENT_INFO at its start, and
        // the buffer is 8-byte aligned (Vec<u64>).
        unsafe { &*self.0.as_ptr().cast::<TRACE_EVENT_INFO>() }
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: viewing initialised u64s as bytes.
        unsafe { std::slice::from_raw_parts(self.0.as_ptr().cast::<u8>(), self.0.len() * 8) }
    }

    fn props(&self) -> &[EVENT_PROPERTY_INFO] {
        let count = self.header().PropertyCount as usize;
        trailing_array(&self.0, std::mem::offset_of!(TRACE_EVENT_INFO, EventPropertyInfoArray), count)
    }

    /// The NUL-terminated UTF-16 string at byte offset `off` (0 = none).
    fn string_at(&self, off: u32) -> String {
        let b = self.bytes();
        let start = off as usize;
        if off == 0 || start >= b.len() {
            return String::new();
        }
        let units: Vec<u16> =
            b[start..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
        String::from_utf16_lossy(&units)
    }

    /// Top-level fields with their in-types. Struct fields are reported with
    /// in-type 0 (none of our events has one).
    fn fields(&self) -> Vec<TdhField> {
        let h = self.header();
        self.props()
            .iter()
            .take(h.TopLevelPropertyCount as usize)
            .map(|p| {
                let in_type = if p.Flags.0 & PropertyStruct.0 != 0 {
                    0
                } else {
                    // SAFETY: nonStructType is the active variant when PropertyStruct is clear.
                    unsafe { p.Anonymous1.nonStructType.InType }
                };
                (self.string_at(p.NameOffset), in_type)
            })
            .collect()
    }
}

/// The `count` entries of a variable-length array that starts `offset` bytes into
/// `buf` (a C struct's trailing `T[1]`), clamped to what the buffer holds. The
/// pointer is derived from the whole buffer, not from a reference to the struct's
/// one-element array, so the slice stays in bounds of its provenance.
fn trailing_array<T>(buf: &[u64], offset: usize, count: usize) -> &[T] {
    let bytes = buf.len() * 8;
    let fits = bytes.saturating_sub(offset) / size_of::<T>();
    debug_assert!(offset.is_multiple_of(align_of::<T>()) && align_of::<T>() <= 8);
    // SAFETY: `buf` is 8-byte aligned and `offset` is a field offset of a struct
    // that TDH wrote at its start, so the address is aligned for T; at most `fits`
    // entries lie inside `buf`, and TDH initialised them (all-zero is valid too).
    unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>().add(offset).cast::<T>(), count.min(fits)) }
}

/// The installed manifest's layout of (provider, event, version), or `None`
/// if the manifest has no such event. Needs no elevation.
pub fn manifest_layout(provider: Provider, id: u16, version: u8) -> Result<Option<Vec<TdhField>>, EtwError> {
    let guid = GUID::from_u128(provider.guid());
    // TdhGetManifestEventInformation wants the full descriptor, so find it first.
    let mut size = 0u32;
    // SAFETY: a size query with no buffer.
    let st = unsafe { TdhEnumerateManifestProviderEvents(&guid, None, &mut size) };
    if st != ERROR_INSUFFICIENT_BUFFER.0 {
        return Err(EtwError { op: "TdhEnumerateManifestProviderEvents", code: st });
    }
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let pe = buf.as_mut_ptr().cast::<PROVIDER_EVENT_INFO>();
    // SAFETY: the buffer holds `size` bytes, 8-byte aligned.
    let st = unsafe { TdhEnumerateManifestProviderEvents(&guid, Some(pe), &mut size) };
    if st != ERROR_SUCCESS.0 {
        return Err(EtwError { op: "TdhEnumerateManifestProviderEvents", code: st });
    }
    // SAFETY: TDH wrote a PROVIDER_EVENT_INFO at the start of the buffer.
    let count = unsafe { (*pe).NumberOfEvents } as usize;
    let descs: &[EVENT_DESCRIPTOR] =
        trailing_array(&buf, std::mem::offset_of!(PROVIDER_EVENT_INFO, EventDescriptorsArray), count);
    let Some(d) = descs.iter().find(|d| d.Id == id && d.Version == version) else { return Ok(None) };
    let info = Info::fetch("TdhGetManifestEventInformation", |b, s| {
        // SAFETY: `b` is None or a buffer of `*s` bytes.
        unsafe { TdhGetManifestEventInformation(&guid, d, b, s) }
    })?;
    Ok(Some(info.fields()))
}

/// TDH's layout of a classic (MOF) event of Session B's process class, from a
/// record built here: TDH needs only the class GUID, opcode, version and header
/// flags to find the MOF description, so this needs no session and no elevation.
pub fn classic_layout(opcode: u8, version: u8) -> Result<Vec<TdhField>, EtwError> {
    // SAFETY: an all-zero EVENT_RECORD is valid; TDH reads only the header fields set below.
    let mut rec: EVENT_RECORD = unsafe { std::mem::zeroed() };
    rec.EventHeader.ProviderId = GUID::from_u128(Provider::ClassicProcess.guid());
    rec.EventHeader.EventDescriptor.Opcode = opcode;
    rec.EventHeader.EventDescriptor.Version = version;
    rec.EventHeader.Flags = (EVENT_HEADER_FLAG_CLASSIC_HEADER | EVENT_HEADER_FLAG_64_BIT_HEADER) as u16;
    let info = Info::fetch("TdhGetEventInformation", |b, s| {
        // SAFETY: `rec` lives for the call; `b` is None or a buffer of `*s` bytes.
        unsafe { TdhGetEventInformation(&rec, None, b, s) }
    })?;
    Ok(info.fields())
}

fn event_info(rec: &EventRecord) -> Result<Info, EtwError> {
    Info::fetch("TdhGetEventInformation", |b, s| {
        // SAFETY: the record is valid for the duration of the callback that holds it.
        unsafe { TdhGetEventInformation(rec.raw(), None, b, s) }
    })
}

/// TDH's layout of a live event (manifest or classic MOF).
pub fn event_layout(rec: &EventRecord) -> Result<Vec<TdhField>, EtwError> {
    event_info(rec).map(|i| i.fields())
}

/// Decodes a live event into (field name, TDH's text) pairs, in payload order.
/// For tests and fixture recording only: it costs microseconds per event.
pub fn decode(rec: &EventRecord) -> Result<Vec<(String, String)>, EtwError> {
    let info = event_info(rec)?;
    let data = rec.payload();
    let ptr_size: u32 = match rec.pointer_size() {
        crate::parse::PointerSize::P32 => 4,
        crate::parse::PointerSize::P64 => 8,
    };
    let props = info.props();
    let mut ints: Vec<Option<u64>> = vec![None; props.len()];
    let mut offset = 0usize;
    let mut out = Vec::new();
    for (i, p) in props.iter().enumerate().take(info.header().TopLevelPropertyCount as usize) {
        let name = info.string_at(p.NameOffset);
        let flags = p.Flags.0;
        if flags & (PropertyStruct.0 | PropertyParamCount.0 | PropertyParamFixedCount.0) != 0 {
            return Err(EtwError { op: "decode: struct or array field", code: 0 });
        }
        // SAFETY: the non-struct variants are active (checked above).
        let (in_type, out_type, length) = unsafe {
            let t = p.Anonymous1.nonStructType;
            let len = if flags & PropertyParamLength.0 != 0 {
                ints.get(p.Anonymous3.lengthPropertyIndex as usize).copied().flatten().unwrap_or(0) as u16
            } else {
                p.Anonymous3.length
            };
            (t.InType, t.OutType, len)
        };
        if flags & PropertyParamLength.0 != 0 && length == 0 {
            out.push((name, String::new()));
            continue;
        }
        let rest = &data[offset.min(data.len())..];
        ints[i] = read_int(rest, in_type, ptr_size);
        let mut text = vec![0u16; 256];
        loop {
            let mut size_bytes = (text.len() * 2) as u32;
            let mut consumed = 0u16;
            // SAFETY: `info` is the TRACE_EVENT_INFO for this event; `rest` and `text`
            // are valid buffers of the sizes passed.
            let st = unsafe {
                TdhFormatProperty(
                    info.0.as_ptr().cast(),
                    None,
                    ptr_size,
                    in_type,
                    out_type,
                    length,
                    &rest[..rest.len().min(u16::MAX as usize)],
                    &mut size_bytes,
                    Some(PWSTR(text.as_mut_ptr())),
                    &mut consumed,
                )
            };
            if st == ERROR_INSUFFICIENT_BUFFER.0 {
                text = vec![0u16; (size_bytes as usize).div_ceil(2)];
                continue;
            }
            if st != ERROR_SUCCESS.0 {
                return Err(EtwError { op: "TdhFormatProperty", code: st });
            }
            offset += usize::from(consumed);
            let n = text.iter().position(|&c| c == 0).unwrap_or(text.len());
            out.push((name, String::from_utf16_lossy(&text[..n])));
            break;
        }
    }
    Ok(out)
}

/// The integer at the start of `b`, for fields that size later ones.
fn read_int(b: &[u8], in_type: u16, ptr_size: u32) -> Option<u64> {
    let n = match i32::from(in_type) {
        x if x == TDH_INTYPE_INT8.0 || x == TDH_INTYPE_UINT8.0 => 1,
        x if x == TDH_INTYPE_INT16.0 || x == TDH_INTYPE_UINT16.0 => 2,
        x if x == TDH_INTYPE_INT32.0 || x == TDH_INTYPE_UINT32.0 || x == TDH_INTYPE_HEXINT32.0 => 4,
        x if x == TDH_INTYPE_INT64.0 || x == TDH_INTYPE_UINT64.0 || x == TDH_INTYPE_HEXINT64.0 => 8,
        x if x == TDH_INTYPE_POINTER.0 || x == TDH_INTYPE_SIZET.0 => ptr_size as usize,
        _ => return None,
    };
    let mut v = [0u8; 8];
    v[..n].copy_from_slice(b.get(..n)?);
    Some(u64::from_le_bytes(v))
}
```

- [ ] **Step 5: The version gate**

`crates/atlas-etw/src/session/gate.rs`. Its tests rebuild records offline (F8):
```rust
//! Newer event versions (sensor spec §4.3): the first time a (provider, event,
//! version) newer than our newest layout is seen, ask TDH once for that
//! version's layout **as installed on this machine** and accept it if ours is a
//! prefix of it. The verdict is cached.
//!
//! The installed description is the manifest, or the MOF class for Session B's
//! classic events. It is never read from the event itself: a user-mode provider
//! (DNS-Client) can be forged (§4.4), and a forged event could carry its own
//! schema and so decide a verdict for every later genuine event.

use super::{EventRecord, tdh};
use crate::Provider;
use crate::layout::{self, Field};
use crate::parse::{ParseError, RawEvent, parse, parse_as};
use std::collections::HashMap;

/// What a newer version's installed layout says about ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Not a prefix, or no installed description: dropped as `unknown_version`.
    Rejected,
    /// Ours is a strict prefix: parsed with our layout; a name that ends our
    /// layout stops at its first NUL, because fields may follow it.
    Prefix,
    /// The same fields: parsed exactly as our version.
    Same,
}

/// The verdict for our layout against a newer version's installed layout.
pub fn verdict(ours: &[Field], installed: Option<&[tdh::TdhField]>) -> Verdict {
    match installed {
        Some(theirs) if layout::is_prefix(ours, theirs) => {
            if theirs.len() == ours.len() {
                Verdict::Same
            } else {
                Verdict::Prefix
            }
        }
        _ => Verdict::Rejected,
    }
}

/// The installed layout of a version, or `None` if Windows has no description of it.
fn installed_layout(provider: Provider, id: u16, version: u8) -> Option<Vec<tdh::TdhField>> {
    match provider {
        Provider::ClassicProcess => tdh::classic_layout(u8::try_from(id).ok()?, version).ok(),
        _ => tdh::manifest_layout(provider, id, version).ok().flatten(),
    }
}

/// Parses live events, deciding once per newer version whether to accept it.
#[derive(Default)]
pub struct VersionGate {
    verdicts: HashMap<(Provider, u16, u8), Verdict>,
}

impl VersionGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses `rec`. A newer version that failed the prefix check, or whose TDH
    /// layout could not be read, stays `Err(NewerVersion)`: the caller counts it
    /// as `unknown_version` and drops it.
    pub fn parse(&mut self, rec: &EventRecord) -> Result<RawEvent, ParseError> {
        let meta = rec.meta().ok_or(ParseError::UnknownEvent)?;
        match parse(&meta, rec.payload()) {
            Err(ParseError::NewerVersion { version, newest }) => {
                let v = *self.verdicts.entry((meta.provider, meta.id, version)).or_insert_with(|| {
                    let ours = layout::find(meta.provider, meta.id, newest).map(|l| l.fields).unwrap_or_default();
                    verdict(ours, installed_layout(meta.provider, meta.id, version).as_deref())
                });
                match v {
                    Verdict::Same => parse_as(&meta, newest, rec.payload(), true),
                    Verdict::Prefix => parse_as(&meta, newest, rec.payload(), false),
                    Verdict::Rejected => Err(ParseError::NewerVersion { version, newest }),
                }
            }
            other => other,
        }
    }

    /// Every newer version seen so far and its verdict, for logs and Sensor Health.
    pub fn verdicts(&self) -> impl Iterator<Item = ((Provider, u16, u8), Verdict)> + '_ {
        self.verdicts.iter().map(|(k, v)| (*k, *v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Diagnostics::Etw::{EVENT_HEADER_FLAG_64_BIT_HEADER, EVENT_RECORD};
    use windows::core::GUID;

    /// A record as ETW would deliver it, built offline: TDH needs only the header
    /// to find an event's description, so no session or elevation is involved.
    fn record(provider: Provider, id: u16, version: u8, payload: &mut [u8]) -> EVENT_RECORD {
        // SAFETY: an all-zero EVENT_RECORD is valid; the fields used are set below.
        let mut rec: EVENT_RECORD = unsafe { std::mem::zeroed() };
        rec.EventHeader.ProviderId = GUID::from_u128(provider.guid());
        rec.EventHeader.EventDescriptor.Id = id;
        rec.EventHeader.EventDescriptor.Version = version;
        rec.EventHeader.Flags = EVENT_HEADER_FLAG_64_BIT_HEADER as u16;
        rec.UserData = payload.as_mut_ptr().cast();
        rec.UserDataLength = payload.len() as u16;
        rec
    }

    /// Kernel-Registry CloseKey v0: KeyObject, Status, KeyName "".
    fn close_key_payload() -> Vec<u8> {
        let mut b = 0xffff_d001u64.to_le_bytes().to_vec();
        b.extend(0u32.to_le_bytes());
        b.extend([0, 0]);
        b
    }

    #[test]
    fn a_known_version_parses_without_a_verdict() {
        let mut payload = close_key_payload();
        let rec = record(Provider::KernelRegistry, 13, 0, &mut payload);
        let mut gate = VersionGate::new();
        assert!(
            matches!(gate.parse(&EventRecord::from_raw(&rec)), Ok(RawEvent::RegCloseKey(k)) if k.key_object == 0xffff_d001)
        );
        assert_eq!(gate.verdicts().count(), 0);
    }

    #[test]
    fn a_newer_version_unknown_to_tdh_is_rejected_and_remembered() {
        let mut payload = close_key_payload();
        // No manifest has a CloseKey version 9, so TDH cannot describe it.
        let rec = record(Provider::KernelRegistry, 13, 9, &mut payload);
        let mut gate = VersionGate::new();
        for _ in 0..2 {
            assert_eq!(
                gate.parse(&EventRecord::from_raw(&rec)),
                Err(ParseError::NewerVersion { version: 9, newest: 0 })
            );
        }
        assert_eq!(gate.verdicts().collect::<Vec<_>>(), vec![((Provider::KernelRegistry, 13, 9), Verdict::Rejected)]);
    }

    #[test]
    fn the_verdict_uses_the_installed_layout() {
        // A real older/newer pair from the installed manifest: ProcessStart v3 is a
        // strict prefix of v4 (v4 appends SecurityMitigations).
        let v3 = layout::find(Provider::KernelProcess, 1, 3).unwrap().fields;
        let v4 = layout::find(Provider::KernelProcess, 1, 4).unwrap().fields;
        let installed_v4 = installed_layout(Provider::KernelProcess, 1, 4).expect("v4 is in the manifest");
        assert_eq!(verdict(v3, Some(&installed_v4)), Verdict::Prefix);
        assert_eq!(verdict(v4, Some(&installed_v4)), Verdict::Same);
        // v4 is not a prefix of v3, and no description means rejection.
        let installed_v3 = installed_layout(Provider::KernelProcess, 1, 3).unwrap();
        assert_eq!(verdict(v4, Some(&installed_v3)), Verdict::Rejected);
        assert_eq!(verdict(v4, None), Verdict::Rejected);
        assert_eq!(installed_layout(Provider::KernelProcess, 1, 99), None);
        // The classic class is looked up through its MOF description.
        let classic = layout::find(Provider::ClassicProcess, 1, 4).unwrap().fields;
        assert_eq!(verdict(classic, installed_layout(Provider::ClassicProcess, 1, 4).as_deref()), Verdict::Same);
    }

    #[test]
    fn other_providers_are_unknown_events() {
        let mut payload = close_key_payload();
        let mut rec = record(Provider::KernelRegistry, 13, 0, &mut payload);
        rec.EventHeader.ProviderId = GUID::from_u128(0x1234);
        assert_eq!(VersionGate::new().parse(&EventRecord::from_raw(&rec)), Err(ParseError::UnknownEvent));
    }
}
```

- [ ] **Step 6: The manifest test**

`crates/atlas-etw/tests/manifest.rs`:
```rust
//! Our layout tables against the installed provider manifests (sensor spec
//! §4.3). Needs Windows, not elevation: TDH reads the manifests directly.
#![cfg(windows)]

use atlas_etw::Provider;
use atlas_etw::layout::LAYOUTS;
use atlas_etw::session::tdh::{classic_layout, manifest_layout};

#[test]
fn every_manifest_layout_matches_the_installed_manifest() {
    let mut failures = Vec::new();
    for l in LAYOUTS.iter().filter(|l| l.provider != Provider::ClassicProcess) {
        let ours: Vec<(String, u16)> = l.fields.iter().map(|(n, t)| (n.to_string(), *t as u16)).collect();
        match manifest_layout(l.provider, l.id, l.version) {
            Ok(Some(theirs)) if theirs == ours => {}
            Ok(Some(theirs)) => failures
                .push(format!("{:?} {} v{}:\n  ours   {ours:?}\n  theirs {theirs:?}", l.provider, l.id, l.version)),
            // A build whose manifest lacks this version (older Windows) logs another
            // version, which the replay and live tests cover.
            Ok(None) => eprintln!("{:?} {} v{}: not in this build's manifest", l.provider, l.id, l.version),
            Err(e) => failures.push(format!("{:?} {} v{}: {e}", l.provider, l.id, l.version)),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_classic_process_layout_matches_its_mof_class() {
    for l in LAYOUTS.iter().filter(|l| l.provider == Provider::ClassicProcess) {
        let ours: Vec<(String, u16)> = l.fields.iter().map(|(n, t)| (n.to_string(), *t as u16)).collect();
        let theirs = classic_layout(l.id as u8, l.version).unwrap_or_else(|e| panic!("opcode {}: {e}", l.id));
        assert_eq!(theirs, ours, "opcode {} v{}", l.id, l.version);
    }
}
```

- [ ] **Step 7: Check and commit**

```powershell
cargo test -p atlas-etw
cargo clippy -p atlas-etw --all-targets -- -D warnings
```
Expected: 47 unit tests and the 2 manifest tests pass, unelevated. Clippy is clean.
```powershell
git add crates/atlas-etw
git commit -m "feat(etw): sessions, real-time consumer, version gate and TDH oracle"
```

### Task 4: Replay (tier 2)

**Files:**
- Create: `crates/atlas-etw/tests/common/mod.rs`, `tests/replay.rs`

**Interfaces:**
- Produces: the fixture line format (written by Task 5, read here) and `common::compare`, the oracle comparison. The comparison accepts TDH's first-NUL truncation of a counted name (F4).

- [ ] **Step 1: Fixture format and oracle**

`crates/atlas-etw/tests/common/mod.rs`:
```rust
//! Shared by the replay and live tests: the fixture line format and the TDH
//! oracle comparison (sensor spec §4.3, §12.2).
#![allow(dead_code)] // each test binary uses a different part

use atlas_etw::Provider;
use atlas_etw::parse::{EventMeta, PointerSize, RawEvent, WStr};
use serde_json::{Map, Value};
use std::net::IpAddr;

/// `EVENT_HEADER_FLAG_32_BIT_HEADER`.
pub const FLAG_32_BIT: u16 = 0x0020;

/// Short provider names used in fixture lines (the spike probe's names, so its
/// recordings replay too).
pub fn short_name(p: Provider) -> &'static str {
    match p {
        Provider::KernelProcess => "kernel-process",
        Provider::KernelFile => "kernel-file",
        Provider::KernelRegistry => "kernel-registry",
        Provider::KernelNetwork => "kernel-network",
        Provider::DnsClient => "dns-client",
        Provider::ClassicProcess => "process-classic",
    }
}

pub fn from_short_name(s: &str) -> Option<Provider> {
    Provider::ALL.into_iter().find(|p| short_name(*p) == s)
}

/// One fixture line, as written by the recorder (`tests/live.rs`).
#[derive(Debug)]
pub struct Fixture {
    pub meta: EventMeta,
    pub pid: u32,
    pub tid: u32,
    pub timestamp: i64,
    pub start_key: Option<u64>,
    pub payload: Vec<u8>,
    /// TDH's decoding on the machine that recorded the event.
    pub tdh: Map<String, Value>,
}

fn hex_u64(s: &str) -> Option<u64> {
    u64::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

pub fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

pub fn encode_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Fixture {
    /// Reads one line. `None` for lines that are not parseable events (no raw
    /// payload, a provider or event we don't parse).
    pub fn from_json(line: &str) -> Option<Fixture> {
        let v: Value = serde_json::from_str(line).ok()?;
        let provider = from_short_name(v["provider"].as_str()?)?;
        let flags = u16::try_from(hex_u64(v["flags"].as_str()?)?).ok()?;
        // Classic events have ID 0 and are told apart by opcode.
        let id = if provider == Provider::ClassicProcess { v["opcode"].as_u64()? } else { v["id"].as_u64()? };
        let meta = EventMeta {
            provider,
            id: u16::try_from(id).ok()?,
            version: u8::try_from(v["version"].as_u64()?).ok()?,
            pointer_size: if flags & FLAG_32_BIT != 0 { PointerSize::P32 } else { PointerSize::P64 },
        };
        Some(Fixture {
            meta,
            pid: u32::try_from(v["pid"].as_u64()?).ok()?,
            tid: u32::try_from(v["tid"].as_u64()?).ok()?,
            timestamp: v["ts"].as_i64()?,
            start_key: v["start_key"].as_str().and_then(hex_u64),
            payload: decode_hex(v["raw"].as_str()?)?,
            tdh: v["fields"].as_object()?.clone(),
        })
    }
}

/// How one parsed field is compared with TDH's text.
#[derive(Debug)]
pub enum Expect {
    /// An integer; TDH prints it in decimal or `0x` hex depending on its out-type.
    Num(u64),
    /// A signed integer, printed in decimal.
    Signed(i64),
    Str(String),
    /// A FILETIME; TDH prints an ISO 8601 UTC time (with direction marks).
    Time(u64),
    Addr(IpAddr),
    /// A binary blob; TDH prints `0x` + uppercase hex, or nothing when empty.
    Bytes(Vec<u8>),
    /// Not compared here: a field we skip over, or one TDH renders in a form
    /// that cannot be checked offline (a SID shown as an account name).
    Skip,
}

fn s(w: &WStr) -> Expect {
    Expect::Str(w.to_string_lossy())
}

fn a(b: &[u8]) -> Expect {
    Expect::Str(String::from_utf8_lossy(b).into_owned())
}

/// Every TDH field of the event, with what our parser produced for it.
pub fn expected_fields(e: &RawEvent) -> Vec<(&'static str, Expect)> {
    use Expect::*;
    match e {
        RawEvent::ProcessStart(p) => {
            let mut v = vec![
                ("ProcessID", Num(p.pid.into())),
                ("ProcessSequenceNumber", Num(p.sequence_number)),
                ("CreateTime", Time(p.create_time)),
                ("ParentProcessID", Num(p.parent_pid.into())),
                ("ParentProcessSequenceNumber", Num(p.parent_sequence_number)),
                ("SessionID", Num(p.session_id.into())),
                ("Flags", Num(p.flags.into())),
                ("ProcessTokenElevationType", Num(p.token_elevation_type.into())),
                ("ProcessTokenIsElevated", Num(p.token_is_elevated.into())),
                ("MandatoryLabel", Str(p.mandatory_label.to_string())),
                ("ImageName", s(&p.image_name)),
                ("ImageChecksum", Num(p.image_checksum.into())),
                ("TimeDateStamp", Num(p.time_date_stamp.into())),
                ("PackageFullName", s(&p.package_full_name)),
                ("PackageRelativeAppId", s(&p.package_relative_app_id)),
            ];
            if let Some(m) = p.security_mitigations {
                v.push(("SecurityMitigations", Num(m.into())));
            }
            v
        }
        RawEvent::ProcessStop(p) => {
            let mut v = vec![
                ("ProcessID", Num(p.pid.into())),
                ("ProcessSequenceNumber", Num(p.sequence_number)),
                ("CreateTime", Time(p.create_time)),
                ("ExitTime", Time(p.exit_time)),
                ("ExitCode", Num(p.exit_code.into())),
                ("ImageName", a(&p.image_name)),
            ];
            for skipped in [
                "TokenElevationType",
                "HandleCount",
                "CommitCharge",
                "CommitPeak",
                "CPUCycleCount",
                "ReadOperationCount",
                "WriteOperationCount",
                "ReadTransferKiloBytes",
                "WriteTransferKiloBytes",
                "HardFaultCount",
            ] {
                v.push((skipped, Skip));
            }
            v
        }
        RawEvent::ImageLoad(i) => vec![
            ("ImageBase", Num(i.image_base)),
            ("ImageSize", Num(i.image_size)),
            ("ProcessID", Num(i.pid.into())),
            ("ImageCheckSum", Num(i.image_checksum.into())),
            ("TimeDateStamp", Num(i.time_date_stamp.into())),
            ("DefaultBase", Num(i.default_base)),
            ("ImageName", s(&i.image_name)),
        ],
        RawEvent::FileCreate(c) | RawEvent::FileCreateNew(c) => vec![
            ("Irp", Num(c.irp)),
            ("FileObject", Num(c.file_object)),
            ("IssuingThreadId", Num(c.issuing_tid.into())),
            ("CreateOptions", Num(c.create_options.into())),
            ("CreateAttributes", Num(c.create_attributes.into())),
            ("ShareAccess", Num(c.share_access.into())),
            ("FileName", s(&c.file_name)),
        ],
        RawEvent::FileCleanup(h) | RawEvent::FileClose(h) => vec![
            ("Irp", Num(h.irp)),
            ("FileObject", Num(h.file_object)),
            ("FileKey", Num(h.file_key)),
            ("IssuingThreadId", Num(h.issuing_tid.into())),
        ],
        RawEvent::FileWrite(w) => vec![
            ("ByteOffset", Num(w.byte_offset)),
            ("Irp", Num(w.irp)),
            ("FileObject", Num(w.file_object)),
            ("FileKey", Num(w.file_key)),
            ("IssuingThreadId", Num(w.issuing_tid.into())),
            ("IOSize", Num(w.io_size.into())),
            ("IOFlags", Num(w.io_flags.into())),
            ("ExtraFlags", Num(w.extra_flags.into())),
        ],
        RawEvent::FileSetInfo(i) => vec![
            ("Irp", Num(i.irp)),
            ("FileObject", Num(i.file_object)),
            ("FileKey", Num(i.file_key)),
            ("ExtraInformation", Num(i.extra_information)),
            ("IssuingThreadId", Num(i.issuing_tid.into())),
            ("InfoClass", Num(i.info_class.into())),
        ],
        RawEvent::FileOpEnd(o) => {
            vec![("Irp", Num(o.irp)), ("ExtraInformation", Num(o.extra_information)), ("Status", Num(o.status.into()))]
        }
        RawEvent::FileDeletePath(p) | RawEvent::FileRenamePath(p) => vec![
            ("Irp", Num(p.irp)),
            ("FileObject", Num(p.file_object)),
            ("FileKey", Num(p.file_key)),
            ("ExtraInformation", Num(p.extra_information)),
            ("IssuingThreadId", Num(p.issuing_tid.into())),
            ("InfoClass", Num(p.info_class.into())),
            ("FilePath", s(&p.file_path)),
        ],
        RawEvent::RegCreateKey(o) | RawEvent::RegOpenKey(o) => vec![
            ("BaseObject", Num(o.base_object)),
            ("KeyObject", Num(o.key_object)),
            ("Status", Num(o.status.into())),
            ("Disposition", Num(o.disposition.into())),
            ("BaseName", s(&o.base_name)),
            ("RelativeName", s(&o.relative_name)),
        ],
        RawEvent::RegDeleteKey(k) | RawEvent::RegCloseKey(k) => {
            vec![("KeyObject", Num(k.key_object)), ("Status", Num(k.status.into())), ("KeyName", s(&k.key_name))]
        }
        RawEvent::RegSetValue(v) => vec![
            ("KeyObject", Num(v.key_object)),
            ("Status", Num(v.status.into())),
            ("Type", Num(v.value_type.into())),
            ("DataSize", Num(v.data_size.into())),
            ("KeyName", s(&v.key_name)),
            ("ValueName", s(&v.value_name)),
            ("CapturedDataSize", Num(v.captured_data.len() as u64)),
            ("CapturedData", Bytes(v.captured_data.to_vec())),
            ("PreviousDataType", Num(v.previous_data_type.into())),
            ("PreviousDataSize", Num(v.previous_data_size.into())),
            ("PreviousDataCapturedSize", Num(v.previous_data.len() as u64)),
            ("PreviousData", Bytes(v.previous_data.to_vec())),
        ],
        RawEvent::RegDeleteValue(v) => vec![
            ("KeyObject", Num(v.key_object)),
            ("Status", Num(v.status.into())),
            ("KeyName", s(&v.key_name)),
            ("ValueName", s(&v.value_name)),
        ],
        RawEvent::TcpConnect(n)
        | RawEvent::TcpAccept(n)
        | RawEvent::TcpDisconnect(n)
        | RawEvent::UdpSend(n)
        | RawEvent::UdpRecv(n) => {
            let mut v = vec![
                ("PID", Num(n.pid.into())),
                ("size", Num(n.size.into())),
                ("daddr", Addr(n.daddr)),
                ("saddr", Addr(n.saddr)),
                ("dport", Num(n.dport.into())),
                ("sport", Num(n.sport.into())),
                ("seqnum", Num(n.seqnum.into())),
                ("connid", Num(n.connid.into())),
            ];
            if matches!(e, RawEvent::TcpConnect(_) | RawEvent::TcpAccept(_)) {
                for skipped in ["mss", "sackopt", "tsopt", "wsopt", "rcvwin", "rcvwinscale", "sndwinscale"] {
                    v.push((skipped, Skip));
                }
            }
            v
        }
        RawEvent::DnsQuery(q) => vec![
            ("QueryName", s(&q.query_name)),
            ("QueryType", Num(q.query_type.into())),
            ("QueryOptions", Num(q.query_options)),
            ("QueryStatus", Num(q.query_status.into())),
            ("QueryResults", s(&q.query_results)),
        ],
        RawEvent::ClassicProcess(c) => vec![
            ("UniqueProcessKey", Num(c.unique_process_key)),
            ("ProcessId", Num(c.pid.into())),
            ("ParentId", Num(c.parent_pid.into())),
            ("SessionId", Num(c.session_id.into())),
            ("ExitStatus", Signed(c.exit_status.into())),
            ("DirectoryTableBase", Num(c.directory_table_base)),
            ("Flags", Num(c.flags.into())),
            // TDH shows the account name; the live test checks it via LookupAccountSid.
            ("UserSID", Skip),
            ("ImageFileName", a(&c.image_file_name)),
            ("CommandLine", s(&c.command_line)),
            ("PackageFullName", s(&c.package_full_name)),
            ("ApplicationId", s(&c.application_id)),
        ],
    }
}

/// TDH's text for an integer: decimal, or `0x` + hex.
fn tdh_num(t: &str) -> Option<u64> {
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => t.parse().ok(),
    }
}

/// FILETIME → `YYYY-MM-DDTHH:MM:SS.nnnnnnnnnZ`, the form TDH prints (minus its
/// left-to-right marks).
pub fn filetime_iso(ft: u64) -> String {
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    let nanos = (ft % 10_000_000) * 100;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{nanos:09}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Compares a parsed event with TDH's decoding. Every TDH field must be
/// accounted for, so a renamed or extra field is caught too. Returns the
/// mismatches, empty when the event agrees.
pub fn compare(e: &RawEvent, tdh: &Map<String, Value>) -> Vec<String> {
    let ours = expected_fields(e);
    let mut errs = Vec::new();
    for name in tdh.keys() {
        if !ours.iter().any(|(n, _)| n == name) {
            errs.push(format!("{name}: in TDH's decoding but not in ours"));
        }
    }
    for (name, want) in &ours {
        let Some(got) = tdh.get(*name).and_then(Value::as_str) else {
            errs.push(format!("{name}: missing from TDH's decoding"));
            continue;
        };
        let ok = match want {
            Expect::Num(n) => tdh_num(got) == Some(*n),
            Expect::Signed(n) => got.parse::<i64>().ok() == Some(*n),
            // TDH stops a string at its first NUL; ours keeps a counted registry
            // name whole (embedded NULs, sensor spec §7.5).
            Expect::Str(s) => got == s || (s.contains('\0') && s.split('\0').next() == Some(got)),
            Expect::Time(ft) => got.replace('\u{200e}', "") == filetime_iso(*ft),
            Expect::Addr(a) => got.parse::<IpAddr>().ok() == Some(*a),
            Expect::Bytes(b) if b.is_empty() => got.is_empty(),
            Expect::Bytes(b) => got.strip_prefix("0x").map(str::to_ascii_lowercase) == Some(encode_hex(b)),
            Expect::Skip => true,
        };
        if !ok {
            errs.push(format!("{name}: TDH {got:?}, ours {want:?}"));
        }
    }
    errs
}
```

- [ ] **Step 2: Replay**

`crates/atlas-etw/tests/replay.rs`:
```rust
//! Tier 2 (sensor spec §12.2): replays recorded events through the parsers and
//! compares every field with TDH's decoding from the machine that recorded them.
//!
//! - `tests/fixtures/*.jsonl` are committed. They are recorded on a GitHub
//!   Windows runner by `tests/live.rs` and hold only the scenario's own events.
//! - `ATLAS_ETW_EXTRA_FIXTURES` (directories, separated by `;`) adds local
//!   recordings, such as the host's, that are never committed (§4.3).

mod common;

use atlas_etw::layout;
use atlas_etw::parse::{ParseError, parse};
use common::{Fixture, compare, short_name};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|x| x == "jsonl"));
    files.sort();
    files
}

struct Outcome {
    parsed: usize,
    /// (provider, id, version) of every event that parsed and agreed with TDH.
    kinds: BTreeSet<(&'static str, u16, u8)>,
    failures: Vec<String>,
}

/// `strict`: every line of one of our providers must be a readable fixture (the
/// committed recordings). Local recordings from the spike probe hold lines
/// without a raw payload, which are skipped.
fn replay(files: &[PathBuf], strict: bool) -> Outcome {
    let mut out = Outcome { parsed: 0, kinds: BTreeSet::new(), failures: Vec::new() };
    for file in files {
        let bytes = std::fs::read(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let text = String::from_utf8_lossy(&bytes);
        for (n, line) in text.lines().enumerate() {
            let at = format!("{}:{}", file.display(), n + 1);
            let Some(f) = Fixture::from_json(line) else {
                if strict && !line.trim().is_empty() {
                    out.failures.push(format!("{at}: not a readable fixture line"));
                }
                continue;
            };
            match parse(&f.meta, &f.payload) {
                Ok(ev) => {
                    let errs = compare(&ev, &f.tdh);
                    if errs.is_empty() {
                        out.parsed += 1;
                        out.kinds.insert((short_name(f.meta.provider), f.meta.id, f.meta.version));
                    } else {
                        out.failures.push(format!("{at}: {}", errs.join("; ")));
                    }
                }
                // Recordings may hold events outside our set (the spike probe recorded everything).
                Err(ParseError::UnknownEvent) => {}
                Err(e) => out.failures.push(format!("{at}: {:?} {e}", f.meta)),
            }
        }
    }
    out
}

#[test]
fn filetime_formats_like_tdh() {
    // 2026-10-02T18:37:27.610206700Z, from a spike S8 ProcessStart.
    let unix = 1_790_966_247u64;
    let ft = (unix + 11_644_473_600) * 10_000_000 + 6_102_067;
    assert_eq!(common::filetime_iso(ft), "2026-10-02T18:37:27.610206700Z");
    assert_eq!(common::filetime_iso(0), "1601-01-01T00:00:00.000000000Z");
}

#[test]
fn committed_fixtures_agree_with_tdh() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let files = jsonl_files(&dir);
    let out = replay(&files, true);
    assert!(out.failures.is_empty(), "{} mismatches:\n{}", out.failures.len(), out.failures.join("\n"));
    // Until the first recording is committed there is nothing to check; after
    // that, a recording that yields no events is a broken recording.
    if !files.is_empty() {
        assert!(out.parsed > 0, "{} fixture file(s) but no events", files.len());
        // The scenario produces every event we parse, at some version.
        let expected: BTreeSet<_> = layout::LAYOUTS.iter().map(|l| (short_name(l.provider), l.id)).collect();
        let seen: BTreeSet<_> = out.kinds.iter().map(|(p, id, _)| (*p, *id)).collect();
        let missing: Vec<_> = expected.difference(&seen).collect();
        assert!(missing.is_empty(), "the fixtures lack {missing:?}");
    }
}

#[test]
fn extra_local_fixtures_agree_with_tdh() {
    let Ok(dirs) = std::env::var("ATLAS_ETW_EXTRA_FIXTURES") else { return };
    let files: Vec<PathBuf> =
        dirs.split(';').filter(|d| !d.is_empty()).flat_map(|d| jsonl_files(Path::new(d))).collect();
    let out = replay(&files, false);
    eprintln!("replayed {} events from {} files; kinds: {:?}", out.parsed, files.len(), out.kinds);
    assert!(out.failures.is_empty(), "{} mismatches:\n{}", out.failures.len(), out.failures.join("\n"));
}
```

- [ ] **Step 3: Check, optionally against local recordings, and commit**

```powershell
cargo test -p atlas-etw --test replay
```
Expected: 3 pass. `committed_fixtures_agree_with_tdh` checks nothing until Task 7 commits the fixtures.

Optional, on the host: replay the spike and live-run recordings, which stay local.
```powershell
$env:ATLAS_ETW_EXTRA_FIXTURES = 'spikes\results\s1;spikes\results\s6;spikes\results\s7;spikes\results\s8;spikes\results\s5;spikes\results\1b2'
cargo test -p atlas-etw --test replay extra -- --nocapture
Remove-Item Env:ATLAS_ETW_EXTRA_FIXTURES
```
Expected at planning time: every event agrees (6,407 spike events of 23 kinds, plus 1,235 from the live run).
```powershell
git add crates/atlas-etw/tests
git commit -m "test(etw): replay fixtures through the parsers against TDH (tier 2)"
```

### Task 5: Live test (tier 3)

**Files:**
- Create: `crates/atlas-etw/tests/live.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: the CI recording (`ATLAS_ETW_RECORD`) and report (`ATLAS_ETW_REPORT`).

The test starts three sessions: `Atlas-Test-Watch` (Kernel-EventTracing, F2), then `Atlas-Test-Sensor` (Session A's settings) and `Atlas-Test-Process` (Session B's). It then runs itself as the actor and checks:
- every captured event: parse, layout, and each field against TDH;
- the scenario: process; file create, write, overwrite, timestamps, rename, delete, a failed create and a failed delete with their OperationEnds, and delete-on-close;
- registry: create, set, delete, embedded NULs in value and key names, and the CloseKey handle test;
- TCP and UDP over IPv4 and IPv6, with address ends; DNS;
- the session primitives: query, provider state, disable and re-enable, re-apply, filter change, and an external stop, after which the old `Session` must report itself stale;
- that the event-ID filters deliver only requested IDs, that the consumer's QPC frequency is the system's, and that TDH failed to decode only the one expected event (the embedded-NUL SetValueKey).

Process events are kept whatever their PID, and the actor's tree is rebuilt in timestamp order afterwards. Real-time buffers are per CPU, so a child's ImageLoad can arrive before its ProcessStart.

What it changes on the machine, all temporary: those three sessions, a folder `%TEMP%\atlas-etw-live-<pid>`, a key `HKCU\Software\AtlasEtwLive-<pid>`, loopback sockets, and DNS lookups of `example.com`, `atlas-etw-live.invalid` and `localhost`.

- [ ] **Step 1: The live test**

`crates/atlas-etw/tests/live.rs`:
```rust
//! Tier 3 for `atlas-etw` (sensor spec §12.3): real sessions, a scripted
//! scenario, and every event of the scenario's process tree parsed and compared
//! with TDH. Needs administrator rights, so it is `#[ignore]`d; CI runs it
//! explicitly on its (elevated) Windows runner:
//!
//! `cargo test -p atlas-etw --test live -- --ignored --test-threads=1 --nocapture`
//!
//! Two processes: this one observes (sessions, TDH, checks) and re-runs itself
//! as the **actor** (`ATLAS_ETW_ACTOR=1`), which performs the scenario. Only the
//! actor's tree is kept, so the observer's own TDH lookups (registry and file
//! reads) never feed back into what it records.
//!
//! - `ATLAS_ETW_RECORD=<file>` writes the scenario's events as replay fixtures.
//! - `ATLAS_ETW_REPORT=<file>` writes the checks and observations as JSON.
#![cfg(windows)]

mod common;

use atlas_etw::Provider;
use atlas_etw::layout;
use atlas_etw::parse::{ClassicKind, EventMeta, ParseError, RawEvent, WStr};
use atlas_etw::providers::{Enable, session_a};
use atlas_etw::session::{self, Config, EtwError, EventRecord, Kind, STALE, Session, VersionGate, tdh};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::os::windows::fs::OpenOptionsExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::System::Registry::{NtCreateKey, NtDeleteKey, NtDeleteValueKey, NtSetValueKey};
use windows::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, UNICODE_STRING};
use windows::Win32::NetworkManagement::Dns::*;
use windows::Win32::Security::{LookupAccountSidW, PSID, SID_NAME_USE};
use windows::Win32::Storage::FileSystem::DeleteFileW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::core::{PCWSTR, PWSTR};

const SA: &str = "Atlas-Test-Sensor";
const SB: &str = "Atlas-Test-Process";
const SW: &str = "Atlas-Test-Watch";
/// Microsoft-Windows-Kernel-EventTracing: does it report enable changes? (plan 1b-4 input)
const KERNEL_EVENT_TRACING: u128 = 0xb675ec37_bdb6_4648_bc92_f3fdc74d3ca2;
/// `EVENT_ENABLE_PROPERTY_PROCESS_START_KEY`.
const START_KEY_PROPERTY: u32 = 0x80;
const ACTOR_ENV: &str = "ATLAS_ETW_ACTOR";
const ACTOR_LINE: &str = "ATLAS_ETW_ACTOR_RESULT ";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn qpc() -> i64 {
    let mut v = 0;
    // SAFETY: writes one i64.
    unsafe { QueryPerformanceCounter(&mut v) }.expect("QueryPerformanceCounter");
    v
}

/// One kept event, with TDH's view of it taken inside the callback.
struct Captured {
    meta: EventMeta,
    raw_id: u16,
    opcode: u8,
    flags: u16,
    pid: u32,
    tid: u32,
    ts: i64,
    start_key: Option<u64>,
    payload: Vec<u8>,
    event: Result<RawEvent, ParseError>,
    tdh: Result<Vec<(String, String)>, EtwError>,
    /// TDH's layout of this event, when it differs from our table.
    layout_mismatch: Option<String>,
    /// The classic `UserSID`, resolved the way TDH shows it (`\\DOMAIN\name`).
    account: Option<String>,
}

impl Captured {
    fn tdh_map(&self) -> Map<String, Value> {
        self.tdh.as_ref().map(|v| v.iter().map(|(k, s)| (k.clone(), json!(s))).collect()).unwrap_or_default()
    }

    fn fixture_line(&self) -> Value {
        json!({
            "provider": common::short_name(self.meta.provider),
            "id": self.raw_id,
            "version": self.meta.version,
            "opcode": self.opcode,
            "flags": format!("{:#06x}", self.flags),
            "pid": self.pid,
            "tid": self.tid,
            "ts": self.ts,
            "start_key": self.start_key.map(|k| format!("{k:#018x}")),
            "raw": common::encode_hex(&self.payload),
            "fields": self.tdh_map(),
        })
    }
}

fn account_of(sid: &[u8]) -> Option<String> {
    let (mut n, mut d) = (256u32, 256u32);
    let (mut name, mut dom) = (vec![0u16; 256], vec![0u16; 256]);
    let mut use_ = SID_NAME_USE::default();
    // SAFETY: `sid` holds a valid SID (it parsed); the buffers have the sizes passed.
    unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            PSID(sid.as_ptr() as *mut _),
            Some(PWSTR(name.as_mut_ptr())),
            &mut n,
            Some(PWSTR(dom.as_mut_ptr())),
            &mut d,
            &mut use_,
        )
    }
    .ok()?;
    Some(format!(
        r"\\{}\{}",
        String::from_utf16_lossy(&dom[..d as usize]),
        String::from_utf16_lossy(&name[..n as usize])
    ))
}

type Tree = Arc<Mutex<HashSet<u32>>>;
type Sink = Arc<Mutex<Vec<Captured>>>;
/// Events of our manifest providers with an ID we did not ask for: the event-ID
/// filter should make this empty.
type Unrequested = Arc<Mutex<BTreeMap<(Provider, u16), u64>>>;
/// A Kernel-EventTracing event: QPC time, ID, header PID, start key, TDH fields.
type Watched = (i64, u16, u32, Option<u64>, Vec<(String, String)>);

/// Keeps the events of the actor's tree. Every process event is kept whatever
/// its PID: real-time buffers are per CPU, so a child's ImageLoad can arrive
/// before its ProcessStart (§3.2); [`final_tree`] rebuilds the tree in time
/// order afterwards. Network events are matched by their payload PID only:
/// their header PID is not the owner (§5.3).
fn tree_callback(tree: Tree, sink: Sink, unrequested: Unrequested) -> impl FnMut(&EventRecord) + Send + 'static {
    let mut gate = VersionGate::new();
    move |rec: &EventRecord| {
        let Some(meta) = rec.meta() else { return };
        let event = gate.parse(rec);
        if matches!(event, Err(ParseError::UnknownEvent)) && meta.provider != Provider::ClassicProcess {
            *unrequested.lock().unwrap().entry((meta.provider, meta.id)).or_default() += 1;
        }
        let keep = {
            let mut t = tree.lock().unwrap();
            let header = t.contains(&rec.pid());
            match &event {
                Ok(RawEvent::ProcessStart(s)) => {
                    if header || t.contains(&s.parent_pid) {
                        t.insert(s.pid);
                    }
                    true
                }
                Ok(RawEvent::ClassicProcess(c)) => {
                    if t.contains(&c.parent_pid) {
                        t.insert(c.pid);
                    }
                    true
                }
                Ok(RawEvent::ImageLoad(_) | RawEvent::ProcessStop(_)) => true,
                Ok(
                    RawEvent::TcpConnect(n)
                    | RawEvent::TcpAccept(n)
                    | RawEvent::TcpDisconnect(n)
                    | RawEvent::UdpSend(n)
                    | RawEvent::UdpRecv(n),
                ) => t.contains(&n.pid),
                // Events of ours we do not parse (classic opcode 11, for one).
                Err(ParseError::UnknownEvent) => false,
                _ => header,
            }
        };
        if !keep {
            return;
        }
        let layout_mismatch = layout::find(meta.provider, meta.id, meta.version).and_then(|l| {
            let ours: Vec<(String, u16)> = l.fields.iter().map(|(n, t)| (n.to_string(), *t as u16)).collect();
            match tdh::event_layout(rec) {
                Ok(theirs) if theirs == ours => None,
                Ok(theirs) => Some(format!("TDH {theirs:?}")),
                Err(e) => Some(e.to_string()),
            }
        });
        let account = match &event {
            Ok(RawEvent::ClassicProcess(c)) => c.user_sid.as_ref().and_then(|s| account_of(s.as_bytes())),
            _ => None,
        };
        sink.lock().unwrap().push(Captured {
            meta,
            raw_id: rec.id(),
            opcode: rec.opcode(),
            flags: rec.flags(),
            pid: rec.pid(),
            tid: rec.tid(),
            ts: rec.timestamp(),
            start_key: rec.start_key(),
            payload: rec.payload().to_vec(),
            event,
            tdh: tdh::decode(rec),
            layout_mismatch,
            account,
        });
    }
}

/// The actor's process tree, rebuilt in timestamp order, applied to what the
/// callback kept (see [`tree_callback`]). The observer's own classic events
/// stay too: its rundown (DCStart, DCEnd) is checked.
fn final_tree(mut events: Vec<Captured>, actor: u32, observer: u32) -> Vec<Captured> {
    events.sort_by_key(|c| c.ts);
    let mut tree = HashSet::from([actor]);
    for c in &events {
        match ok(c) {
            Some(RawEvent::ProcessStart(s)) if tree.contains(&s.parent_pid) => {
                tree.insert(s.pid);
            }
            Some(RawEvent::ClassicProcess(p)) if p.kind == ClassicKind::Start && tree.contains(&p.parent_pid) => {
                tree.insert(p.pid);
            }
            _ => {}
        }
    }
    events.retain(|c| match ok(c) {
        Some(RawEvent::ProcessStart(s)) => tree.contains(&s.pid),
        Some(RawEvent::ProcessStop(s)) => tree.contains(&s.pid),
        Some(RawEvent::ImageLoad(i)) => tree.contains(&i.pid),
        Some(RawEvent::ClassicProcess(p)) => tree.contains(&p.pid) || p.pid == observer,
        Some(
            RawEvent::TcpConnect(n)
            | RawEvent::TcpAccept(n)
            | RawEvent::TcpDisconnect(n)
            | RawEvent::UdpSend(n)
            | RawEvent::UdpRecv(n),
        ) => tree.contains(&n.pid),
        _ => tree.contains(&c.pid),
    });
    events
}

/// Everything Kernel-EventTracing logs, TDH-decoded (exploration for plan 1b-4).
fn watch_callback(sink: Arc<Mutex<Vec<Watched>>>) -> impl FnMut(&EventRecord) + Send {
    move |rec: &EventRecord| {
        if rec.provider_guid() == KERNEL_EVENT_TRACING {
            let fields = tdh::decode(rec).unwrap_or_else(|e| vec![("decode_error".into(), e.to_string())]);
            sink.lock().unwrap().push((rec.timestamp(), rec.id(), rec.pid(), rec.start_key(), fields));
        }
    }
}

/// Named QPC windows around each scenario step.
#[derive(Default)]
struct Steps(Vec<(String, i64, i64)>);

impl Steps {
    fn run<T>(&mut self, name: &str, f: impl FnOnce() -> T) -> T {
        let a = qpc();
        let r = f();
        self.0.push((name.into(), a, qpc()));
        r
    }

    fn window(&self, name: &str) -> (i64, i64) {
        self.0.iter().find(|(n, ..)| n == name).map(|(_, a, b)| (*a, *b)).unwrap_or_else(|| panic!("no step {name}"))
    }

    fn to_json(&self) -> Value {
        json!(self.0.iter().map(|(n, a, b)| json!([n, a, b])).collect::<Vec<_>>())
    }

    fn extend_from_json(&mut self, v: &Value) {
        for s in v.as_array().into_iter().flatten() {
            self.0.push((s[0].as_str().unwrap().into(), s[1].as_i64().unwrap(), s[2].as_i64().unwrap()));
        }
    }
}

/// The results: hard checks (the test fails on any) and observations.
#[derive(Default)]
struct Report {
    checks: Vec<(String, bool, String)>,
    notes: Map<String, Value>,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        self.checks.push((name.into(), ok, detail.into()));
    }

    fn note(&mut self, name: &str, v: Value) {
        self.notes.insert(name.into(), v);
    }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened by us and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn create_key(path: &str) -> Key {
    let w = wide(path);
    let mut k = HKEY::default();
    // SAFETY: valid strings and out-pointers.
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(w.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut k,
            None,
        )
    };
    assert!(r.is_ok(), "RegCreateKeyExW: {r:?}");
    Key(k)
}

fn open_key(path: &str) -> Key {
    let w = wide(path);
    let mut k = HKEY::default();
    // SAFETY: valid strings and out-pointer.
    let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(w.as_ptr()), None, KEY_ALL_ACCESS, &mut k) };
    assert!(r.is_ok(), "RegOpenKeyExW: {r:?}");
    Key(k)
}

/// A counted `UNICODE_STRING` over `name` (which may contain NULs).
fn counted(name: &[u16]) -> UNICODE_STRING {
    let len = (name.len() * 2) as u16;
    UNICODE_STRING { Length: len, MaximumLength: len, Buffer: PWSTR(name.as_ptr() as *mut _) }
}

fn dns_query(name: &str) -> u32 {
    let w = wide(name);
    let mut rec: *mut DNS_RECORDA = std::ptr::null_mut();
    // SAFETY: valid name and out-pointer; the result list is freed below.
    let st = unsafe { DnsQuery_W(PCWSTR(w.as_ptr()), DNS_TYPE_A, DNS_QUERY_STANDARD, None, &mut rec, None) };
    if !rec.is_null() {
        // SAFETY: frees the list DnsQuery_W allocated.
        unsafe { DnsFree(Some(rec as *const _), DnsFreeRecordList) };
    }
    st.0
}

/// `KUSER_SHARED_DATA.BootId` (spec §6.1; offset from public symbols, 26200/26300).
fn boot_id() -> u64 {
    // SAFETY: KUSER_SHARED_DATA is mapped read-only at this address in every process.
    u64::from(unsafe { std::ptr::read_volatile(0x7FFE_02C4 as *const u32) })
}

fn ends_with(w: &WStr, suffix: &str) -> bool {
    w.to_string_lossy().to_ascii_lowercase().ends_with(&suffix.to_ascii_lowercase())
}

fn in_window(c: &Captured, (a, b): (i64, i64)) -> bool {
    c.ts >= a && c.ts <= b
}

fn ok(c: &Captured) -> Option<&RawEvent> {
    c.event.as_ref().ok()
}

// ---------------------------------------------------------------- actor

/// The scenario, run in the actor process. Returns what the observer needs to
/// find the events again: step windows, the child's PID, ports, DNS statuses.
fn actor() -> Value {
    let me = std::process::id();
    let mut steps = Steps::default();
    // Let the observer add us to its tree before anything happens.
    std::thread::sleep(Duration::from_millis(500));

    // ---- process ----
    let cmd = format!(r"{}\System32\cmd.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()));
    let child_pid = steps.run("spawn_child", || {
        let mut c = std::process::Command::new(&cmd).args(["/c", "exit 7"]).spawn().expect("spawn cmd");
        let st = c.wait().expect("wait");
        assert_eq!(st.code(), Some(7));
        c.id()
    });

    // ---- files ----
    let dir = std::env::temp_dir().join(format!("atlas-etw-live-{me}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b, c, d) = (dir.join("a.txt"), dir.join("b.txt"), dir.join("c.txt"), dir.join("d.txt"));
    steps.run("file_create_new_write", || {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&a).unwrap();
        f.write_all(b"hello").unwrap();
    });
    steps.run("file_overwrite", || std::fs::write(&a, b"x").unwrap());
    steps.run("file_set_times", || {
        let f = std::fs::OpenOptions::new().write(true).open(&a).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000)).unwrap();
    });
    steps.run("file_rename", || std::fs::rename(&a, &b).unwrap());
    steps.run("file_delete", || std::fs::remove_file(&b).unwrap());
    std::fs::write(&c, b"c").unwrap();
    steps.run("file_create_existing_fails", || {
        assert!(std::fs::OpenOptions::new().write(true).create_new(true).open(&c).is_err());
    });
    let mut perms = std::fs::metadata(&c).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&c, perms.clone()).unwrap();
    steps.run("file_delete_readonly_fails", || {
        let w = wide(&c.to_string_lossy());
        // SAFETY: a valid path.
        assert!(unsafe { DeleteFileW(PCWSTR(w.as_ptr())) }.is_err());
    });
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&c, perms).unwrap();
    std::fs::remove_file(&c).unwrap();
    steps.run("file_delete_on_close", || {
        // FILE_FLAG_DELETE_ON_CLOSE
        let f = std::fs::OpenOptions::new().write(true).create_new(true).custom_flags(0x0400_0000).open(&d).unwrap();
        drop(f);
        assert!(!d.exists());
    });
    let _ = std::fs::remove_dir_all(&dir);

    // ---- registry ----
    let key_path = format!(r"Software\AtlasEtwLive-{me}");
    let k = steps.run("reg_create", || create_key(&key_path));
    steps.run("reg_set_value", || {
        // SAFETY: valid key, name and data.
        let r = unsafe { RegSetValueExW(k.0, PCWSTR(wide("v").as_ptr()), None, REG_DWORD, Some(&7u32.to_le_bytes())) };
        assert!(r.is_ok());
    });
    // Names with embedded NULs: values (a known way to hide Run entries) and keys.
    let nul_value = units("a\0b");
    steps.run("reg_set_value_embedded_nul", || {
        let us = counted(&nul_value);
        let data: Vec<u8> = "x\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        // SAFETY: valid key handle, counted name and data buffer.
        let st =
            unsafe { NtSetValueKey(HANDLE(k.0.0), &us, None, REG_SZ.0, Some(data.as_ptr().cast()), data.len() as u32) };
        assert!(st.is_ok(), "NtSetValueKey: {st:?}");
    });
    steps.run("reg_delete_value_embedded_nul", || {
        let us = counted(&nul_value);
        // SAFETY: valid key handle and counted name.
        let st = unsafe { NtDeleteValueKey(HANDLE(k.0.0), &us) };
        assert!(st.is_ok(), "NtDeleteValueKey: {st:?}");
    });
    steps.run("reg_create_key_embedded_nul", || {
        let name = units("k\0x");
        let us = counted(&name);
        let oa = OBJECT_ATTRIBUTES {
            Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: HANDLE(k.0.0),
            ObjectName: &us,
            ..Default::default()
        };
        let mut h = HANDLE::default();
        // SAFETY: valid attributes, counted name and out-handle; KEY_ALL_ACCESS.
        let st = unsafe { NtCreateKey(&mut h, KEY_ALL_ACCESS.0, &oa, None, None, 0, None) };
        assert!(st.is_ok(), "NtCreateKey: {st:?}");
        // SAFETY: deletes and closes the key we just created.
        unsafe {
            let _ = NtDeleteKey(h);
            let _ = CloseHandle(h);
        }
    });
    steps.run("reg_delete_value", || {
        // SAFETY: valid key and name.
        assert!(unsafe { RegDeleteValueW(k.0, PCWSTR(wide("v").as_ptr())) }.is_ok());
    });
    let opened = steps.run("reg_open", || open_key(&key_path));
    let dup = steps.run("reg_duplicate", || {
        let mut h = HANDLE::default();
        // SAFETY: duplicates our own key handle within our process.
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                HANDLE(opened.0.0),
                GetCurrentProcess(),
                &mut h,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
        }
        .expect("DuplicateHandle");
        h
    });
    steps.run("reg_close_duplicate", || {
        // SAFETY: closes the duplicate once.
        unsafe { CloseHandle(dup) }.expect("CloseHandle");
    });
    steps.run("reg_close_original", || drop(opened));
    steps.run("reg_delete_key", || {
        // SAFETY: deletes our test key and its values.
        assert!(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide(&key_path).as_ptr())) }.is_ok());
    });
    steps.run("reg_close_created", || drop(k));

    // ---- network ----
    let tcp = |addr: &str| {
        let l = TcpListener::bind(addr).unwrap();
        let server_port = l.local_addr().unwrap().port();
        let mut c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let client_port = c.local_addr().unwrap().port();
        let (mut s, _) = l.accept().unwrap();
        c.write_all(&[7; 1000]).unwrap();
        let mut buf = [0u8; 1000];
        s.read_exact(&mut buf).unwrap();
        drop(c);
        drop(s);
        [client_port, server_port]
    };
    let tcp4 = steps.run("tcp_v4", || tcp("127.0.0.1:0"));
    let tcp6 = steps.run("tcp_v6", || tcp("[::1]:0"));
    let udp = |addr: &str| {
        let x = UdpSocket::bind(addr).unwrap();
        let y = UdpSocket::bind(addr).unwrap();
        x.send_to(&[1; 100], y.local_addr().unwrap()).unwrap();
        let mut buf = [0u8; 100];
        y.recv_from(&mut buf).unwrap();
        [x.local_addr().unwrap().port(), y.local_addr().unwrap().port()]
    };
    let udp4 = steps.run("udp_v4", || udp("127.0.0.1:0"));
    let udp6 = steps.run("udp_v6", || udp("[::1]:0"));

    // ---- dns ----
    let dns_ok = steps.run("dns_example", || dns_query("example.com"));
    let dns_nx = steps.run("dns_nxdomain", || dns_query("atlas-etw-live.invalid"));
    steps.run("dns_localhost", || dns_query("localhost"));

    json!({
        "pid": me,
        "child_pid": child_pid,
        "steps": steps.to_json(),
        "tcp4": tcp4, "tcp6": tcp6, "udp4": udp4, "udp6": udp6,
        "dns_ok": dns_ok, "dns_nx": dns_nx,
    })
}

/// Runs the actor (this test binary again) and returns its result.
fn run_actor(tree: &Tree) -> Value {
    let exe = std::env::current_exe().unwrap();
    let child = std::process::Command::new(exe)
        .args(["--ignored", "--exact", "live_scenario", "--nocapture", "--test-threads=1"])
        .env(ACTOR_ENV, "1")
        .env_remove("ATLAS_ETW_RECORD")
        .env_remove("ATLAS_ETW_REPORT")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn actor");
    tree.lock().unwrap().insert(child.id());
    let out = child.wait_with_output().expect("actor");
    let text = String::from_utf8_lossy(&out.stdout);
    // libtest prints "test live_scenario ... " without a newline, so the marker can be mid-line.
    let line = text
        .lines()
        .find_map(|l| l.find(ACTOR_LINE).map(|i| &l[i + ACTOR_LINE.len()..]))
        .unwrap_or_else(|| panic!("actor failed:\n{text}"));
    serde_json::from_str(line).expect("actor result")
}

// ---------------------------------------------------------------- observer

#[test]
#[ignore = "needs administrator rights; CI runs it on its Windows runner"]
fn live_scenario() {
    if std::env::var_os(ACTOR_ENV).is_some() {
        println!(
            "
{ACTOR_LINE}{}",
            actor()
        );
        return;
    }
    let me = std::process::id();
    let tree: Tree = Arc::new(Mutex::new(HashSet::new()));
    let sink: Sink = Arc::new(Mutex::new(Vec::new()));
    let unrequested: Unrequested = Arc::new(Mutex::new(BTreeMap::new()));
    let watch = Arc::new(Mutex::new(Vec::new()));
    let mut report = Report::default();
    let mut steps = Steps::default();

    // The watch session first, so it sees our sessions' enables.
    let sw =
        Session::start(&Config { kind: Kind::Manifest, ..Config::session_b(SW) }).expect("start watch (elevated?)");
    let cw = session::consume(SW, watch_callback(watch.clone())).expect("consume watch");
    sw.enable_guid(KERNEL_EVENT_TRACING, u64::MAX).expect("enable Kernel-EventTracing (elevated?)");

    let sa = Session::start(&Config::session_a(SA)).expect("start A");
    let sb = Session::start(&Config::session_b(SB)).expect("start B");
    let ca = session::consume(SA, tree_callback(tree.clone(), sink.clone(), unrequested.clone())).expect("consume A");
    let cb = session::consume(SB, tree_callback(tree.clone(), sink.clone(), unrequested.clone())).expect("consume B");
    let mut freq = 0;
    // SAFETY: writes one i64.
    unsafe { QueryPerformanceFrequency(&mut freq) }.expect("QueryPerformanceFrequency");
    report.check(
        "consumer_qpc_frequency_is_the_systems",
        ca.qpc_frequency() == freq && cb.qpc_frequency() == freq,
        format!("A {} B {} system {freq}", ca.qpc_frequency(), cb.qpc_frequency()),
    );
    let enables = session_a(true);
    steps.run("enable_a", || {
        for e in &enables {
            sa.enable(e).unwrap_or_else(|err| panic!("enable {:?}: {err}", e.provider));
        }
    });
    std::thread::sleep(Duration::from_millis(1500));

    let actor = steps.run("actor", || run_actor(&tree));
    steps.extend_from_json(&actor["steps"]);

    // ---- session control (§9.1 primitives) ----
    let q = session::query_by_name(SA).expect("query A");
    report.check(
        "query_by_name_logger_id",
        q.is_some_and(|i| i.logger_id == sa.logger_id()),
        format!("{q:?} vs {}", sa.logger_id()),
    );
    let file_state = session::provider_state(Provider::KernelFile).expect("provider_state");
    let ours: Vec<_> = file_state.iter().filter(|p| p.logger_id == sa.logger_id()).collect();
    report.check(
        "provider_state_shows_our_enable",
        ours.iter()
            .any(|p| p.match_any_keyword == 0x1EE0 && p.level == 5 && p.enable_property & START_KEY_PROPERTY != 0),
        format!("{ours:?}"),
    );
    let dns_state = session::provider_state(Provider::DnsClient).expect("provider_state dns");
    report.note(
        "dns_client_instances",
        json!({
            "instances": dns_state.iter().map(|p| p.pid).collect::<HashSet<_>>().len(),
            "with_our_session": dns_state.iter().filter(|p| p.logger_id == sa.logger_id()).count(),
        }),
    );
    let net = enables.iter().find(|e| e.provider == Provider::KernelNetwork).unwrap().clone();
    steps.run("ctl_disable_network", || sa.disable(Provider::KernelNetwork).expect("disable"));
    let after_disable = session::provider_state(Provider::KernelNetwork).expect("state");
    report.check(
        "disable_removes_our_enable",
        !after_disable.iter().any(|p| p.logger_id == sa.logger_id()),
        format!("{after_disable:?}"),
    );
    steps.run("ctl_reenable_network", || sa.enable(&net).expect("re-enable"));
    let after_enable = session::provider_state(Provider::KernelNetwork).expect("state");
    report.check(
        "reenable_restores_it",
        after_enable.iter().any(|p| p.logger_id == sa.logger_id()),
        format!("{after_enable:?}"),
    );
    let file = enables.iter().find(|e| e.provider == Provider::KernelFile).unwrap().clone();
    steps.run("ctl_reapply_same_file_enable", || sa.enable(&file).expect("re-apply"));
    let narrowed = Enable { event_ids: file.event_ids.iter().copied().filter(|&i| i != 16).collect(), ..file.clone() };
    steps.run("ctl_narrow_file_filter", || sa.enable(&narrowed).expect("narrow"));
    steps.run("ctl_restore_file_filter", || sa.enable(&file).expect("restore"));

    std::thread::sleep(Duration::from_millis(1500));
    let _ = sa.flush();

    // ---- a session stopped from outside ends its consumer (§9.1) ----
    let stopped = steps.run("ctl_stop_b_by_name", || session::stop_by_name(SB).expect("stop B"));
    let t0 = Instant::now();
    while !cb.is_finished() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    report.check("stop_by_name_ends_consumer", stopped.is_some() && cb.is_finished(), format!("{stopped:?}"));
    // Our handle to B is stale now: it must not act on whatever holds its LoggerId.
    let stale = (sb.is_current(), sb.query().map_err(|e| e.code));
    report.check(
        "an_externally_stopped_session_is_stale",
        stale == (false, Err(STALE)) && sb.stop().is_err_and(|e| e.code == STALE),
        format!("{stale:?}"),
    );
    report.note("process_trace_status_after_external_stop", json!(cb.close()));

    let info_a = sa.stop().expect("stop A");
    report.check("no_events_lost", info_a.events_lost == 0 && info_a.realtime_buffers_lost == 0, format!("{info_a:?}"));
    let panics = ca.panics();
    report.note("process_trace_status_after_stop", json!(ca.close()));
    let _ = sw.stop();
    let _ = cw.close();
    report.check("no_callback_panics", panics == 0, format!("{panics} panics"));

    let unrequested = unrequested.lock().unwrap().clone();
    report.check(
        "event_id_filters_deliver_only_requested_ids",
        unrequested.is_empty(),
        format!("{:?}", unrequested.iter().map(|((p, id), n)| format!("{p:?} {id}: {n}")).collect::<Vec<_>>()),
    );
    let actor_pid = actor["pid"].as_u64().unwrap() as u32;
    let events = final_tree(std::mem::take(&mut *sink.lock().unwrap()), actor_pid, me);
    analyse(&mut report, &steps, &events, &actor, me);
    explore_watch(&mut report, &steps, &watch.lock().unwrap(), me);
    finish(report, &events);
}

fn port(v: &Value, i: usize) -> u16 {
    v[i].as_u64().unwrap() as u16
}

fn analyse(r: &mut Report, steps: &Steps, events: &[Captured], actor: &Value, observer: u32) {
    let actor_pid = actor["pid"].as_u64().unwrap() as u32;
    let child = actor["child_pid"].as_u64().unwrap() as u32;
    // Every kept event parsed, matches TDH field by field, and has our layout.
    // TDH cannot decode only one event of the scenario: the SetValueKey whose
    // value name embeds a NUL (F4), checked separately below.
    let nul_window = steps.window("reg_set_value_embedded_nul");
    let mut bad = Vec::new();
    for c in events {
        match &c.event {
            Ok(e) => {
                let excused = c.tdh.is_err() && matches!(e, RawEvent::RegSetValue(_)) && in_window(c, nul_window);
                if c.tdh.is_err() && !excused {
                    bad.push(format!("{:?}: TDH could not decode it: {:?}", c.meta, c.tdh.as_ref().err()));
                }
                let errs = common::compare(e, &c.tdh_map());
                if !errs.is_empty() && c.tdh.is_ok() {
                    bad.push(format!("{:?}: {}", c.meta, errs.join("; ")));
                }
                if let RawEvent::ClassicProcess(p) = e {
                    let tdh_sid = c.tdh_map().get("UserSID").and_then(Value::as_str).map(str::to_string);
                    if p.user_sid.is_some() && c.account != tdh_sid {
                        bad.push(format!("{:?}: UserSID TDH {tdh_sid:?}, ours {:?}", c.meta, c.account));
                    }
                }
            }
            Err(e) => bad.push(format!("{:?}: parse error {e}", c.meta)),
        }
        if let Some(m) = &c.layout_mismatch {
            bad.push(format!("{:?}: layout differs: {m}", c.meta));
        }
    }
    bad.sort();
    bad.dedup();
    r.check("every_event_agrees_with_tdh", bad.is_empty(), bad.join("\n"));
    let kinds: HashSet<_> = events.iter().map(|c| (c.meta.provider, c.meta.id)).collect();
    let missing: BTreeSet<_> =
        layout::LAYOUTS.iter().map(|l| (l.provider, l.id)).filter(|k| !kinds.contains(k)).collect();
    r.check("every_parsed_kind_was_seen", missing.is_empty(), format!("missing {missing:?}"));
    r.note("events_kept", json!(events.len()));
    r.note("tdh_decode_failures", json!(events.iter().filter(|c| c.tdh.is_err()).count()));

    let evs = || events.iter().filter_map(|c| ok(c).map(|e| (c, e)));

    // Process: Launch, image load, stop, both classic halves, and the start key formula.
    let start = evs().find_map(|(_, e)| match e {
        RawEvent::ProcessStart(s) if s.pid == child => Some(s.clone()),
        _ => None,
    });
    let child_key =
        evs().find_map(|(c, e)| matches!(e, RawEvent::ImageLoad(i) if i.pid == child).then_some(c.start_key).flatten());
    r.check(
        "start_key_is_bootid_shl_48_or_sequence",
        matches!((&start, child_key), (Some(s), Some(k)) if k == (boot_id() << 48) | s.sequence_number),
        format!("seq {:?}, child key {child_key:x?}, boot id {}", start.as_ref().map(|s| s.sequence_number), boot_id()),
    );
    r.check(
        "process_start_parent_is_the_actor",
        start.as_ref().is_some_and(|s| s.parent_pid == actor_pid && ends_with(&s.image_name, r"\cmd.exe")),
        format!("{start:?}"),
    );
    r.check(
        "image_load_for_child",
        evs().any(
            |(_, e)| matches!(e, RawEvent::ImageLoad(i) if i.pid == child && ends_with(&i.image_name, r"\cmd.exe")),
        ),
        "",
    );
    r.check(
        "process_stop_exit_code",
        evs().any(|(_, e)| matches!(e, RawEvent::ProcessStop(s) if s.pid == child && s.exit_code == 7)),
        "",
    );
    let classic_start = evs().find_map(|(_, e)| match e {
        RawEvent::ClassicProcess(c) if c.pid == child && c.kind == ClassicKind::Start => Some(c.clone()),
        _ => None,
    });
    r.check(
        "classic_start_has_command_line_and_user",
        classic_start
            .as_ref()
            .is_some_and(|c| c.command_line.to_string_lossy().contains("exit 7") && c.user_sid.is_some()),
        format!("{classic_start:?}"),
    );
    r.check(
        "classic_end_for_child",
        evs().any(|(_, e)| matches!(e, RawEvent::ClassicProcess(c) if c.pid == child && c.kind == ClassicKind::End)),
        "",
    );
    let rundown: Vec<_> = evs()
        .filter_map(|(_, e)| match e {
            RawEvent::ClassicProcess(c) if c.pid == observer => Some(format!("{:?}", c.kind)),
            _ => None,
        })
        .collect();
    r.check("rundown_has_the_observers_dcstart", rundown.iter().any(|k| k == "DcStart"), format!("{rundown:?}"));
    r.note("observer_classic_events", json!(rundown));
    if let Some(s) = &start {
        // The join (§5.2): both halves of the child's launch, microseconds apart.
        let t_kp =
            events.iter().find(|c| matches!(ok(c), Some(RawEvent::ProcessStart(x)) if x.pid == s.pid)).map(|c| c.ts);
        let t_cl = events
            .iter()
            .find(|c| matches!(ok(c), Some(RawEvent::ClassicProcess(x)) if x.pid == s.pid && x.kind == ClassicKind::Start))
            .map(|c| c.ts);
        r.note("launch_join_qpc_delta", json!(t_kp.zip(t_cl).map(|(a, b)| (a - b).abs())));
    }

    // Files.
    let fev = |step: &str| -> Vec<&RawEvent> {
        let w = steps.window(step);
        events.iter().filter(|c| in_window(c, w)).filter_map(ok).collect()
    };
    let created = fev("file_create_new_write");
    let new_fo = created.iter().find_map(|e| match e {
        RawEvent::FileCreateNew(c) if ends_with(&c.file_name, r"\a.txt") => Some(c.file_object),
        _ => None,
    });
    r.check("file_create_new_30", new_fo.is_some(), "");
    r.check(
        "file_write_and_cleanup_on_that_handle",
        new_fo.is_some_and(|fo| {
            created.iter().any(|e| matches!(e, RawEvent::FileWrite(w) if w.file_object == fo))
                && created.iter().any(|e| matches!(e, RawEvent::FileCleanup(h) if h.file_object == fo))
        }),
        "",
    );
    let ow = fev("file_overwrite");
    r.check(
        "file_overwrite_is_create_plus_eof_truncation",
        ow.iter().any(|e| matches!(e, RawEvent::FileCreate(c) if ends_with(&c.file_name, r"\a.txt")))
            && ow.iter().any(|e| matches!(e, RawEvent::FileSetInfo(i) if i.info_class == 19))
            && !ow.iter().any(|e| matches!(e, RawEvent::FileCreateNew(_))),
        format!("{ow:?}"),
    );
    r.check(
        "file_set_times_is_info_class_4",
        fev("file_set_times").iter().any(|e| matches!(e, RawEvent::FileSetInfo(i) if i.info_class == 4)),
        "",
    );
    r.check(
        "file_rename_carries_new_name",
        fev("file_rename")
            .iter()
            .any(|e| matches!(e, RawEvent::FileRenamePath(p) if ends_with(&p.file_path, r"\b.txt"))),
        "",
    );
    r.check(
        "file_delete_path",
        fev("file_delete")
            .iter()
            .any(|e| matches!(e, RawEvent::FileDeletePath(p) if ends_with(&p.file_path, r"\b.txt"))),
        "",
    );
    let failed_irp = |step: &str, status: u32, pick: &dyn Fn(&RawEvent) -> Option<u64>| {
        let ev = fev(step);
        let irps: Vec<u64> = ev.iter().filter_map(|e| pick(e)).collect();
        ev.iter().any(|e| matches!(e, RawEvent::FileOpEnd(o) if o.status == status && irps.contains(&o.irp)))
    };
    r.check(
        "failed_create_new_has_failed_opend",
        failed_irp("file_create_existing_fails", 0xC000_0035, &|e| match e {
            RawEvent::FileCreate(c) if ends_with(&c.file_name, r"\c.txt") => Some(c.irp),
            _ => None,
        }),
        "",
    );
    r.check(
        "failed_delete_has_failed_opend",
        failed_irp("file_delete_readonly_fails", 0xC000_0121, &|e| match e {
            RawEvent::FileDeletePath(p) => Some(p.irp),
            _ => None,
        }),
        "",
    );
    r.check(
        "delete_on_close_flag_on_create",
        fev("file_delete_on_close")
            .iter()
            .any(|e| matches!(e, RawEvent::FileCreate(c) if ends_with(&c.file_name, r"\d.txt") && c.delete_on_close())),
        "",
    );

    // Registry.
    let rev = fev;
    let created_key = rev("reg_create").iter().find_map(|e| match e {
        RawEvent::RegCreateKey(o) if o.status == 0 && o.disposition == 1 => Some(o.clone()),
        _ => None,
    });
    r.check(
        "reg_create_key_created_new",
        created_key.as_ref().is_some_and(|o| ends_with(&o.relative_name, &format!("AtlasEtwLive-{actor_pid}"))),
        format!("{created_key:?}"),
    );
    let ko = created_key.as_ref().map(|o| o.key_object);
    r.check(
        "reg_set_value_on_created_key",
        rev("reg_set_value").iter().any(|e| {
            matches!(e, RawEvent::RegSetValue(v) if Some(v.key_object) == ko && v.value_name.to_string_lossy() == "v" && v.value_type == 4 && v.data_size == 4)
        }),
        "",
    );
    let nul_value = units("a\0b");
    r.check(
        "embedded_nul_value_name_on_set",
        rev("reg_set_value_embedded_nul")
            .iter()
            .any(|e| matches!(e, RawEvent::RegSetValue(v) if v.value_name.as_units() == nul_value && v.value_type == 1 && v.data_size == 4)),
        format!("{:?}", rev("reg_set_value_embedded_nul")),
    );
    r.check(
        "embedded_nul_value_name_on_delete",
        rev("reg_delete_value_embedded_nul")
            .iter()
            .any(|e| matches!(e, RawEvent::RegDeleteValue(v) if v.value_name.as_units() == nul_value && v.status == 0)),
        format!("{:?}", rev("reg_delete_value_embedded_nul")),
    );
    r.check(
        "embedded_nul_key_name_on_create",
        rev("reg_create_key_embedded_nul").iter().any(
            |e| matches!(e, RawEvent::RegCreateKey(o) if o.relative_name.as_units() == units("k\0x") && o.status == 0),
        ),
        format!("{:?}", rev("reg_create_key_embedded_nul")),
    );
    r.check(
        "reg_delete_value",
        rev("reg_delete_value").iter().any(
            |e| matches!(e, RawEvent::RegDeleteValue(v) if v.value_name.to_string_lossy() == "v" && v.status == 0),
        ),
        "",
    );
    let opened = rev("reg_open").iter().find_map(|e| match e {
        RawEvent::RegOpenKey(o) if o.status == 0 => Some(o.key_object),
        _ => None,
    });
    let closes = |step: &str| {
        rev(step).iter().filter(|e| matches!(e, RawEvent::RegCloseKey(k) if Some(k.key_object) == opened)).count()
    };
    r.note(
        "closekey_per_handle",
        json!({
            "closes_when_duplicate_closed": closes("reg_close_duplicate"),
            "closes_when_original_closed": closes("reg_close_original"),
        }),
    );
    r.check(
        "closekey_only_on_the_last_handle",
        closes("reg_close_duplicate") == 0 && closes("reg_close_original") == 1,
        "",
    );
    r.check(
        "reg_delete_key",
        rev("reg_delete_key").iter().any(|e| matches!(e, RawEvent::RegDeleteKey(k) if k.status == 0)),
        "",
    );
    r.check(
        "closekey_for_created_key",
        rev("reg_close_created").iter().any(|e| matches!(e, RawEvent::RegCloseKey(k) if Some(k.key_object) == ko)),
        "",
    );

    // Network: which end is saddr (§7.3)? Local for connect, accept and UDP send;
    // remote (the sender) for UDP receive.
    let (tcp4, tcp6, udp4, udp6) = (&actor["tcp4"], &actor["tcp6"], &actor["udp4"], &actor["udp6"]);
    let net = |step: &str| -> Vec<String> {
        rev(step)
            .iter()
            .filter_map(|e| match e {
                RawEvent::TcpConnect(n) => Some(format!("connect s{}:{} d{}:{}", n.saddr, n.sport, n.daddr, n.dport)),
                RawEvent::TcpAccept(n) => Some(format!("accept s{}:{} d{}:{}", n.saddr, n.sport, n.daddr, n.dport)),
                RawEvent::TcpDisconnect(n) => {
                    Some(format!("disconnect s{}:{} d{}:{}", n.saddr, n.sport, n.daddr, n.dport))
                }
                RawEvent::UdpSend(n) => Some(format!("udp_send s{}:{} d{}:{}", n.saddr, n.sport, n.daddr, n.dport)),
                RawEvent::UdpRecv(n) => Some(format!("udp_recv s{}:{} d{}:{}", n.saddr, n.sport, n.daddr, n.dport)),
                _ => None,
            })
            .collect()
    };
    let has = |step: &str, f: &dyn Fn(&RawEvent) -> bool| rev(step).iter().any(|e| f(e));
    for (step, ports, v6) in [("tcp_v4", tcp4, false), ("tcp_v6", tcp6, true)] {
        let (client, server) = (port(ports, 0), port(ports, 1));
        r.check(
            &format!("{step}_connect_and_accept_have_local_as_saddr"),
            has(step, &|e| {
                matches!(e, RawEvent::TcpConnect(n) if n.sport == client && n.dport == server && n.daddr.is_ipv6() == v6)
            }) && has(step, &|e| matches!(e, RawEvent::TcpAccept(n) if n.sport == server && n.dport == client)),
            format!("{:?}", net(step)),
        );
        r.check(&format!("{step}_disconnect"), has(step, &|e| matches!(e, RawEvent::TcpDisconnect(_))), "");
    }
    for (step, ports, v6) in [("udp_v4", udp4, false), ("udp_v6", udp6, true)] {
        let (sender, receiver) = (port(ports, 0), port(ports, 1));
        r.check(
            &format!("{step}_send_local_is_saddr_receive_local_is_daddr"),
            has(step, &|e| {
                matches!(e, RawEvent::UdpSend(n) if n.sport == sender && n.dport == receiver && n.daddr.is_ipv6() == v6)
            }) && has(step, &|e| matches!(e, RawEvent::UdpRecv(n) if n.sport == sender && n.dport == receiver)),
            format!("{:?}", net(step)),
        );
    }

    // DNS.
    let q = |step: &str| -> Vec<String> {
        rev(step)
            .iter()
            .filter_map(|e| match e {
                RawEvent::DnsQuery(q) => Some(format!(
                    "{} type {} status {} results {:?}",
                    q.query_name.to_string_lossy(),
                    q.query_type,
                    q.query_status,
                    q.query_results
                )),
                _ => None,
            })
            .collect()
    };
    r.check(
        "dns_3008_for_example_com",
        has("dns_example", &|e| {
            matches!(e, RawEvent::DnsQuery(q) if q.query_name.to_string_lossy() == "example.com" && q.query_type == 1)
        }),
        format!("{:?} (DnsQuery_W returned {})", q("dns_example"), actor["dns_ok"]),
    );
    r.check(
        "dns_3008_for_nxdomain",
        has("dns_nxdomain", &|e| {
            matches!(e, RawEvent::DnsQuery(q) if q.query_name.to_string_lossy() == "atlas-etw-live.invalid" && q.query_status != 0)
        }),
        format!("{:?} (DnsQuery_W returned {})", q("dns_nxdomain"), actor["dns_nx"]),
    );
    r.note("dns_localhost", json!(q("dns_localhost")));
}

/// What Kernel-EventTracing logged around each enable change, and whom it
/// attributes the change to (plan 1b-4 input).
fn explore_watch(r: &mut Report, steps: &Steps, watch: &[Watched], observer: u32) {
    let mut out = Map::new();
    for step in [
        "enable_a",
        "ctl_disable_network",
        "ctl_reenable_network",
        "ctl_reapply_same_file_enable",
        "ctl_narrow_file_filter",
        "ctl_restore_file_filter",
        "ctl_stop_b_by_name",
    ] {
        let (a, b) = steps.window(step);
        let evs: Vec<Value> = watch
            .iter()
            .filter(|(ts, ..)| *ts >= a && *ts <= b)
            .map(|(_, id, pid, key, f)| {
                json!({
                    "id": id,
                    "header_pid_is_the_caller": *pid == observer,
                    "has_start_key": key.is_some(),
                    "fields": f.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<Map<_, _>>(),
                })
            })
            .collect();
        out.insert(step.into(), Value::Array(evs));
    }
    out.insert("total_events".into(), json!(watch.len()));
    r.note("kernel_event_tracing", Value::Object(out));
}

fn finish(r: Report, events: &[Captured]) {
    let failed: Vec<_> = r.checks.iter().filter(|(_, ok, _)| !ok).collect();
    for (name, ok, detail) in &r.checks {
        let detail = if *ok || detail.is_empty() { String::new() } else { format!(": {detail}") };
        println!("{} {name}{detail}", if *ok { "PASS" } else { "FAIL" });
    }
    let doc = json!({
        "checks": r.checks.iter().map(|(n, ok, d)| json!({ "name": n, "ok": ok, "detail": d })).collect::<Vec<_>>(),
        "notes": r.notes,
    });
    if let Ok(path) = std::env::var("ATLAS_ETW_REPORT") {
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).expect("write report");
    }
    if let Ok(path) = std::env::var("ATLAS_ETW_RECORD") {
        let mut lines: Vec<&Captured> = events.iter().filter(|c| c.event.is_ok() && c.tdh.is_ok()).collect();
        lines.sort_by_key(|c| c.ts);
        let text: String = lines.iter().map(|c| format!("{}\n", c.fixture_line())).collect();
        std::fs::write(&path, text).expect("write fixtures");
    }
    assert!(failed.is_empty(), "{} check(s) failed", failed.len());
}
```

- [ ] **Step 2: Check and commit**

```powershell
cargo clippy -p atlas-etw --all-targets -- -D warnings
cargo test -p atlas-etw --test live
```
Expected: clean; the live test is listed as ignored. It runs in CI (Task 6). On the host, only the user runs it, elevated:
```powershell
cargo test -p atlas-etw --test live -- --ignored --test-threads=1 --nocapture
```
At planning time (run 4), all 45 checks passed.
```powershell
git add crates/atlas-etw/tests/live.rs
git commit -m "test(etw): live sessions with an actor process (tier 3)"
```

### Task 6: Fuzz targets, CI, documentation

**Files:**
- Create: `crates/atlas-etw/fuzz/Cargo.toml`, `fuzz_targets/parse_any.rs`, `fuzz_targets/dns_query_results.rs`
- Modify: `.github/workflows/ci.yml`, `.github/workflows/fuzz.yml`, `docs/specs/2026-10-01-etw-sensor-design.md`, `docs/architecture-overview.md`

- [ ] **Step 1: Fuzz crate**

`crates/atlas-etw/fuzz/Cargo.toml`:
```toml
[package]
name = "atlas-etw-fuzz"
version = "0.0.0"
edition = "2024"
publish = false

[package.metadata]
cargo-fuzz = true

[dependencies]
atlas-etw = { path = ".." }
libfuzzer-sys = "0.4"

[[bin]]
name = "parse_any"
path = "fuzz_targets/parse_any.rs"
test = false
doc = false
bench = false

[[bin]]
name = "dns_query_results"
path = "fuzz_targets/dns_query_results.rs"
test = false
doc = false
bench = false

# Standalone workspace: cargo-fuzz needs nightly + sanitizers, so keep it out
# of the main workspace build.
[workspace]
members = ["."]
```

`crates/atlas-etw/fuzz/fuzz_targets/parse_any.rs`:
```rust
#![no_main]

use atlas_etw::layout::LAYOUTS;
use atlas_etw::parse::{EventMeta, PointerSize, parse, parse_as};
use libfuzzer_sys::fuzz_target;

// Every parser, both pointer sizes, known and newer versions (sensor spec §12.1).
// The first two bytes pick the event, pointer size and version; the rest is the
// payload. Parsing must never panic, and must be deterministic.
fuzz_target!(|data: &[u8]| {
    let [pick, flags, payload @ ..] = data else { return };
    let l = &LAYOUTS[usize::from(*pick) % LAYOUTS.len()];
    let pointer_size = if flags & 1 == 0 { PointerSize::P64 } else { PointerSize::P32 };
    let meta = EventMeta { provider: l.provider, id: l.id, version: l.version.saturating_add(flags >> 6), pointer_size };
    let first = parse(&meta, payload);
    assert_eq!(first, parse(&meta, payload));
    // A newer version parsed with the known layout (after the version check) must not panic either.
    let _ = parse_as(&meta, l.version, payload, flags & 2 != 0);
});
```

`crates/atlas-etw/fuzz/fuzz_targets/dns_query_results.rs`:
```rust
#![no_main]

use atlas_etw::parse::{DnsAnswers, parse_query_results};
use libfuzzer_sys::fuzz_target;

// DNS-Client 3008's QueryResults text (sensor spec §5.4): arbitrary UTF-16 never
// panics and never yields more than 0a's 64 answers.
fuzz_target!(|data: &[u8]| {
    let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let a = parse_query_results(&units);
    assert!(a.answers.len() <= DnsAnswers::MAX);
    assert!(!a.truncated || a.answers.len() == DnsAnswers::MAX);
});
```

```powershell
cargo check --manifest-path crates/atlas-etw/fuzz/Cargo.toml
```
Expected: it compiles (this also creates `fuzz/Cargo.lock`, which is committed like the other fuzz crates').

- [ ] **Step 2: CI**

The `etw-live` job runs the live test on the hosted Windows runner, whose account is an administrator. It uploads the recording and the report as the `etw-live` artifact, even on failure.

`.github/workflows/ci.yml`:
```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -31,12 +31,14 @@ jobs:
             . -> target
             crates/atlas-schema/fuzz -> target
             crates/atlas-buffer/fuzz -> target
+            crates/atlas-etw/fuzz -> target
       - run: cargo fmt --all --check
       - run: cargo clippy --workspace --all-targets -- -D warnings
       - run: cargo test --workspace
       # Each fuzz crate is its own workspace, so the commands above don't build them.
       - run: cargo check --manifest-path crates/atlas-schema/fuzz/Cargo.toml
       - run: cargo check --manifest-path crates/atlas-buffer/fuzz/Cargo.toml
+      - run: cargo check --manifest-path crates/atlas-etw/fuzz/Cargo.toml
 
   rust-windows:
     runs-on: windows-latest
@@ -51,6 +53,36 @@ jobs:
       - run: cargo clippy --workspace --all-targets -- -D warnings
       - run: cargo test --workspace
 
+  # Tier 3 (sensor spec §12.3): real ETW sessions. The hosted runner's account is an
+  # administrator, which starting sessions needs. The run also records the scenario's
+  # events as replay fixtures (§12.2): download the artifact, review it, and commit it
+  # as crates/atlas-etw/tests/fixtures/scenario.jsonl.
+  etw-live:
+    runs-on: windows-latest
+    steps:
+      - uses: actions/checkout@v7
+      - name: Install Rust ${{ env.RUST_TOOLCHAIN }}
+        shell: bash
+        run: |
+          rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal
+          rustup default "$RUST_TOOLCHAIN"
+      - uses: Swatinem/rust-cache@v2
+      - name: Live sessions
+        shell: bash
+        env:
+          ATLAS_ETW_RECORD: ${{ runner.temp }}/etw/scenario.jsonl
+          ATLAS_ETW_REPORT: ${{ runner.temp }}/etw/report.json
+        run: |
+          mkdir -p "$RUNNER_TEMP/etw"
+          cargo test -p atlas-etw --test live -- --ignored --test-threads=1 --nocapture
+      - name: Upload the recording and report
+        if: always()
+        uses: actions/upload-artifact@v7
+        with:
+          name: etw-live
+          path: ${{ runner.temp }}/etw/
+          if-no-files-found: ignore
+
   proto:
     runs-on: ubuntu-latest
     steps:
@@ -101,3 +133,4 @@ jobs:
       - run: cargo audit
       - run: cargo audit --file crates/atlas-schema/fuzz/Cargo.lock
       - run: cargo audit --file crates/atlas-buffer/fuzz/Cargo.lock
+      - run: cargo audit --file crates/atlas-etw/fuzz/Cargo.lock
```

`.github/workflows/fuzz.yml`:
```diff
--- a/.github/workflows/fuzz.yml
+++ b/.github/workflows/fuzz.yml
@@ -11,6 +11,7 @@ on:
       - "crates/atlas-schema/**"
       - "crates/atlas-proto/**"
       - "crates/atlas-buffer/**"
+      - "crates/atlas-etw/**"
       - ".github/workflows/fuzz.yml"
 
 permissions:
@@ -38,7 +39,9 @@ jobs:
           # crate: the directory holding fuzz/. watch: path prefixes that trigger the target on a PR.
           targets='[
             {"target": "decode_event", "crate": "crates/atlas-schema", "watch": ["crates/atlas-schema/", "crates/atlas-proto/"]},
-            {"target": "buffer_recover", "crate": "crates/atlas-buffer", "watch": ["crates/atlas-buffer/"]}
+            {"target": "buffer_recover", "crate": "crates/atlas-buffer", "watch": ["crates/atlas-buffer/"]},
+            {"target": "parse_any", "crate": "crates/atlas-etw", "watch": ["crates/atlas-etw/src/", "crates/atlas-etw/fuzz/"]},
+            {"target": "dns_query_results", "crate": "crates/atlas-etw", "watch": ["crates/atlas-etw/src/parse/", "crates/atlas-etw/fuzz/"]}
           ]'
           if [ "$EVENT" = pull_request ]; then
             changed=$(git diff --name-only "$BASE"...HEAD | jq -R . | jq -s .)
```

```powershell
actionlint .github/workflows/ci.yml .github/workflows/fuzz.yml
```
Expected: no findings.

- [ ] **Step 3: Spec and decision log**

In `docs/specs/2026-10-01-etw-sensor-design.md`:
- Status line: plan 1b-2 is done (`atlas-etw`); 1b-3 is next.
- §3.1/§3.2 [1]: clarification 2.
- §4.3: clarifications 3 and 4.
- §7.3: clarification 7 (F5).
- §7.4: clarification 6. Replace "The spikes did not examine CloseKey. Plan 1b-2 verifies …" with F1.
- §7.5: clarification 5.
- §9.1: clarification 8.
- §12.2 and §12.3: clarifications 1 and 9.
- §15.3: a "Plan 1b-2 (2026-10-05)" addendum with F1–F9.
- §17: a "Plan 1b-2 clarifications" paragraph listing 1–9.

In `docs/architecture-overview.md`:
- Roadmap row 1: "Building: plans 1b-1 and 1b-2 done (`atlas-buffer`, `atlas-etw`); plan 1b-3 (pipeline) next", with a link to this plan.
- Decision log, two rows (wording in the Review Log once approved): the plan's approval with D1 and D2; and the build.

```powershell
git add crates/atlas-etw/fuzz .github docs
git commit -m "ci(etw): fuzz targets, live-session job; docs: plan 1b-2 clarifications and findings"
```

### Task 7: PR, recorded fixtures, merge

- [ ] **Step 1: Push and open the PR**

```powershell
git push -u origin feat/1b-2-atlas-etw
gh pr create --title "Sub-project 1b-2: atlas-etw" --body-file <body>
```
The body summarises the crate, D1 and D2, F1–F9, and the test counts, and ends with the attribution line.

- [ ] **Step 2: Wait for CI**

All jobs must pass: `rust-linux`, `rust-windows`, `etw-live`, `proto`, `powershell`, `audit`, and the fuzz runs for `parse_any` and `dns_query_results` (2 minutes each on a PR).
- If `etw-live` fails, read the uploaded report first (`gh run download <run> -n etw-live`). A failed check there means a difference between build 26100 (the runner) and 26200 (the host): fix it, with a replay test that pins it.
- Replay calls `parse`, not the gate. If a future runner image logs a newer version, replay fails with `NewerVersion`, even where the live gate accepted it. That alarm is intended: check the new version's manifest and add it to the layout table.

- [ ] **Step 3: Commit the runner's recording**

```powershell
gh run download <run-id> -n etw-live -D $env:TEMP\etw-live
Copy-Item $env:TEMP\etw-live\scenario.jsonl crates\atlas-etw\tests\fixtures\scenario.jsonl
cargo test -p atlas-etw --test replay
```
Review the recording before committing it:
- only the runner's paths, user (`runneradmin`), test keys and DNS names (`example.com`, `atlas-etw-live.invalid`, `localhost`) appear;
- its size is about 0.5 MB;
- `committed_fixtures_agree_with_tdh` passes and finds every kind in the layout table.
```powershell
git add crates/atlas-etw/tests/fixtures/scenario.jsonl
git commit -m "test(etw): replay fixtures recorded on the CI runner"
git push
```

- [ ] **Step 4: Merge**

After CI is green on the fixture commit, the user merges. Update the decision log row for the build (Task 6, Step 3) if anything changed on the way.

## Review Log

**Independent review (2026-10-05)** of the draft plan and its worktree. The reviewer reran the tests and clippy, confirmed the per-task counts, and found no blockers. Findings and what changed:

- **Major**
  - **M1, a stale session handle.** After an external stop, `Session` could act on, or drop-stop, whichever session reused its LoggerId. Fixed with a current-session check on every call, `STALE`, `is_current` and `abandon` (clarification 10, live check).
  - **M2, a forgeable version check.** `VersionGate` read the layout from the live record, so a forged DNS-Client event could decide a cached verdict. It now reads the installed manifest or MOF class (clarification 11). A test shows the accept path with a real pair: ProcessStart v3 against the installed v4.
  - **M3, a vacuous replay.** Unreadable fixture lines were skipped silently. Now a committed line that does not read fails, as do files that yield no events. Run 4's recording was checked both ways.
- **Minor**
  - m1: the live test excused every TDH decode failure; now only the embedded-NUL SetValueKey is excused.
  - m2: a SetValueKey name that could end in two places is flagged (`value_name_ambiguous`), with a unit test; the S4 dependency is noted in clarification 5.
  - m3: a newer version with an equal layout is parsed exactly (`parse_as(…, exact)`, `Verdict::Same`).
  - m4: `consume` no longer leaks the trace handle and context if spawning the thread fails.
  - m5: the TDH buffers' trailing arrays are sliced from the owning buffer, with clamped counts (`trailing_array`).
  - m6: added the live checks for the event-ID filter and the QPC frequency, and the gate's accept-path test.
  - m7: the callback-panic guidance is under "Interfaces for later plans".
  - m8: fixed mojibake in the CI diff (the generator now decodes `git diff` as UTF-8); the fuzz targets also watch `crates/atlas-etw/fuzz/`; the Task 7 note on `NewerVersion` in replay.
  - m9: `FileOpEnd::failed`'s wording is clarification 12. The `EtwError` HRESULT display was checked and is correct (`from_win32` passes a failing HRESULT through unchanged).
  - m10: process events are kept regardless of order and the tree is rebuilt by timestamp.
- **Not changed:** none.

Then run 4 of the live test passed all 45 checks.
