# EDR — Architecture Overview & Roadmap

**Status:** Agreed direction (brainstorm, 2026-09-24). Each sub-project below gets its own detailed spec → implementation plan → build cycle before any code is written for it.

This document is the top-level source of truth for *why* the project is shaped the way it is. Detailed designs live in `docs/specs/`.

---

## 1. Goals

In priority order:

1. **Protect my own machines** — something I actually run day-to-day. Reliability and low overhead matter more than feature count. A bluescreen on a daily machine is a failure.
2. **Learning** — understand how real EDRs work (Windows internals, ETW, kernel drivers). Build the real pieces rather than wrapping existing tools (e.g., not "just Sysmon").
3. **Detection-engineering lab** — write detections and test them against attack techniques (e.g., Atomic Red Team) in a safe VM.
4. **Product** — possibly, eventually. Influences architecture hygiene (clean rule format, proper agent/server split), not current features.

**Guiding principle:** choose the optimal approach, not the familiar one. Language/tooling comfort is not a constraint.

## 2. Scope Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Platforms | **Windows first**, Linux later | Start small. Design the *seams* cross-platform; do not abstract sensor internals prematurely. |
| Topology | **Agents + separate server** | Real EDR model; an attacker on the endpoint can't trivially erase evidence. |
| Dev deployment | **All on one PC** | Server stack in Docker (Linux containers via WSL2); agent native on Windows; driver work in a Hyper-V VM. |
| Capabilities | **Detect + respond + prevent** | Prevention seam built in from day one; enforcement switched on later (sub-project 7). |
| Scale (assumed) | < 10 endpoints, single user | No multi-tenancy for now. |

### Cross-platform boundary

**Designed platform-neutral now** (cheap now, expensive to retrofit):
- **Event schema** — a process-start event looks the same from Windows ETW or Linux eBPF.
- **Detection rules** — operate on the normalized schema, never raw OS events.
- **Agent↔server protocol** — normalized events up, generic commands down.

**Windows-specific, not abstracted:**
- **Sensor internals** (ETW, kernel callbacks, minifilter). Linux (eBPF/fanotify/auditd) shares almost nothing; a future Linux sensor is a sibling that emits the same schema.
- **Response execution** — the *command* is generic ("isolate host"); the *implementation* is per-OS.

Agent shape: `[OS-specific sensor] → normalize → [shared pipeline: buffer, local detection, transport]`

## 3. Technology Stack

| Layer | Choice | Why |
|---|---|---|
| Kernel driver | **C** (WDK) | All Microsoft samples/docs/debugging assume C; Rust drivers still rough. Keep the driver **thin**: collect & forward only, no parsing or logic in kernel. |
| Agent (user-mode service) | **Rust** | Runs as SYSTEM parsing untrusted data → memory safety. No GC pauses, small footprint, harder to tamper with than managed runtimes. Official `windows` crate. |
| Server | **Rust** | Shared schema crate with the agent → types can't drift. |
| Transport | **gRPC over mTLS** | Bidirectional streaming (events up, commands down), typed contract, mutual cert auth for enrolled agents. |
| Telemetry storage | **ClickHouse** | Columnar, built for high-volume append-only event search. Leaner than Elasticsearch/OpenSearch. |
| State storage | **PostgreSQL** | Agents, alerts, rules, users, command history — relational, transactional. |
| Detection | **Sigma → compiled streaming matcher (Rust)** | Industry-standard rule format with large community corpus; compiled for speed. |
| Console | **TypeScript + React** (or Svelte — decide in sub-project 5) | Conventional; nothing EDR-specific. |
| Event schema | **Own typed model, OCSF-modeled** (OCSF 1.9.0); protobuf wire + Rust domain types | See [0a spec](specs/2026-09-24-event-schema-design.md). |

Repo: single monorepo — Cargo workspace (agent, server, shared crates) + driver + UI.

## 4. Key Architectural Principles

