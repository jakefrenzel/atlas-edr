# Sub-project 1: ETW Sensor (Brainstorm Notes)

**Status:** Superseded by the approved spec [2026-10-01-etw-sensor-design.md](2026-10-01-etw-sensor-design.md) (rev 2, approved 2026-10-01). Kept as the brainstorm record. **This is not the spec.** It is the handoff between sessions.
The next session resumes the brainstorm at **"Next steps"** below and then writes the real spec
(`docs/specs/<date>-etw-sensor-design.md`) from these notes.

**Process position** (per CLAUDE.md: brainstorm → spec → plan → build): clarifying questions in progress.

---

## Context

- Started while 0b's VM build is on hold for disk space. ETW consumption is user-mode, so the sensor is developed and
  run on the host; only attack simulation (Atomic Red Team) needs the VM.
- **Carried in from 0a** (0a spec §4.4, §9), to verify (on the host where possible):
  - Whether ETW Kernel-Process ProcessStart `ProcessSequenceNumber` equals `PsGetProcessStartKey`.
  - A stable `boot_time` source for `boot_id`.
  - The requester PID in the header of DNS-Client event 3008.
- **Owned here per 0a spec §1:** which events the sensor emits, hashing/signature caching, file-read volume policy.

## Decisions made in this brainstorm

| # | Decision | Why |
|---|---|---|
| E1 | **Scope = agent core:** `atlas-agent` with the ETW sensor, process cache/enrichment, on-disk buffer, a dev CLI that dumps the buffer as JSON, and a thin Windows-service wrapper (console mode for dev). No transport. | Self-contained and testable on a real machine; the roadmap already assigns the buffer to 1; sub-project 2 then adds only enrollment and gRPC. The buffer read API is designed around an "ack up to cursor" model now but not wired to a consumer. |

| E2 | **Own thin `atlas-etw` crate on the official `windows` crate; hand-written parsers.** Session plumbing (`StartTrace`/`EnableTraceEx2`/`OpenTrace`/`ProcessTrace`) is the only `unsafe` in the sensor. Parsers are pure `&[u8]` functions per provider/event ID/version, unit- and property-tested and fuzzed on Linux CI. A replay test of recorded `.etl` files cross-checks our parsers against TDH. | Learning goal; small auditable attack surface in a SYSTEM service parsing attacker-influenced data; no TDH per-event cost; full control of enable properties and filters. Rejected: `ferrisetw` (last release 1.2.0, 2024-06; 2025 fixes unreleased; hides the mechanics), our plumbing + TDH parsing (slower, Windows-only, not fuzzable on Linux). |
| E3 | **Manifest providers only, in one agent-owned real-time session:** Microsoft-Windows-Kernel-Process, -Kernel-File, -Kernel-Registry, -Kernel-Network, -DNS-Client. | `EnableTraceEx2` applies `EVENT_ENABLE_PROPERTY_PROCESS_START_KEY` (actor uid for free, per 0a §4.4) and kernel-side keyword + event-ID filtering (low overhead, goal 1); versioned manifests make the TDH cross-check possible. Rejected: classic NT Kernel Logger (all-or-nothing category flags → far more volume; different enable path; start-key property unclear). Known cost: some fields need practical verification (e.g. handle-relative Kernel-Registry key names). |