1. **Detection runs on the agent *and* the server.** The detection engine is a shared Rust crate.
   - Agent: fast, local, works offline, can produce prevention verdicts in milliseconds.
   - Server: cross-host correlation, historical queries, rules too heavy for the endpoint.
2. **Prevention seam from day one.**
   - The driver registers *blocking-capable* callbacks (`PsSetCreateProcessNotifyRoutineEx`, minifilter pre-op callbacks, `ObRegisterCallbacks` for handle stripping e.g. LSASS protection) but initially always returns "allow".
   - Every rule has a **mode: `audit` | `enforce`**. Prevention = promoting proven rules from audit to enforce. Audit mode shows exactly what *would* have been blocked.
3. **Thin kernel, smart user-mode.** Minimize kernel code; all parsing/logic/policy lives in Rust user-mode.
4. **ETW sensor first, driver later.** The user-mode sensor gets the full loop working with zero BSOD risk and is what protects real machines soonest. The driver becomes an *additional* sensor feeding the same schema.
5. **Offline resilience.** Agent buffers events on disk when the server is unreachable.

## 5. Driver Signing Constraint

Windows only loads kernel drivers signed via Microsoft. Production (attestation) signing requires an EV code-signing certificate (~$300/yr, typically needs a registered business) and a Partner Center account. Test-signing mode works but weakens the host's security.

Practical consequence:
- **Test VM:** full agent + test-signed driver.
- **Daily machines:** agent in **ETW-only mode** until signing is resolved.
- ELAM / PPL-antimalware protection (and the `Microsoft-Windows-Threat-Intelligence` ETW provider) require Microsoft Virus Initiative membership — out of reach for now.

## 6. Development Environment

```
This PC (Windows 11 Pro host)
├── Docker Desktop (WSL2) → server stack containers (server, ClickHouse, Postgres, console)
├── IDE / build toolchains (Rust, WDK, Node)
└── Hyper-V VM "edr-test" (Windows 11, test-signing ON, snapshots)
      └── agent + driver → sends to server on host
```

- The agent **cannot** run in Docker — containers can't see host processes/ETW/kernel.
- Driver development and attack simulation (goal 3) happen **only in the VM**; snapshot before every driver load.

## 7. Roadmap (Sub-projects)

Each row: its own spec → plan → implementation.

| # | Sub-project | Delivers | Status |
|---|---|---|---|
| 0a | **Foundations: event schema** | `atlas-proto` + `atlas-schema` crates, OCSF-modeled, 7 event classes ([spec](specs/2026-09-24-event-schema-design.md)) | Done |
| 0b | **Foundations: scaffolding** | Hyper-V test VM setup, CI (incl. `buf breaking` and the 0a `cargo fuzz` `decode_event` 10-min run) | Done ([spec](specs/2026-09-25-scaffolding-design.md), [runbook](runbooks/edr-test-vm.md)) |
| 1 | **Agent: ETW sensor** | Process / image-load / network / file / registry telemetry → normalized events; on-disk offline buffer | Building: plans 1b-1, 1b-2 and 1b-3a done (`atlas-buffer`, `atlas-etw`, the `atlas-agent` pipeline core); plan 1b-3b (Windows services) approved 2026-10-06, build next; then 1b-3c (driver + agent-level live test) and 1b-4 ([spec](specs/2026-10-01-etw-sensor-design.md), [plan 1a](plans/2026-10-02-etw-sensor-spikes-plan.md), [plan 1b-1](plans/2026-10-04-etw-sensor-1b-1-plan.md), [plan 1b-2](plans/2026-10-05-etw-sensor-1b-2-plan.md), [plan 1b-3a](plans/2026-10-05-etw-sensor-1b-3a-plan.md), [plan 1b-3b](plans/2026-10-06-etw-sensor-1b-3b-plan.md)) |
| 2 | **Server: ingest + storage** | Agent enrollment, mTLS gRPC ingest, ClickHouse + Postgres (incl. Docker Compose stack) | — |
| 3 | **Detection engine** | Sigma → compiled matcher, shared by agent + server; alerts | — |
| 4 | **Response** | Command channel; kill process, quarantine file, network-isolate host (WFP) | — |
| 5 | **Console** | Alerts, host view, process tree, event search, response actions | — |
| 6 | **Kernel driver** | Process/thread/image/object/registry callbacks + filesystem minifilter as additional sensor; self-protection | — |
| 7 | **Prevention** | Enforce mode wired through driver blocking callbacks | — |
| 8 | **Hardening** | Tamper resistance, agent auto-update, performance budgets | — |
| — | **Linux sensor** (future) | eBPF-based sensor emitting the same schema | — |

**Milestones:**
- After #2 — working telemetry pipeline.
- After #5 — usable EDR on real machines.
- After #6–8 — serious EDR.

## 8. Open Decisions

- Console framework: React vs Svelte (sub-project 5).
- Whether to pursue an EV cert for production driver signing (before relying on the driver on daily machines).

## 9. Decision Log

| Date | Decision |
|---|---|
| 2026-09-24 | Goals ordered B (protect) > A (learn) > C (lab) > D (product). |
| 2026-09-24 | Windows first; cross-platform seams at schema/rules/protocol only. |
| 2026-09-24 | Agent + server topology; single-PC dev via Docker + Hyper-V VM. |
| 2026-09-24 | Stack: C driver, Rust agent/server, gRPC+mTLS, ClickHouse + Postgres, Sigma, TS console. |
| 2026-09-24 | Detect + respond first; prevention seam built in, enforcement later (audit/enforce rule modes). |
| 2026-09-24 | Build order: ETW sensor before kernel driver. |
| 2026-09-24 | Sub-project 0 split into 0a (event schema) and 0b (scaffolding). |
| 2026-09-24 | Event schema: own typed model modeled on OCSF 1.9.0; OCSF/ECS exporters as later adapters. |
| 2026-09-24 | Schema source of truth: protobuf is the wire contract, Rust domain types are the code model, and a validating `TryFrom` sits between them. |
| 2026-09-24 | Process identity: `uid = BLAKE3(device.uid, boot_id, ProcessStartKey)`, which is deterministic and stateless across sensors. |
| 2026-09-24 | v1 event classes: Process, Module, Network, File System, Registry Key, Registry Value, DNS. |
| 2026-09-24 | Events carry an actor-process core; full process detail is only in Launch, and a process cache fills in the rest. |
| 2026-09-24 | 0a implemented: `atlas-proto` + `atlas-schema`; unknown classes/activities from newer agents are rejected as Missing; `parent_process` optional. |
| 2026-09-24 | 0a's 10-minute `cargo fuzz` run deferred to 0b CI (Docker Desktop was unavailable). Stable hostile-input property tests cover the decoder until then. |
| 2026-09-24 | From the 0a final review: `reg_value.type` has explicit wire presence (absent means `Missing`, not `REG_NONE`); every class checks its activity before class fields; `user.uid` ≤ 256 B, `user.name` and `signature.signer` ≤ 1 KiB. |
| 2026-09-24 | Repo made public (free GitHub-hosted CI). Licensed AGPL-3.0-only: open for lab/personal use, modified network deployments must share source, and the sole copyright holder keeps the dual-licensing (product) option. Replaces the unfiled `MIT OR Apache-2.0` Cargo metadata. |
| 2026-09-24 | Docker Compose (ClickHouse, Postgres) moved from 0b to sub-project 2, where its first consumer lives; 0b = CI + Hyper-V test VM. |
| 2026-09-24 | 0b CI: GitHub Actions. Linux and Windows Rust jobs, `buf lint` + `buf breaking`, PSScriptAnalyzer/Pester, `cargo audit` on every push and PR; `cargo fuzz` nightly for 10 min, plus 2 min on schema/proto PRs; Dependabot weekly. |
| 2026-09-24 | 0b test VM: scripted Hyper-V build plus a runbook; isolated internal switch by default with NAT on demand; Windows 11 Enterprise Evaluation; test-signing and KDNET on; Secure Boot, HVCI and Defender real-time protection off (VM only, never the host). |
| 2026-09-25 | 0b design approved (repo layout, Pester-with-mocks + PSScriptAnalyzer for VM scripts, manual acceptance checklist, DoD). Verified: Win11 setup needs Secure Boot on (turned off after install); KDNET uses VMBus (no `busparams`), so VM NICs are identified by Hyper-V device naming; 0a protos pass `buf lint` STANDARD unmodified. |
| 2026-09-25 | 0b plan: one `EdrTestVm` PowerShell module holds all VM logic; scripts are thin wrappers; every external command is stubbed in tests so no test can reach the real host; the module stays Windows PowerShell 5.1 compatible and ASCII-only for the guest. |
| 2026-10-01 | Sub-project 1 scope: agent core (ETW sensor, process cache and enrichment, on-disk buffer, dev CLI, service wrapper); no transport. Developed on the host (user-mode only); attack validation waits for the VM. |
| 2026-10-01 | ETW via our own `atlas-etw` crate on the `windows` crate with hand-written, fuzzed parsers; TDH only as a test oracle. Not `ferrisetw`. |
| 2026-10-01 | Providers: manifest Kernel-Process/File/Registry/Network + DNS-Client in one session, plus a minimal system-logger session for process start, because Kernel-Process ProcessStart has no command line. |
| 2026-10-01 | File telemetry: changes only, with one Update per handle and no Read events; a watchlist of sensitive paths emits File System Activity `Open`. |
| 2026-10-01 | Launched images and loaded modules get SHA-256 + Authenticode, cached by file identity, with no online revocation checks. |
| 2026-10-01 | Buffer: own append-only segment log (CRC32C records, ack cursor). On overflow it keeps the head and the tail and drops the middle. |
| 2026-10-01 | Network: TCP Open/Close; UDP flows synthesized in the agent, kept only if a measured CPU gate passes. |
| 2026-10-01 | Blinding detection in sub-project 1: watchdog + canary, reported as Event Log Activity (OCSF 1008) and an Atlas "Sensor Health" extension class. |
| 2026-10-01 | Pipeline: an ordering stage (750 ms hold, raw QPC) comes before a single-threaded stateful pipeline, followed by an ordered completion stage for enrichment and the Launch join. This corrects the brainstorm design, which ordered events only *after* the pipeline. |
| 2026-10-01 | Unresolvable actors: never drop an event for lack of context. If the uid is computable, emit with an empty path and count it; drop and count only when no uid can be computed. One uid formula applies across all paths, chosen once by spike S1. |
| 2026-10-01 | The process session uses the legacy `EVENT_TRACE_FLAG_PROCESS` in a system-logger session on every build, giving one CI-tested path that includes Windows 10 22H2. |
| 2026-10-01 | Schema deviations: Event Log Activity has an optional actor and Sensor Health has none (0a D5 assumed one on every event); Sensor Health is an Atlas extension *class* (0a §5.9 reserved the namespace for fields). |
| 2026-10-01 | While there is no transport, the buffer runs as rolling local retention; head-plus-tail overflow switches on with sub-project 2. The agent installs to `Program Files` and refuses to start if its `ProgramData` directory's owner or DACL is wrong or it contains a reparse point. |
| 2026-10-01 | Sub-project 1 design approved (spec rev 2, after an independent review). Next is the implementation plan; its phase 0 is spikes S1–S10 on the host. |
| 2026-10-01 | 0a done. The scheduled nightly 10-minute `cargo fuzz` runs of `decode_event` have been clean every night since 2026-09-27, which closes 0a DoD §8.2.2. |
| 2026-10-01 | 0b: the VM build started after disk space was freed. The runbook now notes the ~5 s "press any key to boot from DVD" prompt; missing it produces Hyper-V event 18603. |
| 2026-10-01 | Sub-project 1 gets two plans, each reviewed and approved before it runs. Plan 1a covers spikes S1–S10: throwaway code in a git-ignored `spikes/` folder, with results written into spec §15.3. Plan 1b, the build with full code, is written from the spike results. S1/S2 (reboots, clock changes) prefer the VM if it is ready. |
| 2026-10-01 | 0b done: CI green on main; the first nightly 10-minute fuzz run of `decode_event` was clean (closes 0a DoD §8.2.2); `edr-test` was built from the runbook and all 10 acceptance items pass (KDNET while Online: connects, and KDNET still takes the internal NIC). |
| 2026-10-01 | First VM build confirmed 0b review findings #3 and #5. **KDNET replaces the internal NIC:** the design keeps the Kernel Debug Network Adapter as the internal NIC, and a `-NetworkOnly` guest run re-applies `192.168.77.10` before the baseline (rejected: serial debugging, which is slower; a second NIC dedicated to KDNET, since we can't choose which NIC it takes). **Defender reverted real-time protection:** the script also sets the policy value. The guest script now refuses to run outside a Hyper-V VM, and winget is pinned to `--source winget`. |
| 2026-10-02 | Plan 1a approved. The spikes use one throwaway Rust probe (`windows` crate, TDH as oracle) whose source lives in the plan's appendix; `spikes/` stays git-ignored. The user runs elevated steps and Claude analyzes the output, so the agent never runs with admin rights on the host. Thresholds the spec left open: UDP stays on if it costs ≤ 2.5% of one core with zero loss (S9); a process canary is added if it costs ≤ 0.1% of one core (S8); the Launch deadline is raised to the p99 if > 5% of first launches miss it (S10); a > 5% clean-build slowdown with tracing on comes back as a decision (S10). |
| 2026-10-02 | Spikes S1 and S2 (spec §15.3). The start key is `(BootId << 48) \| ProcessSequenceNumber` (spec §5.2 row 2), so Launch and actor uids come from one formula without the fallback. `boot_time` is the creation time of the System process (PID 4), the only cheap source unaffected by clock changes. BootId is read from `KUSER_SHARED_DATA` (offset 0x2c4), never from the registry's `PrefetchParameters\BootId`, which lagged the kernel value by one in the VM (refines 0a §4.4). The probe links the C runtime statically; the agent must too, because fresh machines lack `VCRUNTIME140.dll`. |
| 2026-10-02 | Spike S8 (spec §15.3): the classic Process Start v4 layout is pinned, and the PID + 200 ms Launch join paired 201 of 201 launches (1–56 µs apart). A process-launch canary costs 0.082% of one core with Defender on, within the 0.1% threshold, so §9.2 adds one: the agent starts itself with a no-op argument every 60 s. |
| 2026-10-02 | Spike S3 (spec §15.3): DNS-Client 3008 is attributed by its header PID and start key, which name the requesting process, including for cache hits, NXDOMAIN and 32-bit callers. No sibling-event join is needed. The provider is enabled with only the Operational channel keyword (`0x8000000000000000`). |
| 2026-10-02 | Spikes S4 and S7 (spec §15.3). **Value data:** Kernel-Registry never fills `CapturedData`, so the agent reads the value right after a SetValueKey and marks it with an additive `data_read_after` flag (rejected: no data, which would leave detections blind to what a persistence key points to until the driver exists). **Key paths:** names are relative to per-handle `KeyObject`s, so the agent keeps a `KeyObject → name` map from CreateKey/OpenKey (OpenKey joins §4.2). **Pre-existing handles:** the map is seeded at startup from the system handle table, whose object addresses equal `KeyObject` (verified; ~140 ms for about 15k handles, 93% named). Anything still unresolved is emitted with its relative name and an `unresolved` flag (rejected: floor only; dropping, which contradicts E11). Key rename is a confirmed v1 gap; the provider has no rundown. |
| 2026-10-02 | Spike S6 (spec §15.3). Kernel-File `DeletePath`/`RenamePath` fire before the outcome, so `OperationEnd` (24) joins §4.2 and failed operations are dropped by `Irp` status. Rename takes the new name from 27 and the old name from the handle map. Overwrites are a truncation (`SetInformation`, end of file) on a mapped handle. Delete-on-close has no event of its own and is detected from `CreateOptions`. 8.3 names are expanded before watchlist matching. **Pre-existing file handles:** the `FileObject` map is seeded from the system handle table, the same approach as the registry (verified; rejected: keeping the documented gap). Only disk files are named, from a worker thread with a timeout. |
| 2026-10-02 | Spike S9 (spec §15.3): UDP flows stay on by default. Under real QUIC streaming (about 2 900 UDP events/s, peaks near 11 000/s) the probe with the full provider set used 0.495% of one core and lost nothing, within the 2.5% gate. The §4.1 buffer defaults lost nothing at about 10 000 events/s on the host. |
| 2026-10-02 | Spike S10 (spec §15.3) and plan 1a complete. Under a clean build plus browsing, the probe with the full provider set used 0.39–0.42% of one core and an 18–26 MB working set, and lost nothing, so no §13 fallback is needed. No first launch missed the 1 s enrichment deadline (p99 540 ms), so the deadline stays. Builds run about 3% slower (median of 9 pairs) with the sessions on; the mean straddled the 5% D3 line only through noisy pairs. **Accepted** (rejected: the §13 fallback of turning off File Update and SetAttributes); sub-project 8 re-measures on an idle machine. Plan 1b is next. |
| 2026-10-02 | Before plan 1b, the spec gets a revision 3 that folds the spike results and decisions into its design sections, with an independent review (rejected: planning straight from §15.3, which would leave the plan as the only full design). Plan 1b is split along crate seams into four plans, each reviewed and built before the next: **1b-1** schema additions + `atlas-buffer`; **1b-2** `atlas-etw` (sessions, parsers, replay/TDH tests); **1b-3** `atlas-agent` pipeline, enrichment, value reads and seeding; **1b-4** health, service, CLI, the 24-hour run, runbook (rejected: one plan of ~6–8k lines, too long to review well). |
| 2026-10-02 | Spec rev 3 review: E12 re-decided. The independent review showed that reading registry values on the ordered path happens about 1 s after the write, not "right after" as first described. **Fast path chosen:** Session A's event callback keeps an early registry key map and triggers the read at once (about 0.1–0.3 s), falling back to the ordered path on a miss (rejected: ordered path only at ~1 s; no data). Other review fixes: seeding applies snapshots in stream-time order, with a negative cache and no-access duplicates; OperationEnd is failure-only with a stream-time confirm window; the key map uses CloseKey and parent links; value-read flags and checks; per-reason deadlines. |
| 2026-10-02 | Sub-project 1 spec revision 3 approved. The spike results and decisions are now part of the design sections, after an independent review and a verification pass. Next: plan 1b-1 (schema additions + `atlas-buffer`). |
| 2026-10-04 | Plan 1b-1 drafted; its code was verified in a scratch worktree before the plan was written, then independently reviewed (no blockers; 3 major and 10 minor findings folded in: an undeletable segment no longer stalls the writer; the writer starts a new segment at every open, so an ack ahead of a power cut cannot hide new records; Sensor Health gains a `buffer` group). **D1 A:** Sensor Health uses typed, grouped counters (loss, quality, housekeeping, resources, buffer, plus a gap) that count occurrences per report interval; absent means not measured (rejected: a name→number map; deferring both new classes to plan 1b-4). **D2 A:** Atlas's OCSF extension uid is 500, so Sensor Health is `class_uid` 50006001 (rejected: 999 "Development", shared with anyone; registering with OCSF now). Plan 1b-1 approved the same day; the build runs on a new branch after the plan merges. |
| 2026-10-04 | Plan 1b-1 built: the schema additions (File `Open`, registry flags, Event Log Activity, Sensor Health with typed per-interval counters under Atlas extension uid 500) and `atlas-buffer` (segment header `ATLSEG01`; `append` queues and `tick` does the I/O; a new segment at every open; four overflow policies, with undeletable segments skipped and counted; a reader waits on the newest segment and skips corrupt sealed ones). The fuzz workflow is now a matrix over (crate, target). The two symlink tests skip on the host (no symlink privilege) and are first exercised by CI's elevated Windows runner. |
| 2026-10-05 | Plan 1b-2 (`atlas-etw`) drafted. Its code was verified before the plan was written: unelevated in a scratch worktree, and in four elevated runs of the live test on the host, run by the user (the last passed all 45 checks). It was then independently reviewed: no blockers; 3 major and 10 minor findings folded in. A `Session` refuses to act once its LoggerId may have been reused; the version check reads the installed manifest or MOF class, never the forgeable event; replay fails instead of passing vacuously. **D1 A:** verify unelevated plus one elevated host script (rejected: unelevated only; verifying on the CI runner during planning). **D2 A:** replay fixtures are JSON lines filtered to the scenario at capture and recorded by CI's `etw-live` job (rejected: `.etl` filtered with `ITraceRelogger`; unfiltered `.etl`). Findings for plans 1b-3/1b-4: CloseKey fires only on the last handle close; the event-ID filter cannot be read back, but Kernel-EventTracing reports every enable change on our session with the caller's PID; UDP receive's `saddr` is the remote end; registry names are counted strings that may embed NULs (TDH truncates them, the parser does not). Plan 1b-2 approved the same day; the build runs on a new branch after the plan merges. |
| 2026-10-05 | Plan 1b-2 built: `atlas-etw` with portable hand-written parsers (counted registry names keep embedded NULs), a layout table checked against the installed manifests and MOF class, and the Windows session layer (all ETW `unsafe`: sessions that refuse to act on a stale LoggerId, a real-time consumer, the watchdog's primitives, a version check that reads only the installed description). Tests: replay of text fixtures recorded by CI's new `etw-live` job on the admin runner, a live test with an observer and an actor process, and the `parse_any` and `dns_query_results` fuzz targets. The spec carries the plan's 12 clarifications and findings F1–F9 (§15.3). |
| 2026-10-05 | Plan 1b-3a (`atlas-agent` pipeline core) drafted from code verified in a scratch worktree, independently reviewed (4 blocking, 7 major findings, all fixed with pinned tests that fail when the fix is reverted), and approved. **D1 A:** plan 1b-3 is split along the portable/Windows seam: 1b-3a is the portable pipeline with Windows behind the `Lookups` and `Request`/`Reply` interfaces, faked in tests; 1b-3b implements them (rejected: one plan of ~12–15k lines; a split by data type, which mixes portable and Windows code in both halves). Plan 1b becomes five plans. **D2 A:** one thread runs the ordering, pipeline and completion stages, with the clock passed in (rejected: a thread per stage; the option to split written into the spec). Measured at 0.57 µs per event steady state, 0.91 µs at 52,000 events/s of unique addresses. **D3 C:** the full-pipeline replay of the CI recording checks scenario assertions plus a protobuf-JSON golden snapshot (rejected: assertions only; snapshot only). **D4 A:** 8.3 short names are expanded in every emitted file path, Launch and Module images included (rejected: watchlist matching only, which leaves mixed spellings; adding a field with the logged short form, deferred). Notable clarifications: a reused or closed key base never names the old object's children; a snapshot carries the addresses it was asked, and a covered address absent from the table is answered at once; request bookkeeping is freed when the event leaves; seeding has separate start and miss settings; Sensor Health gains `loss.callback_panics` and `quality.reg_name_ambiguous`. |
| 2026-10-05 | Plan 1b-3a built: the `atlas-agent` library with intake (the callbacks' logic), one pipeline thread (ordering, state, completion), the process cache and Launch join, files, registry, network, DNS, seeding, value reads and 8.3 expansion, all portable, with Windows behind `Lookups` and `Request`/`Reply` and faked in tests. Sensor Health gains `loss.callback_panics` and `quality.reg_name_ambiguous`. Tests: 93 unit tests, and a full-pipeline replay of the CI recording with scenario assertions and a golden snapshot. The spec carries the plan's 26 clarifications and findings B1–B5 (§15.3). |
| 2026-10-06 | Plan 1b-3b brainstorm. **D1 A:** a minimal driver (threads wired to a test sink) and the agent-level live test (seeding, value reads, 8.3 + watchlist, undelete) come before plan 1b-4 (rejected: services only, with end-to-end waiting for 1b-4; the driver without its end-to-end test). **D3 A:** they form their own plan, **1b-3c**, after 1b-3b; plan 1b becomes six plans (rejected: one plan of ~10k lines built in two PRs; one plan and one PR). 1b-3b is the Windows services behind `Lookups` and `Request`/`Reply`, each with its own tests; 1b-4 keeps the buffer writer, watchdog and canaries, Sensor Health reports, service, CLI and the 24-hour run. **D2 A:** the seeder duplicates key handles with no access and names them with `NtQueryObject(ObjectNameInformation)` (rejected: `DUPLICATE_SAME_ACCESS` with `NtQueryKey`). Verified on the host, unelevated: `NtQueryKey(KeyNameInformation)` is denied on a no-access duplicate but works with any non-zero access; a duplicate cannot gain rights its source lacks, so the spec's `KEY_QUERY_VALUE` fallback is unworkable; file handles are named with no access (`GetFinalPathNameByHandleW`, `FileFsDeviceInformation`). **D4 A:** the 8.3 expansion cache, keyed by (parent directory, short name), is invalidated by the pipeline's existing `InvalidateHash` requests (Delete, Rename source, Update, SetAttributes; the path and everything under it), with a 60 s TTL as backstop (rejected: no cache, ~54 µs per short component; TTL only, which reports a reused short name under the old directory for up to a minute). Verified on the host: `GetLongPathNameW` rejects `\\?\GLOBALROOT\Device\…` paths (`ERROR_INVALID_NAME`) and costs 1.55 ms per 7-short-component path, so expansion is done per short component with `NtQueryDirectoryFile` on the parent directory's NT path (works for shadow copies too); deleting a directory frees its short name for the next one, and a rename regenerates it. **D5 A:** defaults: Windows code as a `cfg(windows)` module in `atlas-agent`; `account_name` on a helper thread with a 100 ms timeout so a slow domain lookup never stalls the pipeline thread; one `Services` object owning the hash workers, reader lane and seeder; bounded request lanes (a drop costs only the event's deadline); `sha2` for SHA-256; value reads fall back to a normal open without `SeBackupPrivilege`; seeder and identity tests run elevated in CI and once on the host (26200; CI is 26100). |
| 2026-10-06 | Plan 1b-3b (`atlas-agent` Windows services) drafted from code verified in a scratch worktree: unelevated, and in four elevated runs of its privileged tests on the host, run by the user (the last passed all 154). Independently reviewed: no blockers; 7 major and 15 minor findings, fixed with pinned tests that fail when the fix is reverted, or documented. Notable fixes: nothing is hashed from or into the cache while a writer has the file open (writes through an open handle keep the USN); 8.3 expansion gets its own lane so a stalled directory never delays value reads; a dropped invalidation clears the expansion cache; file duplicates are closed as soon as they are named and verified by the holder's handle; value reads take the first 4 KiB of a value of any size. Findings F1–F7 (`GetLongPathNameW` cannot expand NT paths; a handle can change between the table read and the duplicate; which name queries block; mixed-case kernel key names; PID 4 unopenable unelevated; a dead counter; an upper-case `\DEVICE\` bug). Sensor Health gains `housekeeping.service_queue_drops`; CI gains the `agent-live` job. Approved by the user with D1–D5 as brainstormed. |