| E4 | **File policy: changes only, coalesced, plus a sensitive-path watchlist.** Create only when the disposition actually created/overwrote; Update once per handle (first write, finalized at handle close); Delete; Rename; SetAttributes limited to timestamp/attribute changes (timestomping). Read events filtered out in the kernel; opens of watchlist paths (browser credential stores, hive copies, `.ssh` keys, configurable) become events. | Matches what Sigma file rules use; daily-machine overhead. Covers credential-file access without Read volume. **Spec decides:** map watchlist opens to `Read` (with "open ≠ read" caveat) or add OCSF `Open` additively to the schema. |
| E5 | **Hash (SHA-256) + Authenticode for Process Launch and Module Load images.** Cache keyed by volume + file ID + size + last-write time; worker pool, never on the ETW callback thread; launch events wait a short deadline (~500 ms, spec sets it) then emit without enrichment; size cap skips huge files. | High detection value (unsigned-in-System32, hash IOCs, sideloaded DLLs); module loads are mostly cache hits. Known gap: self-deleting binaries may be unhashable. |
| E6 | **Buffer = own append-only segment log.** ~16 MB numbered segments; records `[len][CRC32C][proto]`; single writer thread, batched flush (~1 s loss window on power cut); recovery truncates the tail segment at the first bad CRC; persisted `(segment, offset)` ack cursor, acked segments deleted whole; overflow drops whole segments with a counter + gap marker. Replay goes through 0a `TryFrom`. | Queue workload → log structure: no compaction, cheap deletes, pure Rust, testable/fuzzable cross-platform. Rejected: `redb`, SQLite (table-as-queue overhead; SQLite adds a big C dependency). **Spec decides:** overflow policy (drop oldest vs. newest), configurable with a stated default. |
| E7 | **Tests in three CI tiers:** (1) pure tests on Linux (parsers, cache, coalescing, buffer incl. crash recovery, fuzz); (2) Windows replay of committed `.etl` fixtures (recorded on the host) through the real consumer, incl. the TDH cross-check; (3) live integration tests on `windows-latest` (admin): real session, known actions, assert normalized events. Live tests `#[ignore]` locally, run explicitly in CI. | Each tier catches a different bug class (logic / version drift / enable flags + filters); none needs the VM. |

| E8 | **Network:** TCP Connect/Accept → Open (outbound/inbound), Disconnect → Close; byte counts only if Disconnect carries totals (verify), never per-packet events. UDP: flows synthesized in the agent from datagram events, one Open per (process, local, remote) and Close after ~60 s idle. **Gate:** the plan measures UDP event rate + agent CPU under heavy QUIC streaming on the host; over budget → UDP falls back to "none" (DNS only) with a decision-log entry. | UDP C2/exfil and resolver-bypassing DNS are real; flows give rules a clean shape; the gate protects goal 1. Rejected: TCP only (blind to UDP C2), per-packet TCP for byte counts (every packet through ETW; exfil volume is better done server-side later). |
| E9 | **Sensor health in sub-project 1:** watchdog checks every few seconds that the session exists, each provider is still enabled, and the lost-event/lost-buffer counters; restarts a stopped session; periodic **canary** action (e.g. set a dedicated registry value) must produce its event (the only exception to own-PID filtering); every anomaly is written to the buffer as a health record. | Silent loss makes an EDR untrustworthy; blinding is a standard intrusion step and its timestamp is a top-value signal; a local log is what an attacker deletes. Tamper *resistance* stays in sub-project 8. **Spec decides:** health records as an additive schema class (OCSF has candidates) vs. a separate buffer record type. |

| E10 | **Revises E3: a second, minimal system-logger session** (`EVENT_TRACE_SYSTEM_LOGGER_MODE`) enabling only System Process Provider `SYSTEM_PROCESS_KW_GENERAL` (legacy `EVENT_TRACE_FLAG_PROCESS` on builds older than 20348). Its classic Process Start carries `CommandLine` + `UserSID`; the pipeline joins it with Kernel-Process ProcessStart by PID + timestamp inside the reorder window. Watchdog covers both sessions. | Kernel-Process ProcessStart has **no command line** (confirmed: manifest dump; Microsoft Q&A 1331639), and `cmd_line` is required + the most-used Sigma field. Rejected: reading the PEB at Launch (races short-lived `cmd /c` processes), the classic logger for everything (loses E3's filtering/start-key reasons). Cost: second consumer thread, the join, one scarce system-logger slot. |
| E11 | **Unresolvable actors: never drop for lack of context.** Lookup = cache entry live at the event's timestamp (incl. 30 s post-exit retention); built-in identities for PID 0/4 and minimal processes; ProcessStop payload on a miss. If the uid is still computable (start key known), emit with empty path/name and count; drop + count only when no uid can be computed (network events for a never-seen PID). | Decided after the spec review (2026-10-01). Incomplete telemetry beats missing telemetry; dropping creates a blind spot an attacker can aim at. Rejected: drop all unresolved (blind spot), `<unknown>` placeholder (fake data that looks real). |

### Manifest facts (jdu2600/Windows10EtwEvents dump, Windows 11 26H1, checked 2026-10-01)

- **Kernel-Process:** ProcessStart (1) v3+ has `ProcessSequenceNumber`, `ParentProcessSequenceNumber`, `MandatoryLabel` (SID → `integrity`), elevation fields; v5 is current. ProcessStop (2) v2 has `ExitCode` + sequence number. ImageLoad (5) has `ImageBase`, `ProcessID` (payload), `ImageName`.
- **Kernel-File:** `CreateNewFile` (30, keyword `CREATE_NEW_FILE`) fires only for new files → E4 Create needs no disposition parsing. `DeletePath` (26) and `RenamePath` (27) carry `FilePath` directly. `Create` (12) carries `FileName` + `FileObject`; `Write` (16), `SetInformation` (17), `Cleanup` (13), `Close` (14) carry only `FileObject`/`FileKey` → need the map. `OperationEnd` (24) carries the NTSTATUS, correlated by `Irp`; the pre-op events don't say whether the operation succeeded.
- **Kernel-Registry:** every op carries `Status` (failures filterable in user mode). `CreateKey` (1) has `BaseObject` + `RelativeName` (handle-relative names). `SetValueKey` (5) has `Type`, `DataSize`, `CapturedData` + `PreviousData`. **No RenameKey event exists**; key rename may surface as `SetInformationKey` (11) or not at all. Hive export (41, `SourceKeyPath`, e.g. `reg save HKLM\SAM`) exists: valuable, no schema class yet.
- **Kernel-Network:** TCP and UDP events carry **`PID` in the payload** (these events often fire in arbitrary/system context, so the header PID and start key are not the owner). TCP: 12/28 connect, 15/31 accept, 13/29 disconnect (has a `size` field, meaning unknown), 17 connect-failed. UDP: 42/58 send, 43/59 recv.
- **DNS-Client:** 3008 = `QueryName, QueryType, QueryOptions, QueryStatus, QueryResults` and **no ClientPID**. Other events on the same query path (3009–3020, v1+) carry an explicit `ClientPID`, which is a fallback if the 3008 header PID turns out to be wrong.

### To verify (plan spikes, on the host)

- From 0a: `ProcessSequenceNumber` vs start key; `boot_time` source; DNS 3008 header PID.
- Kernel-Registry SetValueKey: manifests list `CapturedDataSize`/`CapturedData` (+ `PreviousData*`), but sample events show size 0. Find out when data is captured; if never, the spec picks between a post-event read (racy, flagged) and no data.
- Kernel-Network Disconnect: does its `size` field carry byte totals (E8)?
- Kernel-File: does `CreateNewFile` fire only on success? How do `DeletePath` / `RenamePath` relate to `OperationEnd` status (is a failed delete reported)? What exactly is in `RenamePath.FilePath` (old or new name)? Overwrite-of-existing (`FILE_OVERWRITE_IF` / `SUPERSEDE`) is not "new": does it need `Create` disposition parsing after all?
- Kernel-Registry: does key rename produce any event? Is `CapturedData` populated by default?
- Classic Process Start (E10): fields/version on current builds; confirm the PID + timestamp join is unambiguous.

### Design notes (no fork; goes straight into the spec)

- **Process cache:** seeded from a running-process snapshot at startup, maintained from Launch/Terminate; keyed by start key (never PID); misses resolved live via `ProcessTelemetryIdInformation` (start key + image path + user SID in one call), discarded if the start key doesn't match; entries kept for a grace period after Terminate because ETW delivery can be slightly out of order.

## Section 1: Components & data flow (approved 2026-10-01; **corrected after review**)

> **Correction (spec review, 2026-10-01):** the reorder window below sat *after* the stateful pipeline, so the
> pipeline still saw arrival order (cache/map misses, lost Updates). The spec (rev 2, §3.2) moves ordering to an
> **ordering stage before the pipeline** (750 ms hold) and adds a **completion stage after it** (enrichment + Launch
> join, ordered output). The text below is kept as the original record.

Crates: `atlas-etw` (`parse` portable, no `unsafe`, fuzzed; `session` `#[cfg(windows)]`, all the `unsafe`),
`atlas-buffer` (portable segment log), `atlas-agent` (Windows binary: pipeline, cache, enrichment, watchdog,
service wrapper, CLI `run` / `dump` / `service install|uninstall`).

Threads: [1] consumer thread per ETW session (`ProcessTrace` callback parses → bounded queue; never blocks, drops +
counts when full) → [2] single pipeline thread owning all mutable state (process cache, file-object→path map, Update
coalescer, UDP flow table, own-PID filter with canary exception) → [3] 2 low-priority enrichment workers (E5) →
[4] **reorder window** (~1 s, configurable): events held sorted by timestamp, enrichment lands here, released
oldest-first → (empty agent-side detection hook for sub-project 3) → [5] buffer writer. [6] watchdog (E9).

- The reorder window fixes cross-CPU out-of-order ETW delivery (Launch always precedes the process's other events)
  and replaces E5's separate 500 ms deadline. Cost: ~1 s latency, a few MB of memory. Prevention (sub-project 7)
  comes from the driver path, not this one.
- **Refinement agreed at approval:** ETW real-time delivery can lag by up to the session flush timer (default 1 s).
  Set a sub-second flush timer (`EVENT_TRACE_USE_MS_FLUSH_TIMER`), keep the window comfortably above it, and emit
  late arrivals out of order with a counter. Never drop them.

## Section 2: Provider → schema mapping (approved 2026-10-01)

Session A = manifest providers; Session B = system logger (E10). Event-ID filters drop everything unlisted in the kernel.

| Schema event | Source (event ID) | Mapping rules |
|---|---|---|
| Process Launch | Kernel-Process 1 (v3+) ⨝ classic Process Start (Session B) | `uid` from `ProcessSequenceNumber` if verified, else `ProcessTelemetryIdInformation`; `parent_process` from `ParentProcessID` + parent sequence number; `integrity` from `MandatoryLabel`; `cmd_line` + `user` from the classic event; `actor` = header (creator; keeps the PPID-spoofing signal); hashes/signature per E5. |
| Process Terminate | Kernel-Process 2 | `exit_code` from `ExitCode`. |
| Module Load | Kernel-Process 5 | `actor` = payload `ProcessID`; `base_address` = `ImageBase`; hashes/signature per E5. |
| File Create | Kernel-File 30 `CreateNewFile` | New files only; overwrites per spike. |
| File Update | Kernel-File 16 `Write` | Coalesced per `FileObject`, finalized at 13 `Cleanup`; path and actor from the 12 `Create` that opened the handle. |
| File Delete / Rename | Kernel-File 26 / 27 | Paths from `FilePath`; Rename's other name from the `FileObject` map. |
| File SetAttributes | Kernel-File 17 | `InfoClass` = `FileBasicInformation` only. |
| Watchlist access (E4) | Kernel-File 12 `Create` | Path matched against the watchlist (12 is needed anyway for the map). |
| Registry Key Create / Delete | Kernel-Registry 1 (`Disposition` = created new) / 3 | Path from `BaseName` + `RelativeName`. |
| Registry Value Set / Delete | Kernel-Registry 5 / 6 | `data` from `CapturedData`, truncated to 4 KiB with the flag set. |
| Registry Key Rename | none found | Documented v1 gap unless the spike finds a source. |
| Network Open / Close | Kernel-Network 12/28, 15/31 → Open; 13/29 → Close; UDP 42/43/58/59 → flows (E8) | `actor` = payload `PID` via the cache. |
| DNS Response | DNS-Client 3008 | `actor` = header PID if verified, else join with a sibling event's `ClientPID` (3009–3020); `rcode` mapped from `QueryStatus`, raw kept in `platform_status`; `answers` parsed from `QueryResults`. |

Cross-cutting rules:
1. **Actor attribution:** synchronous events (process, image, file create/delete/rename/setinfo, registry, DNS) use the header
   PID + start key; network uses the payload PID; file writes are attributed to the handle opener (the cache manager
   issues many writes later).
2. **Failed operations are dropped:** registry via `Status`; Kernel-File per spike (add `OperationEnd` 24 joined on
   `Irp` if pre-op events can report failures).
3. **Path normalization:** NT device paths → drive paths via a device map (built at startup, refreshed on volume
   change; unmappable paths stay NT paths); `\REGISTRY\MACHINE\…` → `HKLM\…`, `\REGISTRY\USER\<SID>\…` → `HKU\<SID>\…` (Sigma forms).
4. **Documented gaps:** key rename (maybe), hive export (event 41; no schema class yet; later additive class), DNS
   that bypasses the Windows client (from 0a).

## Section 3: Buffer details, health records, config, service/CLI (approved 2026-10-01)

Resolves the "spec decides" items from E4, E6, E9 (OCSF facts checked on schema.ocsf.io 1.9.0):

1. **Watchlist hits = File System Activity `Open` (14)** ("a request to create a file handle"), not `Read`.
   Additive change to `atlas.events.v1`, made in this sub-project's plan.
2. **Health records, both additive schema changes** (so they flow through buffer, server and detection):
   **Event Log Activity (1008)**: `Stop` (session stopped), `Restart` (we restarted it), `Disable` (provider disabled
   or canary silent); plus an **Atlas extension class "Sensor Health"** (the reserved `atlas` namespace) for loss
   accounting: periodic counters (ETW events/buffers lost, queue drops, late arrivals) and buffer gap markers.
3. **Overflow default = keep head + tail, drop the middle:** pin the oldest ~25% of unacked data, drop the oldest
   segments after it, write a gap marker. Drop-oldest lets a flood erase the initial compromise; drop-newest blinds the
   sensor from the flood onward. Configurable; default cap 1 GiB.

Also:
- `C:\ProgramData\Atlas\` (buffer, `agent.toml`, `state`, logs), ACL SYSTEM + Administrators only. `state` holds
  `device.uid` (generated on first run) and the ack cursor.
- `agent.toml`: buffer cap, reorder window, watchlist, enrichment size cap, per-class on/off, UDP on/off (E8 gate);
  built-in defaults, file overrides; validated at start (refuse to start on a bad config); no hot reload in v1.
- CLI: `run` (console, Ctrl+C); `service install|uninstall|start|stop` (LocalSystem, auto-start, recovery = restart;
  `windows-service` crate); `dump [--follow] [--from <cursor>] [--class <name>]` (JSON lines, read-only, never acks).
- Lifecycle: fixed session names `Atlas-Sensor`, `Atlas-Process`; a leftover session with our name is stopped and
  recreated at start; clean stop = flush sessions → drain reorder window → flush buffer → stop sessions.
- Self-filtering: drop events whose actor is the agent (by start key), including its own `tracing` rolling-file
  log writes; canary excepted.

## Next steps

Clarifying questions done (E1–E9). Present the design in sections for approval:

1. **Section 1: components & data flow: APPROVED 2026-10-01** (see "Section 1" below). E3 revised by E10.
2. **Section 2: provider → schema mapping: APPROVED 2026-10-01** (see "Section 2" below).
3. **Section 3: buffer, health records, config, service/CLI: APPROVED 2026-10-01** (see "Section 3" above).
4. **Section 4: testing, verification spikes, performance budget, DoD: APPROVED 2026-10-01.** Content is in the spec (§12–§14).
5. **Spec written:** [2026-10-01-etw-sensor-design.md](2026-10-01-etw-sensor-design.md). Rev 1 had an independent review (2 blockers,
   10 major, ~11 minor); the user decided E11; all findings folded into **rev 2** (spec §17). **Rev 2 approved 2026-10-01.**
6. **Plan structure decided (2026-10-01):** two plans, each reviewed and approved before it runs:
   - **Plan 1a** (`docs/plans/<date>-etw-sensor-spikes-plan.md`): spikes S1–S10 as throwaway code in a git-ignored `spikes/` folder; results go into spec §15.3 and the decision log. S1/S2 need reboots and clock changes, so they prefer the VM if it is ready and fall back to the host.
   - **Plan 1b**: the build, with full code, written from the spike results.
7. **Next:** write plan 1a.
