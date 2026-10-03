# Sub-project 1 — ETW Sensor Design (Agent Core)

**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in four parts, each reviewed and approved before it runs (decision log, 2026-10-02). Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
**Depends on:** 0a (event schema: domain types, `process_uid`, validating `TryFrom`), 0b (CI, nightly fuzz workflow).
**Depended on by:** sub-project 2 (adds enrollment + gRPC on top of the buffer's read API), sub-project 3 (fills the agent-side detection hook), sub-project 6 (the driver becomes a second sensor feeding the same pipeline).

---

## 1. Purpose & Scope

Build the **agent core**: a Rust user-mode agent that collects Windows telemetry through ETW, normalizes it into 0a domain events, enriches it, and stores it durably on disk until a transport (sub-project 2) ships it.

**In scope**
- `atlas-etw` crate: ETW session plumbing (Windows) and hand-written event parsers (portable).
- `atlas-buffer` crate: the on-disk segment log.
- `atlas-agent` binary: pipeline, process cache, enrichment, health watchdog, Windows-service wrapper, CLI.
- Additive schema changes to `atlas.events.v1` (§10).
- Verification spikes for the items 0a handed over and the new ones found in this brainstorm and its review (§15).
- Tests in three CI tiers, new fuzz targets, an operations runbook.

**Out of scope** (owned elsewhere)
- Transport, enrollment, mTLS, server → sub-project 2. The buffer's read API is designed for it (§8.5) but has no consumer yet.
- Detection logic → sub-project 3. The pipeline has an empty hook where it plugs in (§3.2).
- Tamper *resistance* (protecting the agent and its sessions) → sub-project 8. This sub-project *detects* blinding (§9).
- Boot-time coverage (an AutoLogger session that starts before the agent) → sub-project 8 (§16).
- Attack-simulation validation (Atomic Red Team) → first task once the `edr-test` VM exists (§14.2).
- Final performance budgets → sub-project 8. This spec sets provisional ones (§13).

**Development location:** ETW consumption is user-mode, so the agent is developed and run on the host. CLAUDE.md's VM-only rule applies to kernel driver code only.

## 2. Key Decisions (from brainstorm)

| # | Decision | Rationale |
|---|---|---|
| E1 | Scope = agent core (sensor, enrichment, buffer, dev CLI, service wrapper); no transport | Self-contained and runnable on a real machine; sub-project 2 adds only enrollment and gRPC. |
| E2 | Own thin `atlas-etw` crate on the official `windows` crate; hand-written parsers; TDH used only as an oracle | Learning goal; small auditable `unsafe` surface in a SYSTEM service; parsers fuzzable on Linux; no TDH per-event cost. `ferrisetw` rejected (last release 2024-06, hides the mechanics). |
| E3 | Manifest `Microsoft-Windows-Kernel-*` providers + DNS-Client in one real-time session | `EnableTraceEx2` gives the start-key enable property and event-ID filtering. |
| E10 | Revises E3: a second, minimal system-logger session for process events only | Kernel-Process ProcessStart has no command line; the classic Process Start event does (plus the user SID). |
| E4 | File: changes only, coalesced, plus a sensitive-path watchlist | Matches what rules use; Read volume avoided; credential-file access still visible. |
| E5 | SHA-256 + Authenticode for launched images and loaded modules, cached by file identity | High detection value; module loads are mostly cache hits. |
| E6 | Buffer = own append-only segment log | Queue workload; no compaction; cheap deletes; pure Rust, fuzzable. |
| E7 | Tests in three CI tiers: pure (Linux), `.etl` replay (Windows), live sessions (Windows) | Each tier catches a different class of bug; none needs the VM. |
| E8 | Network: TCP Open/Close; UDP flows synthesized in the agent, behind a measured CPU gate | UDP C2 is real; per-packet events never enabled. The gate passed (S9): UDP is on by default. |
| E9 | Sensor health monitoring in this sub-project: watchdog, canary, health records in the buffer | Blinding is a standard intrusion step; silent loss makes an EDR untrustworthy. |
| E11 | Never drop an event for lack of context: if the actor's uid is computable, emit with what is known (empty path/name) and count it; drop and count only when no uid can be computed. Carve-out (rev 3): a file write whose handle never resolves to a path is counted, not emitted (§7.1) | Incomplete telemetry beats missing telemetry; dropping creates a blind spot an attacker can aim at (very short-lived processes, activity right after a blinding). |
| E12 | Registry value data is read by the agent after the SetValueKey event and flagged as read after (§7.5). A fast path in Session A's callback reads within about 0.1–0.3 s; otherwise the ordered path reads about 1 s after the write | Kernel-Registry never fills `CapturedData` (S4), and value data is what registry detections key on. Racy by design; type and length checks catch most stale reads; the driver (sub-project 6) later supplies exact data. Re-decided after the rev 3 review showed the ordered path alone reads about 1 s late. Rejected: ordered path only; no data. |
| E13 | The `FileObject` and `KeyObject` maps are seeded from the system handle table at start and on a miss, with snapshots applied in stream-time order and a negative cache for handles that cannot be named (§7.4); unresolvable registry paths are emitted with a `path_unresolved` flag | Neither provider has a rundown, and their names are relative to handles (S6, S7). The handle table's object addresses equal the ETW object fields (verified). Rejected: keeping it a documented gap; dropping (contradicts E11). |
| E14 | A process-launch canary joins the registry and file canaries (§9.2) | It costs 0.082% of one core with Defender on (S8), within the 0.1% threshold. |

## 3. Architecture

### 3.1 Crates

| Crate | Platform | Contents |
|---|---|---|
| `atlas-etw` | `parse` portable; `session` `#[cfg(windows)]` | **`parse`**: pure functions `&[u8]` (+ provider, event ID, version, per-event pointer size) → typed `RawEvent`. No `unsafe`. **`session`**: start, enable, consume and stop sessions; extended-data extraction (start key). All of the sensor's `unsafe` lives here, behind a safe API. |
| `atlas-buffer` | portable | Segment log (§8). No Windows or ETW knowledge; stores opaque records. |
| `atlas-agent` | binary; Windows functionality behind `#[cfg(windows)]` | Ordering stage, pipeline, process cache, coalescers, flow table, registry key map, enrichment and value reads, completion stage, handle-table seeder, watchdog, config, service wrapper, CLI. Compiles on Linux (all portable modules tested there); on non-Windows `main` exits with "unsupported platform". |

### 3.2 Threads and data flow

```
ETW (Session A: manifest providers; Session B: system logger, process events)
   │
[1] one consumer thread per session (ProcessTrace callback)
      parse → RawEvent (+ header: pid, tid, raw QPC timestamp, start key)
      Session A only: OperationEnd (24) forwarded only when its status is a failure;
        early registry key map + fast value-read trigger (§7.5) ──────────┐
      → kernel queue (Kernel-* + Session B)    ┐ bounded, never block;     │
      → user-mode queue (DNS-Client), per-PID  ┘ full ⇒ drop + count       │
        rate limit (§4.4)                                                   │
   │                                                                        │
[2] ordering stage: merges all queues; holds each event until               │
      now − event time ≥ hold (default 750 ms); releases in timestamp order │
      late arrivals (older than the last released) pass through at once, counted
   │                                                                        │
[3] pipeline thread: owns ALL mutable state, sees events in time order      │
      process cache · FileObject map · KeyObject map · Update coalescer     │
      UDP flow table · Launch join · failure-confirm window (12/26/27)      │
      actor resolution · canary matching · path normalization               │
      then, at emission: self-filter on the resolved actor                  │
      → domain Events (some pending: see "Pending events")                  │
   │              ▲ seeder replies         [4] workers (2, below-normal) ◄──┘
   │              │                            SHA-256 + Authenticode (§6.3)
   │              │                            + 1 reader lane: registry value reads (§7.5),
   │              │                              8.3 name expansion (§7.2)
   │         [8] seeder (below-normal): names key and file handles from the
   │             system handle table at start and on a miss (§7.4)
[5] completion stage: emits in order; a pending event holds the line until
      completed, cancelled, or past its deadline, then goes as-is
   │
      agent-side detection hook (empty; sub-project 3)
   │
[6] buffer writer thread → segment log (§8)

[7] watchdog thread: session/provider checks, canaries, counters, restart (§9)
```

**Order before state.** The stateful pipeline [3] must see events in timestamp order. Otherwise an ImageLoad that arrives before its ProcessStart misses the cache, and a Cleanup that arrives before its Write loses the Update. Stage [2] provides that order. Real-time ETW delivers per-CPU buffers at the session flush timer (250 ms, §4.1), so an event older than flush timer + scheduling slack has almost certainly arrived. The 750 ms hold is three flush periods. A late event (one older than what [2] has already released) is processed immediately, out of order, and counted. It is never dropped.

**Stream time.** [3]'s clock is the ordering watermark: max(timestamp of the last event [2] released, now − hold). It therefore keeps advancing when no events arrive. Every wait that depends on other events (the failure-confirm window, §5.5; when seeder snapshots apply, §7.4) is measured in stream time, so it is unaffected by how late events arrive. Waits for workers and the seeder are measured in wall time from the moment the event enters [5].

**Single owner of state.** One pipeline thread removes locking and its race conditions; per-event work is a few hash-map operations. Sharding by process is the escape hatch if profiling ever demands it. Two deliberate exceptions, both caches that never feed emitted fields directly: Session A's callback keeps an early registry key map for the fast value read (§7.5), and the seeder holds its last snapshot (§7.4). The seeder's answers reach the maps only through [3].

**Pending events.** [3] hands an event to [5] with an id and the reasons it is incomplete. Each reason has a deadline:
- enrichment, 1 s (§6.3);
- the Launch join, 1 s (§5.2);
- a registry value read, 1 s (§7.5);
- a 8.3 expansion, 1 s (§7.2);
- waiting for the seeder, 2 s (5 s during the first 30 s after start, while the initial pass runs, §7.4).

[4] reports its results to [5] by event id. Seeder replies go to [3], which owns the maps; [3] then completes the waiting events in [5] by id. [3] can also cancel a pending event, which [5] then drops: for example a delete whose operation failed. An event is emitted when all its reasons are resolved or past their deadlines, except that a reason marked "drop at deadline" drops the event when it expires unresolved (§7.1). [5] is bounded: if more than 100 000 events are pending, the oldest goes out as is and `pending_overflow` counts it. End-to-end latency is about hold + time to complete: 1–2 s, at worst 3 s when the seeder is involved. Prevention (sub-project 7) uses the driver path, not this one.

**Seeding.** Handles opened before the sessions started are never named by ETW (§7.4). The seeder [8] names them from the system handle table. At start the sessions start first and the seeder runs after, so no handle falls between the snapshot and the first event. Each snapshot is stamped with its QPC time, and [3] applies it in stream-time order (§7.4).

**Backpressure.** The ETW callback must stay fast: a slow consumer makes ETW drop events in the kernel. The callback parses, enqueues and keeps the small early registry map (a hash-map operation per CreateKey, OpenKey or CloseKey). Successful OperationEnds, about half of Kernel-File's volume, are discarded right there: they cost parse time but never reach [2]. Queues never block (kernel queue default 65,536 entries; user-mode queue 8,192); drops are counted per queue. User-mode providers get their own queue so a process flooding forged DNS-Client events (§4.4) cannot push kernel events out.

### 3.3 Time base

- **Both sessions use raw QPC timestamps** (`ClientContext = 1`). Ordering ([2]) and all internal comparisons use raw QPC, which is monotonic and unaffected by clock changes.
- **Conversion to `meta.time`** (Unix ns, UTC) is done by the agent: `time = anchor_unix_ns + (qpc − anchor_qpc) × 10⁹ / qpc_frequency`. The anchor pair (QPC, `GetSystemTimePreciseAsFileTime`) is taken at start and re-taken every 60 s; each event uses the newest anchor. A wall-clock change therefore shows up in `meta.time` within 60 s and never reorders events.
- **Synthesized events get a defined time:** File Update = the time of the `Cleanup`; UDP Close = the time of the last datagram in the flow; Network Close (TCP) = the disconnect event; health and Event Log Activity records = QPC at generation, converted the same way.

## 4. ETW Sessions

### 4.1 Session settings

| Setting | Session A `Atlas-Sensor` | Session B `Atlas-Process` |
|---|---|---|
| Mode | Real-time | Real-time + `EVENT_TRACE_SYSTEM_LOGGER_MODE` |
| Clock | `ClientContext = 1` (QPC) | `ClientContext = 1` (QPC) |
| Flush timer | 250 ms (`EVENT_TRACE_USE_MS_FLUSH_TIMER`) | 250 ms |
| Buffers | 64 KB each; min = 2 × CPU count; max = max(256, 4 × CPU count). Confirmed by S9/S10: no loss at about 10 000 events/s average and 52 000/s peaks on a 22-CPU host | 64 KB; min 4, max 16 |
| Enabled | Providers in §4.2, each with `EVENT_ENABLE_PROPERTY_PROCESS_START_KEY` | `EnableFlags = EVENT_TRACE_FLAG_PROCESS` only |

**Session B uses the legacy flag on every Windows build.** System-logger sessions with `EnableFlags` work from Windows 8 onward, so one code path covers Windows 10 22H2 (build 19045) and Windows 11, and that path is the one CI tests. The newer System Process Provider route (`EnableTraceEx2`, builds ≥ 20348) is not used. Process events are low-volume, so the lack of event-ID filtering here costs little. At session start the kernel emits process rundown events (`DCStart`) for every running process, including its command line; the cache uses them (§6.2).

At start, a leftover session with our name (from a crash) is stopped and recreated. Session names are fixed so the watchdog and the blinding test can address them. The watchdog records each session's `LoggerId` at creation (§9.1).

### 4.2 Providers and filters (Session A)

Each provider is enabled with the listed keywords and an `EVENT_FILTER_TYPE_EVENT_ID` allow-list. **The event-ID filter is evaluated when each event is written**, so events that are filtered out still cost CPU in the kernel I/O path, though they never reach user mode. S10 measured that cost (§13).

| Provider | Keywords | Event IDs allowed |
|---|---|---|
| Microsoft-Windows-Kernel-Process | `WINEVENT_KEYWORD_PROCESS`, `WINEVENT_KEYWORD_IMAGE` | 1 ProcessStart, 2 ProcessStop, 5 ImageLoad |
| Microsoft-Windows-Kernel-File | `FILEIO` 0x20, `OP_END` 0x40, `CREATE` 0x80, `WRITE` 0x200, `DELETE_PATH` 0x400, `RENAME_SETLINK_PATH` 0x800, `CREATE_NEW_FILE` 0x1000 (mask `0x1EE0`) | 12 Create, 13 Cleanup, 14 Close, 16 Write, 17 SetInformation, 24 OperationEnd, 26 DeletePath, 27 RenamePath, 30 CreateNewFile |
| Microsoft-Windows-Kernel-Registry | `CloseKey` 0x1, `SetValueKey` 0x100, `DeleteValueKey` 0x200, `CreateKey` 0x1000, `OpenKey` 0x2000, `DeleteKey` 0x4000 (mask `0x7301`) | 1 CreateKey, 2 OpenKey, 3 DeleteKey, 5 SetValueKey, 6 DeleteValueKey, 13 CloseKey |
| Microsoft-Windows-Kernel-Network | `IPV4` 0x10, `IPV6` 0x20 (mask `0x30`) | TCP 12/28 connect, 13/29 disconnect, 15/31 accept; UDP 42/43/58/59 (`network.udp`, on by default, §7.3) |
| Microsoft-Windows-DNS-Client | the Operational channel keyword `0x8000000000000000`, the only one that delivers 3008 (S3) | 3008 |

`FILEIO` is needed because Cleanup, Close and SetInformation sit under it. `OP_END` adds one OperationEnd (24) per operation of the enabled keywords, roughly doubling Kernel-File's kernel-side volume. Session A's callback forwards only those with a failure status and discards the rest on arrival (§3.2), so the extra cost stops at parsing. `OpenKey` is the largest single source (S9: about 5 600/s average, 32 000/s peaks), and `CloseKey` adds about 1 100/s (S7). The key map needs both (§7.4). S10 measured the set without `OP_END` and `CloseKey`, so DoD item 4 re-checks the §13 budget with them.

### 4.3 Parsers

- One parser per (provider, event ID, version) that we consume. Pointer-sized fields take the pointer size from **each event's** header flags (32-bit processes log 32-bit pointers from user-mode providers). Kernel providers and Session B log with the kernel's pointer size even for WOW64 processes (S8). The field layouts found by the spikes are in §15.3.
- **Unknown higher versions** of a known event: the first time a (provider, ID, version) is seen, the agent calls `TdhGetEventInformation` once and checks that our newest known layout is a strict prefix of it (same field names and types, in order; ETW manifests append fields). The verdict is cached. A prefix → parse with our layout; otherwise → count `unknown_version` and drop. TDH is never used per event.
- Strings: kept as raw UTF-16 slices until an event is emitted (most `Create` events are only matched and mapped, never emitted). At emit, converted lossily (U+FFFD for unpaired surrogates, 0a §6.1) and truncated to the schema limits with the matching `*_truncated` flag.
- Parsers never trust lengths in the payload: every read is bounds-checked, and a malformed event yields an error that is counted, never a panic.
- **TDH as oracle:** a Windows test decodes the `.etl` fixtures with TDH and compares field-by-field with our parsers (§12.2). The same comparison runs locally against live events on the host, so the host's build is checked without committing host recordings.

### 4.4 Trust in user-mode providers

DNS-Client logs from user mode. Any process can register its provider GUID and write forged events, and a process can patch its own `EtwEventWrite` to suppress real ones. Consequences:
- DNS events go to their own queue with a per-PID token-bucket limit (default 100 events/s per PID; excess dropped and counted per PID). A forger cannot starve kernel events.
- A forged event's header PID is the forger's own, and attribution is by header PID and start key (S3), so a forger can only claim lookups for itself.

## 5. Provider → Schema Mapping

### 5.1 Mapping table

| Schema event | Source (event ID) | Mapping rules |
|---|---|---|
| Process Launch | Kernel-Process 1 (v3+) ⨝ Session B classic Process Start | §5.2. |
| Process Terminate | Kernel-Process 2 | `process` from the cache entry live at the event time; on a miss, built from the event's own `ProcessID`, sequence number and `ImageName`. `exit_code` from `ExitCode`. |
| Module Load | Kernel-Process 5 | `actor` = payload `ProcessID` (§5.3); `module.file` from `ImageName`; `base_address` = `ImageBase`; hashes/signature per §6.3. |
| File Create | Kernel-File 30 `CreateNewFile` | New files only; 30 fires only when the create succeeded (S6). Overwriting an existing file has no 30: it is a truncation and maps to File Update (§7.1). |
| File Update | Kernel-File 16 `Write`; 17 `SetInformation` with `InfoClass` 19 (end of file) | Coalesced per handle (§7.1). A truncation on a mapped handle (the overwrite case) counts as a write. |
| File Delete | Kernel-File 26 `DeletePath`; 12 `Create` with `FILE_DELETE_ON_CLOSE` | `file.path` from `FilePath`, emitted unless a failed OperationEnd arrives within the confirm window (§5.5). A handle created with `FILE_DELETE_ON_CLOSE` (`CreateOptions` bit `0x1000`) produces no 26; Delete is emitted at its Cleanup (§7.1). |
| File Rename | Kernel-File 27 `RenamePath` | `file` (the original) from the `FileObject` map; `file_result` from `FilePath`, which is the new name (S6). Emitted unless a failed OperationEnd arrives within the confirm window (§5.5); a confirmed rename also updates the map entry (§7.1). |
| File SetAttributes | Kernel-File 17 `SetInformation` | Only `InfoClass` 4 = `FileBasicInformation` (timestamps and attributes; timestomping). |
| File Open (new, §10.1) | Kernel-File 12 `Create` | Only paths matching the watchlist (§7.2). |
| Registry Key Create | Kernel-Registry 1 | Only `Status` = 0 and `Disposition` = 1 (created new); a nested create logs one event per new level (S7). Path from the key map (§7.4). |
| Registry Key Delete | Kernel-Registry 3 | Only `Status` = 0. Path from the key map (§7.4). |
| Registry Key Rename | — | **v1 gap**: no event carries the new name (S7, §16). |
| Registry Value Set | Kernel-Registry 5 | Only `Status` = 0. `type` from `Type`; `data` read right after the event (§7.5), truncated to 4 KiB with `data_truncated`; `CapturedData` is never filled (S4). |
| Registry Value Delete | Kernel-Registry 6 | Only `Status` = 0. `key_path` from the key map (§7.4), `name` from `ValueName`. |
| Network Open / Close | Kernel-Network | §7.3. |
| DNS Response | DNS-Client 3008 | §5.4. |
| Event Log Activity, Sensor Health (new, §10.2–10.3) | watchdog, pipeline counters | §9. |

### 5.2 Process Launch and process identity

**One uid formula everywhere.** `process.uid` must come out the same whether a process appears as a Launch, an actor, a parent or a Terminate; otherwise events cannot be tied together. Spike S1 settled the start key (row 2 below, confirmed on builds 26200 and 26300): **start key = `(BootId << 48) | ProcessSequenceNumber`**, with BootId the kernel's value (§6.1). The table keeps the alternatives for the record:

| S1 outcome | Start key for a new process (Launch) | Start key for an actor (other events) |
|---|---|---|
| `ProcessSequenceNumber` = start key | `ProcessSequenceNumber` | extended data |
| start key = f(`BootId`, sequence number) (e.g. `(BootId << 48) \| seq`, as public research suggests) and S1 confirms f | f(`BootId`, `ProcessSequenceNumber`) | extended data |
| neither | **all paths switch to the 0a fallback formula** (`hash(pid, create time)`, domain tag `atlas.process.v1-fallback`), with create time from ProcessStart / the cache | the extended-data start key is then only a cache key |

Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a uid source: the process may already be gone (the same race E10 rejected). S1 compares `ProcessStartKey` and `ProcessSequenceNumber` from `PROCESS_TELEMETRY_ID_INFORMATION` for live processes, which tests the relationship directly.

**Fields:**
- **`parent_process`** from `ParentProcessID` + `ParentProcessSequenceNumber` (the *claimed* parent), resolved through the cache entry live at the event time; if the parent is unknown, a `ProcessRef` with the computed uid, the PID and empty path/name (E11).
- **`actor`** from the event header (the creator). It differs from the parent under PPID spoofing (0a §5.2).
- **`integrity`** from `MandatoryLabel`: S-1-16-0 Untrusted; -4096 Low; -8192 and -8448 Medium; -12288 High; -16384 System; -20480 and -28672 Protected; anything else → absent.
- **`cmd_line`, `user`** from Session B's classic Process Start v4 (`CommandLine`; `UserSID` → `user.uid`, with `user.name` from `LookupAccountSid`, cached). The SID follows a 16-byte `TOKEN_USER` header (kernel pointer, attributes, padding); offsets in §15.3 (S8). It is parsed from the raw bytes; TDH would render it as an account name.
- **Join:** the pipeline creates the cache entry from whichever half arrives first and marks the Launch pending. The halves match by PID with timestamps within 200 ms (a PID cannot be reused while its process is alive). S8 paired 201 of 201 launches, 1–56 µs apart. If the partner has not arrived by the completion deadline, the Launch is emitted with what exists and `launch_join_miss` increments. For a missing Session B half, the agent first tries `PROCESS_TELEMETRY_ID_INFORMATION` (its `CommandLineOffset`) while the process may still be running.
- Hashes and signature for `process.file` per §6.3.

### 5.3 Actor attribution

| Event kind | Actor source | Why |
|---|---|---|
| Process, image load, file create/delete/rename/setinfo/open, registry, DNS | Event header PID + start-key extended data | Logged synchronously in the caller's context (DNS confirmed by S3). |
| Network | Payload `PID` → cache | Logged in arbitrary or system context; the header is not the owner (S5: an accept was logged under the peer's header PID). |
| Module Load | Payload `ProcessID` → cache | The image is mapped into that process; the header is usually the same, but the payload is authoritative. |
| File Update; File Delete for a delete-on-close handle | The handle's opener from the `FileObject` map (event 12, or the owning process for a seeded handle, §7.4) | The cache manager issues many writes later from System, and the closing context can be the agent's own duplicate during seeding. The Cleanup header is never used. |

**Lookup rule:** "the cache entry for this PID that was live **at the event's timestamp**", which includes entries in their 30 s post-exit retention. Because [3] runs in time order, this is a straightforward interval check.

**Unresolvable actors (E11).**
1. Built-in identities: PID 0 (Idle), PID 4 (System), and the minimal processes (Secure System, Registry, Memory Compression) get synthetic `ProcessRef`s with their well-known names and a fixed empty path, seeded at start.
2. A miss with a known start key (any synchronous event): live lookup via `ProcessTelemetryIdInformation` (accepted only if its start key matches). If that fails, emit with the computed uid, the PID, and empty `file.path`/`file.name`; count `actor_unresolved` per class. Empty strings are valid under 0a.
3. A miss with no start key (network events for a PID with no live-at-time entry): uid cannot be computed → drop and count `actor_dropped` per class.

### 5.4 DNS Response

- `query.hostname` = `QueryName`; `query.type` = `QueryType`.
- `platform_status` = `QueryStatus` (always kept). `rcode` mapped for known codes: 0 and 9501 (no records) → NOERROR (0); 9001 → FORMERR (1); 9002 → SERVFAIL (2); 9003 → NXDOMAIN (3); 9004 → NOTIMP (4); 9005 → REFUSED (5); otherwise absent.
- `answers[]` parsed from `QueryResults`: `;`-separated entries, addresses in text form (IPv4 and IPv6), CNAMEs as `type: 5 <name>` before the addresses, empty on failure (S3). The parser is fuzzed; at most 64 entries per 0a.
- **Actor:** the event header PID and start key, which are the requesting process's, including for cache hits, failures and 32-bit callers (S3). The sibling events (3009–3020, logged by the DNS Client service) are not used.

### 5.5 Failures and normalization

- **Failed operations are dropped.**
  - Registry: keep only `Status` = 0. This also drops the `STATUS_REPARSE` (`0x104`) first attempt that precedes a `CurrentControlSet` open (S7).
  - Kernel-File: 30 fires only on success. Create (12), DeletePath (26) and RenamePath (27) fire before the outcome (S6). Session A's callback forwards only failed OperationEnds: 24s whose `Status` is not `NT_SUCCESS` (error-severity codes). Success and informational codes such as `STATUS_REPARSE` (0x104) and `STATUS_OPLOCK_BREAK_IN_PROGRESS` (0x108) are not failures. A failure is the exception, so success is the default.
  - [3] holds each 12, 26 and 27 for a **failure-confirm window** of stream time: default 250 ms after its timestamp, configurable. By then its OperationEnd, logged within microseconds of the operation, has almost surely passed the ordering stage too.
  - A failed 24 within the window cancels the event: dropped, `file_op_failed` counted, and for a 12 its map entry removed. It is matched to the most recent pending event with the same `Irp` and an earlier timestamp. Irps are recycled, so the window keeps the match local.
  - No failure by the end of the window means the event stands; it is emitted, or for a 12 its entry stays. A short ring of recent failed 24s (also covering the window) catches a failure that arrives before its event.
  - The window bounds the table; pending entries expire by stream time.
  - A slow operation that completes after the window is treated as successful. A failure it later reports is counted as `file_op_late_failure`, not reversed (§16).
- **File paths:** NT device paths (`\Device\HarddiskVolume3\…`) → drive paths (`C:\…`) via a device map from `QueryDosDeviceW`, built at start and refreshed every 60 s and on a lookup miss (at most once per 5 s). Prefixes match only on a path-component boundary (`HarddiskVolume1` never matches `HarddiskVolume10\…`). Unmappable paths (shadow copies, network redirectors, unmounted volumes) stay as NT paths.
- **Registry paths:** come from the key map (§7.4); `KeyName` and `BaseName` are always empty (S7). Then: `\REGISTRY\MACHINE\…` → `HKLM\…`; `\REGISTRY\USER\<SID>\…` → `HKU\<SID>\…`; and `HKLM\SYSTEM\ControlSet00N\…` → `HKLM\SYSTEM\CurrentControlSet\…` when N is the current control set (from `HKLM\SYSTEM\Select\Current`, read at start). These are the forms Sigma uses. WOW64 views stay as `…\WOW6432Node\…` (the kernel logs the real path).
- **Self-filtering** happens at emission, after [3] has updated its maps, joins and canary matching. Otherwise the agent's own CreateKey/OpenKey would be missing from the key map, and the registry canary could not be named.
  - Events whose resolved actor is the agent (by start key) are dropped, including the agent's own buffer and log writes.
  - So are events from the launch canary's child (its start key is taken from its Launch).
  - The test uses the resolved actor, never a Cleanup's header (§7.1).
  - Canary events are consumed by the watchdog, never emitted (§9.2). Activity done on the agent's behalf by other processes (e.g. CryptSvc catalog lookups during signature checks) cannot be self-filtered and is documented (§16).

## 6. Identity and Enrichment

### 6.1 Device and boot identity

- `device.uid`: random UUID generated on first run, persisted in `C:\ProgramData\Atlas\device.json`, written once.
- `device.boot_id`: per 0a §4.4, `BLAKE3("atlas.boot.v1" ‖ BootId ‖ boot_time)[0..16]`.
  - **`BootId`** = `KUSER_SHARED_DATA.BootId` (user-mode address `0x7FFE0000 + 0x2c4`; the offset is from public symbols on builds 26200 and 26300). At start the agent checks it against its own `PROCESS_TELEMETRY_ID_INFORMATION.BootId`, the same kernel value. On a mismatch it uses the telemetry value and logs the disagreement. The registry's `PrefetchParameters\BootId` is never used: it lagged the kernel value by one in the VM (S2).
  - **`boot_time`** = the creation time of the System process (PID 4), from `GetProcessTimes` (FILETIME, u64 LE). It stayed identical across clock and time-zone changes, sleep and restarts, and changed on reboot (S2).

### 6.2 Process cache

- **Seeded** at start from Session B's process rundown (`DCStart`, opcode 3: PID, parent, command line, user SID; S8 saw 318 for 317 running processes) plus `ProcessTelemetryIdInformation` per PID (start key, create time, image path). Seeded processes are cached, not emitted as Launch events.
- **Maintained** from Launch and Terminate events.
- **Keyed by start key**, never by PID; a PID index holds each PID's entries with their live intervals.
- **Retention:** entries stay 30 s after Terminate (late events still resolve), then are removed.
- **Bounded:** a hard entry cap with an eviction counter, so lost Terminate events cannot grow memory without limit.

### 6.3 Hashes and signatures

- Applies to `process.file` on Launch and `module.file` on Module Load.
- **Cache key:** volume serial + 128-bit file ID (`FILE_ID_INFO`) + **the file's USN** (`FSCTL_READ_FILE_USN_DATA`). The USN changes on every modification and, unlike last-write time, cannot be set back by a user. On volumes without a USN journal, results are not cached. Entries for a path are also invalidated when the pipeline sees an Update, Rename, SetAttributes or Delete on it.
- **Work** runs on 2 below-normal-priority worker threads, never on an ETW or pipeline thread. Files are opened with full sharing (`FILE_SHARE_READ | WRITE | DELETE`) and read sequentially.
- **SHA-256** for files up to the size cap (default 100 MB); larger files skip hashing.
- **Signature:** `WinVerifyTrust` with `WTD_UI_NONE`, `WTD_REVOKE_NONE`, `WTD_CACHE_ONLY_URL_RETRIEVAL` (no network calls from the sensor), and catalog lookup (`CryptCATAdmin*`) for catalog-signed OS files. `signer` = the leaf certificate's subject CN. Mapping: valid chain → `Valid`; no signature (`TRUST_E_NOSIGNATURE`) → `Unsigned`; a signature that fails verification (bad digest, untrusted root, explicit distrust, revoked per local cache) → `Invalid`; any operational error (file locked, CryptSvc unavailable, timeout) → signature **absent**, counted.
- **Deadline:** the completion stage's (§3.2), 1 s. A late result is cached for the next event on the same file. S10 measured 38 first launches with no miss (p99 540 ms), so the deadline stays.
- **Known gaps:** a binary deleted right after launch may be unreadable. Hashing re-opens the file by path, so a binary renamed away and replaced in between is hashed as the replacement (§16).

## 7. Coalescing, Watchlist, Flows, Handle Maps

### 7.1 File handle map and Update coalescing

**The map.** `FileObject → entry`. An entry holds:
- the logged NT path;
- the opener's `ProcessRef`;
- `written` and `delete_on_close` flags;
- its source: an ETW Create, a seeder answer, or provisional.

**Entries from Create.** Every Create (12) adds an entry, and `delete_on_close` comes from `CreateOptions` bit `0x1000`. A failed Create is removed when its failure OperationEnd arrives within the confirm window (§5.5). A Create on a `FileObject` already in the map replaces the entry (address reuse after a lost Close) and counts `file_object_replaced`.

**Writes.** The first Write (16), or a SetInformation 17 with `InfoClass` 19 (a truncation, the overwrite case), marks the entry written. Further writes are absorbed.

**Unknown `FileObject`.** A Write, truncation, RenamePath (27) or SetInformation 17/4 on a `FileObject` with no entry creates a **provisional** entry: `written` is set for a write or truncation, the path and actor are unknown. It also asks the seeder (§7.4). The seeder's answer fills in the path and actor (the owning process). Cleanup is not a trigger: by then the last handle is closed and cannot be seeded.

**Cleanup (13), the last handle closed.**
- A written entry emits **one** File Update.
- A `delete_on_close` entry emits a File Delete.
- The actor of both is the entry's opener, never the Cleanup's header. The cache manager, or the agent's own duplicate during seeding (§7.4), can be the closing context.
- A File Update or Delete from a provisional entry waits in [5] with a "drop at deadline" seeder reason (§3.2): if the entry is still unresolved at the deadline, the event is dropped and `unknown_file_object` counts it. This is a deliberate carve-out from E11: a file event without a path, and usually without a trustworthy actor, has no detection value.

**Close and after.**
- Close (14) removes the entry.
- Writes that arrive after Cleanup (lazy-writer flushes, memory-mapped paging writes) are absorbed into the Update already emitted and counted as `writes_after_cleanup`.
- A confirmed rename (§5.5) changes the entry's path to the new name, so a later Update carries the name the file has now.

**Bounds.** The map is bounded (cap + LRU eviction + counter). Seeded entries do not know `delete_on_close`, so a handle opened with that flag before the agent started produces no Delete (§16).

### 7.2 Watchlist

- Patterns are **volume-relative** globs (`\Windows\System32\config\SAM`, `\Users\*\AppData\Local\Google\Chrome\User Data\*\Login Data`), case-insensitive, compiled once into a single `GlobSet`. They are matched against the path with its volume prefix removed, so they also match shadow copies (`\Device\HarddiskVolumeShadowCopyN\…`), the classic route for stealing SAM and NTDS.
- Alternate data streams are stripped before matching (`file.txt:stream`, `file::$DATA` match `file.txt` / `file`).
- **8.3 names.** A Create opened through a short name logs the short form (S6), for example `…\ATLASS~1\…\LONG-F~1.TXT`.
  - A path with any component matching the 8.3 pattern (`^[^.~]{1,6}~[0-9]+(\.[^.]{0,3})?$`, case-insensitive) is expanded by a worker [4]. The Open is pending meanwhile (deadline §3.2).
  - The expansion is `GetLongPathNameW` on `\\?\GLOBALROOT` + the NT path, which also works for shadow copies, and is cached per directory.
  - The logged path is matched at once. An Open that matches neither form is dropped only after the expansion.
  - The emitted `file.path` is the expanded form when expansion succeeded, otherwise the logged one.
- Built-in default list (replaceable or extendable in config): Chromium `Login Data`, `Cookies`, `Local State` and Firefox `logins.json`, `key4.db` under `\Users\*\…`; `SAM`, `SECURITY`, `SYSTEM` hives and `*.sav`/`*.bak` copies under `\Windows\System32\config\`; `\Windows\NTDS\ntds.dit`; `\Users\*\.ssh\*`; `\Users\*\.aws\credentials`, `\Users\*\.azure\*`, `\Users\*\AppData\Roaming\gcloud\*`; `*.kdbx`.
- A successful Create on a matching path emits File System Activity **Open** (§10.1). It waits for the failure-confirm window, because a failed open logs a Create too (§5.5). An open is not proof of a read; rules should treat it as access intent.
- Repeated opens of the same path by the same process are coalesced to one per 60 s.

### 7.3 Network

- **TCP:** connect (12/28) → Open outbound; accept (15/31) → Open inbound; disconnect (13/29) → Close. `bytes_in`/`bytes_out` stay absent: the Disconnect `size` field is not a byte total (S5). Per-packet events are never enabled.
- **UDP flows:** datagram events (42/43/58/59) feed a flow table keyed by (actor uid, local endpoint, remote endpoint). The first datagram emits Open (direction from send vs. receive); a Close is emitted after 60 s idle (configurable), timestamped at the last datagram. The table is bounded (cap + eviction counter; evicted flows emit Close).
- **Performance gate (E8):** passed. Under real QUIC streaming (about 2 900 UDP events/s, peaks near 11 000/s) the full provider set cost 0.495% of one core with nothing lost (S9). `network.udp = true` is the default.
- Ports and addresses are decoded in network byte order (confirmed by fixtures).

### 7.4 Registry key map and handle-table seeding (E13)

**Key map.** Kernel-Registry names keys relative to handles, and its handle identity (`KeyObject`) is per handle, not per key (S7). The pipeline keeps `KeyObject → entry`:
- A successful CreateKey (1) or OpenKey (2) with a `RelativeName` starting with `\REGISTRY\` stores the absolute name.
- Otherwise it stores `(BaseObject, RelativeName)` and resolves it through the base's entry. If the base is resolved, the full name is computed at once. If not, the entry joins the base's list of pending children and is resolved, recursively, when the base is.
- CloseKey (13) removes the entry. A closed base that still has unresolved children stays as a tombstone until they resolve or expire. This keeps the map to live handles; the cap and LRU eviction are only a safety net.
  - **CloseKey events whose actor is the agent are ignored**, in this map and in the early map (§7.5). The seeder closes its own duplicates, and value reads close their keys. If those closes removed entries, seeding would wipe out its own results. If an agent duplicate really was the last handle, the stale entry is harmless: the next CreateKey/OpenKey on that address overwrites it.
  - The spikes did not examine CloseKey. Plan 1b-2 verifies its event ID, version and layout, and whether it fires on every handle close or only the last. The rules above are correct either way.
- Events 3, 5 and 6 carry only `KeyObject` and take their path from the map.

**Seeding.** Neither Kernel-File nor Kernel-Registry has a rundown (S6, S7). Handles opened before the sessions started (by services and Explorer at boot, by every process after an agent or session restart) are therefore never named by ETW. The seeder [8] names them from the system handle table.

- **Privilege.** Object addresses appear in the table only with `SeDebugPrivilege` enabled. The agent enables it at start (LocalSystem holds it; `atlas-agent run` from an elevated admin shell does too). If it cannot, seeding is off and Sensor Health says so.
- **Enumerate.** `NtQuerySystemInformation(SystemExtendedHandleInformation)` lists every handle with its owning PID, type index and kernel object address. That address equals Kernel-Registry's `KeyObject`/`BaseObject` and Kernel-File's `FileObject` (verified, S6/S7). The Key and File type indices are learned from one handle of each type that the agent opens itself.
- **Name.** Each handle is duplicated into the agent with **no access rights** (plan 1b-3 verifies that both name queries work that way; otherwise the minimum, `KEY_QUERY_VALUE` / `FILE_READ_ATTRIBUTES`). It is named and closed at once. Nothing is read or written through it.
  - Key handles: `NtQueryKey(KeyNameInformation)`.
  - File handles: only disk files, and never network redirectors (`FileFsDeviceInformation`). Then `GetFinalPathNameByHandleW(VOLUME_NAME_NT)` on a helper thread with a 200 ms timeout.
  - A query that times out is cancelled (`CancelSynchronousIo`), and its (PID, handle) goes into the negative cache. At most 2 helpers may be stuck at a time. Past that, file seeding pauses and Sensor Health reports it.
  - A duplicate can briefly be a file's last handle. The final Cleanup or Close, and any delete-on-close, then happens in the agent's context. That is why actors come from the map entry, not the event header (§7.1).
- **Actor.** A seeded file entry's actor is the owning process, resolved through the cache at the snapshot time. A `FileObject` owned by several processes (inherited or duplicated handles) takes the owner with the lowest PID: an arbitrary but fixed rule.
- **Snapshot time.** Every snapshot carries the QPC time it was taken (T_snap). [3] applies a snapshot only once stream time reaches T_snap, and then by these rules:
  - A seeded name for address A answers a pending event at time t only if [3] saw no Create/OpenKey/CloseKey (for files: no Create or Close) for A between t and T_snap. Otherwise the event stays unresolved, because the address may have changed owner in between.
  - A seeded entry never overwrites an entry created by an ETW event newer than T_snap.
- **When.**
  - At start, after the sessions are live: keys first (about 140 ms on the host), then files (about 1.2 s).
  - Then on a miss: an unknown `KeyObject`/`BaseObject`, or a Write, truncation, rename or attribute change on an unknown `FileObject` (§7.1).
  - Only an address absent from the latest snapshot triggers a re-read. Misses are batched, the table is re-read at most once per second, and the seeder keeps to a CPU budget (default 1% of one core averaged over 60 s; beyond it, re-reads are deferred and counted).
- **Negative cache.** Addresses that are in the snapshot but cannot be named (protected processes, failed or timed-out queries) go into a negative cache. They stay there until a CreateKey, OpenKey or CloseKey (for files: a Create or Close) for that address is seen, or the owning process exits. Events on them are emitted unresolved at once, without waiting. Without this cache, writes through protected processes' handles would force constant re-reads (S6/S7 saw 955 key and 2 174 file handles in processes the agent cannot open).
- **Cost** (host, all processes): keys, 15 156 handles, 42 ms to read the table + 98 ms to name, 93% named; files, 10 494 handles, 3 830 disk files named in 1.18 s (3 timeouts).
- **Floor (E11).** A registry event whose key stays unresolved is emitted, not dropped, with `path_unresolved = true` (§10.4) and the longest name known: the chain of relative names below the first unresolved base, or empty if nothing is known. It counts `registry_unresolved`. Unresolved file entries follow §7.1.

### 7.5 Registry value reads (E12)

Kernel-Registry never fills `CapturedData` (S4). The agent reads the value itself, as soon after the write as it can.

**The fast path.** Session A's callback [1] keeps an **early key map**: the same construction as §7.4, but in arrival order, and without seeding. On a successful SetValueKey whose key it can name, it sends a read request straight to the reader lane of [4], a separate thread, so reads never wait behind hashing (hash p99 was 540 ms, S10). The early map is bounded (default 200 000 entries, LRU eviction, counted); an eviction only costs a fallback to the ordered path. The read then happens within ETW's delivery delay plus the read itself: about 0.1–0.3 s after the write, mostly the 250 ms buffer flush.

The early map is only a cache. Its names are never emitted, and the event's path always comes from [3]'s map. If the two disagree, the early read is discarded and redone on the ordered path, counted as `early_read_redone`.

**The ordered path.** When the early map misses, the read happens after [3] has resolved the path, about 1 s after the write, or more if seeding was needed. Misses happen when events arrive out of order across CPU buffers, or when the key was opened before the agent started.

**The read.**
- `NtOpenKeyEx` by the raw NT name as logged (`ControlSet00N`, not the normalized form), with `REG_OPTION_BACKUP_RESTORE` (the agent enables `SeBackupPrivilege`). This defeats a deny-SYSTEM DACL a user can put on their own key.
- `OBJ_OPENLINK`, so a key swapped for a symbolic link is not followed.
- `NtQueryValueKey` for the value name as counted in the event, so embedded NULs are kept.
- `\REGISTRY\A\…` (application hives) and `\REGISTRY\WC\…` (containers) cannot be opened this way; their reads fail and are flagged.

**Result and flags** (§10.4):
- `type` always comes from the event.
- `data` is the value read, up to 4 KiB. It is accepted only if its type equals the event's `Type` **and** its length equals the event's `DataSize`. That catches most stale reads, and a decoy value with an embedded-NUL name.
- `data_truncated` is set when `DataSize` > 4 KiB.
- `data_read_after = true` whenever a read was attempted.
- `data_unavailable = true` whenever no data was obtained: the key is unresolved, the read failed, or a check failed. `data` is then empty and `data_truncated` false. Counted as `value_read_failed`.
- Value types above `REG_QWORD` (11) are outside 0a's `type` range. Such events are emitted with `type` absent, the raw type in `raw_type` (§10.4), and `data_unavailable`, and counted as `reg_type_unusual`. Dropping them would let an attacker hide a value just by giving it an unusual type.

**Other notes.**
- The Value Set waits in [5] (deadline §3.2); a fast-path read has usually finished before the event arrives there.
- SetValueKey is rare (about 1–2/s on the host, S9/S10), so the cost is negligible.
- The agent's own key opens are self-filtered at emission (§5.5). QueryValueKey is not enabled, so the reads log nothing.
- The read is racy by design: a value changed or deleted within the window (0.1–0.3 s on the fast path, about 1 s on the ordered path) is reported in its new state, which the length and type checks often catch, or as unavailable (§16). The driver (sub-project 6) can capture the exact data synchronously.

## 8. Buffer (`atlas-buffer`)

### 8.1 Layout

- Directory `C:\ProgramData\Atlas\buffer\` of numbered segment files, ~16 MiB each.
- Record: `[u32 LE length][u32 LE CRC32C of payload][payload]`. Payload = an encoded `atlas.events.v1.Event`; every record is an Event (gap information travels as Sensor Health events, §9.3).
- One writer thread. Writes are appended and flushed (`FlushFileBuffers`) in batches every 1 s: a power cut loses at most ~1 s. Rotation flushes and closes the old segment before creating the next.

### 8.2 Recovery

- At open, the newest segment is scanned; it is truncated at the first record whose length is invalid (0 or > 256 KiB, the 0a encoded-event limit; checked **before** allocating) or whose CRC fails. Older segments were sealed by a clean rotation and are not rescanned.
- Replay decodes every record through 0a's validating `TryFrom`; an invalid record is skipped and counted, never trusted.

### 8.3 Disk full and I/O errors

The writer stops appending, keeps events in a bounded in-memory backlog, retries every 10 s, and counts dropped events once the backlog is full. Recovery is reported in Sensor Health. The agent never deletes data it has not been configured to delete to make room.

### 8.4 Retention and overflow

- **Sub-project 1 (no transport, `transport = none`): rolling retention.** Nothing is ever acked, so the buffer is a local history: at the cap (default 1 GiB) the oldest whole segment is deleted, counted in `retention_evictions`. No gap events (this is expected behaviour, not loss).
- **Once a transport exists (sub-project 2): keep head + tail.** At the cap, the oldest ~25% of *unacked* data is pinned, the oldest segments after it are deleted, and a Sensor Health gap event records the time range and count lost. Drop-oldest lets a flood erase the initial compromise; drop-newest blinds the sensor from the flood onward. `drop_oldest` and `drop_newest` remain config options. This sub-project implements and tests the policy; sub-project 2 switches it on.

### 8.5 Read API (for sub-project 2)

- `reader.from(cursor)` iterates records after a `(segment, offset)` cursor.
- `ack(cursor)` persists the cursor atomically (`cursor` file: segment, offset, CRC; written to a temp file and renamed) and deletes segments wholly behind it.
- Delivery is at-least-once; the server de-duplicates on `event_id` (0a §4.1).
- **Concurrent readers** (`dump --follow`) open segments with `FILE_SHARE_DELETE`, treat an incomplete tail record as "not written yet" (wait, don't flag corruption), and skip ahead when a segment is deleted under them.

## 9. Sensor Health and Blinding Detection

### 9.1 Watchdog (every 5 s)

- **Session identity:** each session exists and has the `LoggerId` recorded at creation (`ControlTraceW` query). Missing, or an impostor session recreated under our name → Event Log Activity **Stop**, then the agent recreates it → **Restart**.
- **`ProcessTrace` returning** on either consumer thread also means Stop.
- **Provider state:** each provider in Session A is still enabled with our keywords, level and filters (`EnumerateTraceGuidsEx` / `TraceGuidQueryInfo`). Missing or changed → **Disable** (with the provider name), then re-enable.
- **Counters:** session events-lost and real-time-buffers-lost, plus all pipeline counters, sampled for Sensor Health.

### 9.2 Canaries

- **Registry:** every 60 s the agent sets `HKLM\SOFTWARE\Atlas\Canary` and expects the matching SetValueKey event within 10 s.
- **File:** every 60 s the agent creates and deletes `C:\ProgramData\Atlas\canary\<n>` and expects CreateNewFile + DeletePath within 10 s.
- **Process launch (E14):** every 60 s the agent starts itself as `atlas-agent.exe canary` (exits at once) and expects the child's Launch, both halves of the join (§5.2), within 10 s. The child is matched by its **creator's** start key (the event header), which PPID spoofing cannot fake, not by the claimed parent. All of the child's own events are self-filtered (§5.5). Cost with Defender on: 49 ms of CPU per launch, 0.082% of one core (S8).
- Matching is by path (or, for the launch canary, by creator start key and image) plus the agent's own start key as actor, never by value. A kernel provider event attributed to the agent cannot be forged by another process.
- A miss → Event Log Activity **Disable** for that provider. This catches a provider that silently stops delivering, which the session checks cannot see.
- Canary events are consumed by the watchdog and never emitted (no daily noise).
- Kernel-Network relies on the watchdog's provider checks.
- `HKLM\SOFTWARE\Atlas` and the canary directory carry the same SYSTEM + Administrators ACL as §11.1.

### 9.3 Sensor Health records

Emitted every 60 s (and immediately on a non-zero change in a loss counter):
- **Loss:** ETW events lost, real-time buffers lost, queue drops (per queue), DNS rate-limit drops, `actor_dropped`, buffer backlog drops, gap events (§8.4).
- **Quality:** late arrivals, parse errors, `unknown_version`, `launch_join_miss`, `actor_unresolved`, `unknown_file_object`, `registry_unresolved`, `value_read_failed`, `early_read_redone`, `reg_type_unusual`, `file_op_late_failure`, `writes_after_cleanup`, `file_object_replaced`, invalid records found on replay (§8.2), enrichment misses and errors.
- **Housekeeping:** cache, map and flow-table evictions, `retention_evictions`, `file_op_failed` (failed creates, deletes and renames dropped, §5.5), `pending_overflow` (§3.2), seeder results (handles named, failed, timed out; stuck helpers outstanding; negative-cache size; table reads; deferred re-reads; whether seeding is on).
- **Self-measurement:** agent CPU time and working set for the interval (`GetProcessTimes`, `GetProcessMemoryInfo`), so long runs measure themselves (§13).

## 10. Schema Additions (`atlas.events.v1`, additive)

All additions pass `buf breaking`, get golden fixtures, are documented in `docs/schema-reference.md`, and keep the 0a rules: no `Unknown`/`Other` activities emitted; unknown enums rejected. Two deliberate deviations from 0a are recorded in the decision log: classes without an actor (0a D5 says every event carries one), and an Atlas extension *class* (0a §5.9 reserved the `atlas` namespace for fields).

### 10.1 File System Activity: `Open` (14)

OCSF 1.9.0: "A request to create a file handle." Fields: `file`, `actor.process`.

### 10.2 Event Log Activity (OCSF class 1008, category 1)

| Activity | Meaning here |
|---|---|
| Stop (7) | An Atlas ETW session was found stopped or replaced. |
| Restart (8) | The agent recreated it. |
| Disable (10) | A provider was disabled or changed in our session, or its canary went silent. |

Fields: `log_name` (session name, required, ≤ 256 B), `log_provider` (provider name, required for Disable, ≤ 256 B), `status_code` (`u32`, optional: the Win32 error or status that revealed it). `actor.process` is **optional**: the watchdog sees the effect, not who caused it.

### 10.3 Sensor Health (Atlas extension class)

One activity, `Report`, carrying the §9.3 counters (`u64`, all optional), the interval, and for gap reports the lost time range and count. No actor. Category: Application Activity (6). Its `class_uid` follows OCSF's extension rule (`extension_uid × 100000 + category_uid × 1000 + n`, as Windows' `201001`), with an Atlas extension UID the plan picks from outside OCSF's registered extensions.

### 10.4 Registry fields (additive, from S4 and S7)

| Message | Field | Meaning |
|---|---|---|
| `RegistryValueSet` | `bool data_read_after = 4` | `data` was read from the registry after the event (§7.5), not captured with it. |
| `RegistryValueSet` | `bool data_unavailable = 5` | No data was obtained: the key is unresolved, the read failed, or the read's type or length did not match the event (§7.5). |
| `RegistryValueSet` | `optional uint32 raw_type = 6` | The value's type when it is above `REG_QWORD` (11); `type` is then absent (§7.5). |
| `RegistryKeyActivity` | `bool path_unresolved = 6` | `path` holds only what ETW logged (a relative name, or empty), not a full path (§7.4). |
| `RegistryValueActivity` | `bool path_unresolved = 6` | Same for `key_path`. |

These have no OCSF equivalent, so OCSF exports carry them under the reserved `atlas` field namespace (0a §5.9). Sigma rules see the flags and can exclude or qualify unresolved paths.

**Validation** (in `atlas-schema`'s `TryFrom`):
- `data_unavailable` requires empty `data` and `data_truncated` false.
- Exactly one of `type` (0–11) and `raw_type` (> 11) is present. This relaxes 0a's "absent `type` is Missing" rule only when `raw_type` is set; every message 0a accepts stays valid.
- `path_unresolved` allows any path, including empty.

Plan 1b-1 also updates the 0a documents: the 0a spec's status line and its §5.9 note that the `atlas` namespace is empty in v1. It also updates the OCSF exporter (`atlas-schema/src/ocsf.rs`) and `schema-reference.md`.

## 11. Configuration, Service, CLI

### 11.1 Install locations and permissions

- **Binary:** `service install` copies the executable to `C:\Program Files\Atlas\atlas-agent.exe` (writable only by administrators) and registers that path, quoted. It never registers the path it was run from (`target\`, `Downloads\`, …).
- **Data:** `C:\ProgramData\Atlas\` holds `buffer\`, `canary\`, `agent.toml`, `device.json`, `cursor`, and `logs\`. `C:\ProgramData` lets ordinary users create folders, so a non-admin could pre-create `Atlas\` with their own ACL or plant a junction, turning the agent's deletes and renames into SYSTEM-level primitives. Therefore:
  - `service install` creates the directory with owner Administrators and a **protected** DACL (no inheritance): SYSTEM and Administrators full control, nothing else.
  - **On every start** the agent verifies the owner, the protected DACL, and that neither the directory nor anything it opens inside is a reparse point (files opened with `FILE_FLAG_OPEN_REPARSE_POINT` and checked). On failure it refuses to start and reports to the Windows Event Log (its own log directory is untrusted at that point).

### 11.2 `agent.toml`

- Keys: buffer cap, retention/overflow policy, `transport` (`none` in this sub-project), ordering hold, completion deadline, enrichment size cap, watchlist (replace or extend the default), per-class enable, `network.udp`, UDP idle timeout, DNS per-PID rate, the failure-confirm window (§5.5), `file.op_end` (on by default), seeding on start and on miss, the seeder CPU budget (§7.4), `registry.value_reads` (on by default), log level.
- Built-in defaults; the file only overrides them. Validated at start: an invalid file stops startup with a clear error in the Windows Event Log (Application, source `Atlas`). No hot reload in v1.

### 11.3 CLI

| Command | Behaviour |
|---|---|
| `atlas-agent run` | Console mode (admin required); Ctrl+C triggers a clean stop. Refuses to start if another instance holds `Global\Atlas-Agent` (single-instance mutex), so it cannot tear down the service's sessions. |
| `atlas-agent service install \| uninstall \| start \| stop` | Install per §11.1; LocalSystem, auto-start. Recovery: restart after 5 s, then 60 s, then every 10 min, so a bad config does not loop hot. Uses the `windows-service` crate. |
| `atlas-agent dump [--follow] [--from <cursor>] [--class <name>]` | JSON lines from the buffer; read-only (never acks); §8.5 concurrency rules. |

### 11.4 Lifecycle

- **Start:** take the mutex → verify the data directory (§11.1) → load config → load `device.json` → compute `boot_id` → start sessions (recreating stale ones) → seed the process cache from the rundown → start threads → seed the key and file maps from the handle table (§7.4).
- **Clean stop:** flush both sessions → drain the ordering and completion stages → flush the buffer → stop the sessions. No in-flight event is lost on a clean stop.
- **Diagnostic log:** `tracing` with a rolling file in `logs\`; its writes are self-filtered (§5.5).

## 12. Testing

### 12.1 Tier 1 — pure, Linux CI

- Parser unit tests per event and version, built from fixture byte strings, for both pointer sizes where relevant.
- **Property tests:** the ordering stage releases in timestamp order and passes late events through; the completion stage preserves order; actor lookup-at-timestamp resolves correctly across PID reuse; the coalescer emits exactly one Update per written handle (truncations included) and a Delete at Cleanup for delete-on-close handles; a Create, DeletePath or RenamePath is dropped iff a failed OperationEnd for its `Irp` falls in its confirm window, with recycled `Irp`s, a failure arriving before its event, and late failures; the key map resolves names through any chain of relative opens, including bases resolved later by the seeder, removes entries on CloseKey and keeps tombstones while children are pending; seeder snapshots apply only by the stream-time rule (§7.4), never name an address that changed owner after the event, and never overwrite newer ETW entries; the negative cache stops re-reads; an early (fast-path) read that disagrees with [3]'s path is redone; every UDP Open gets exactly one Close; retention and head + tail overflow never delete the pinned head; buffer round-trip; recovery after truncation at **every** byte offset of a segment (simulated torn writes).
- **Fuzz targets** (each its own fuzz workspace under its crate, run 10 min nightly): `parse_any` (dispatches over every parser), `dns_query_results`, `buffer_recover` (arbitrary bytes as a segment file).
- **CI wiring** (part of the plan): `fuzz.yml` becomes a matrix over (fuzz workspace, target) with per-target corpus cache keys, crash artifacts, and PR path filters per crate; `ci.yml` runs `cargo check` on each fuzz workspace and `cargo audit` on each fuzz lockfile; `atlas-agent` builds on the Linux job.

### 12.2 Tier 2 — `.etl` replay, Windows CI

- Fixtures are recorded **on a GitHub Windows runner, not the host**: a `workflow_dispatch` job runs the scripted scenario on a clean, throwaway runner with file-mode sessions and uploads the `.etl` files. The repo is public, and a host recording would leak usernames, paths and DNS history. Fixtures are reviewed, then committed.
- Replay feeds the real consumer (`OpenTrace` on a file) through the full pipeline and compares against expected domain events.
- TDH oracle: every fixture event is also decoded with TDH and compared field-by-field with our parser (and locally against live host events, §4.3).

### 12.3 Tier 3 — live sessions, Windows CI

- Tests start real sessions on `windows-latest` (the runner is admin), run the scripted scenario, and assert the expected normalized events, filtered to the test's own process tree, with timeouts. The scenario: spawn a process with a known command line; create, write, overwrite, rename and delete a file; a failed delete (no event); a delete-on-close file; an undelete (disposition set, then cleared) and a delete-on-close cleared through `FileDispositionInformationEx`, which must produce no Delete (result documented either way); open a watchlisted path, also through its 8.3 name; create, set and delete a registry key and value (with the value data read after); a handle opened **before** the agent starts that is then written to (file) or created under (registry), named by seeding; open a TCP connection; send UDP; resolve a name.
- Marked `#[ignore]` locally; CI runs them explicitly.

### 12.4 Known gaps

- CI runs one Windows build (`windows-latest`, 26100). Because Session B uses one code path on all builds (§4.1), the main residual risk is manifest-version differences on older builds, which the runtime prefix check (§4.3) handles.
- Attack-technique validation needs the VM (§14.2).

## 13. Performance Budget (provisional)

Measured on the host. Sub-project 8 sets final budgets. CPU is the agent's own process CPU time (from its Sensor Health self-measurement, §9.3, cross-checked once with `typeperf`), expressed as a share of one core.

| Metric | Budget |
|---|---|
| Agent CPU, workday average | < 2% of one core |
| Agent CPU, heavy load (S9 streaming; S10 build + browsing) | < 5% of one core |
| Working set | < 150 MB |
| Lost events, normal use (ETW events lost + real-time buffers lost + queue drops) | 0 |

Late arrivals, enrichment misses and DNS rate-limit drops are reported, not gated.

**Measured by the spikes (S9, S10, §15.3).** With the full provider set except `OP_END`:
- Probe CPU (the probe parsed every event and ran a model of the ordering stage; it is not the agent): 0.39–0.50% of one core in S10 (build + browsing) and S9's QUIC confirmation; 1.04–1.20% in S9's two 10-minute runs at about 10 000 events/s. Working set 18–26 MB. Nothing lost.
- About 10 000 events/s on an ordinary desktop, peaks of 52 000/s.
- Builds about 3% slower (median) with the sessions on: accepted; sub-project 8 re-measures on an idle machine.
- Buffer growth ceiling: 150–235 MB per hour of build plus browsing. This is the sizing input for sub-project 2.

`OP_END`, `CloseKey`, seeding, the fast path and value reads were added after these measurements; DoD item 4 checks the budget with them.

**Fallbacks if over budget** (applied in order, each one a decision-log entry): on-miss seeding off (start-up seeding stays); then `file.op_end` off (Create, Delete and Rename are emitted without failure confirmation, counted); then UDP off; then File Update and SetAttributes off (drops the `FILEIO` and `WRITE` keywords, the largest kernel-side cost; Create/Delete/Rename/Open remain); then registry value reads off (`data_unavailable` on every Value Set); then per-class disable in config as a last resort.

## 14. Definition of Done

### 14.1 Done means

1. Every spike in §15.2 is resolved and written into §15.3, with fallbacks in the decision log (done 2026-10-02).
2. CI runs tiers 1–3 green on every push; the new fuzz targets run nightly.
3. §10 schema additions are in `atlas.events.v1`, pass `buf breaking`, and have golden fixtures and `schema-reference.md` entries.
4. **24-hour host run as a service** with the full §4.2 set (including `OP_END`), seeding and value reads: no crashes, within the §13 budget (from the agent's own Sensor Health records), zero lost events as defined in §13. A `dump` shows every class the tier-3 scenario produces, plus Sensor Health records.
5. **Blinding test:** `logman stop Atlas-Sensor` produces Stop then Restart within 10 s; a test helper that disables one provider in our session (`EnableTraceEx2` with `EVENT_CONTROL_CODE_DISABLE_PROVIDER`) produces Disable within 10 s; an impostor session recreated under our name is detected.
6. **Crash-safety test:** killing the agent mid-write recovers cleanly and replay finds no invalid records. (Torn writes from power loss are covered by the tier-1 truncation property test, not by this test.)
7. Runbook `docs/runbooks/atlas-agent.md` (install, config, dump, uninstall, troubleshooting); decision log and roadmap updated.

### 14.2 Not in DoD

Atomic Red Team validation. It is the first task once the `edr-test` VM exists, and it pairs naturally with sub-project 3's detections.

## 15. Verification Notes

### 15.1 Checked 2026-10-01

Against the jdu2600/Windows10EtwEvents manifest dump (Windows 11 26H1, build 28000), Microsoft Learn and schema.ocsf.io.

| Claim | Result |
|---|---|
| Kernel-Process ProcessStart carries a command line | **No** (manifest v0–v5; Microsoft Q&A 1331639) → E10. |
| ProcessStart v3+ | Has `ProcessSequenceNumber`, `ParentProcessSequenceNumber`, `MandatoryLabel`, elevation fields; v5 current. |
| System Providers via `EnableTraceEx2` | Windows 10 SDK 20348+, system-logger sessions only. Not used: Session B uses the legacy flag on all builds (§4.1). |
| Event-ID filtering | Evaluated on each event write (Microsoft Learn), so filtered events still cost kernel CPU. |
| Kernel-File new-file event | `CreateNewFile` (30) exists with its own keyword. `DeletePath` (26) / `RenamePath` (27) carry `FilePath`. Write/SetInformation/Cleanup/Close carry only `FileObject`/`FileKey`. `OperationEnd` (24) carries the status. |
| Kernel-Registry | Every op has `Status`; SetValueKey has `CapturedData` + `PreviousData`; **no RenameKey event**; hive export (41) exists. |
| Kernel-Network | Payload `PID` on TCP and UDP events; Disconnect has a `size` field of unknown meaning. |
| DNS-Client 3008 | No `ClientPID` in the payload; sibling events 3009–3020 (v1+) carry `ClientPID`. |
| OCSF 1.9.0 | File System Activity `Open` = 14; Event Log Activity = class 1008 with Stop 7, Restart 8, Disable 10. |
| `ferrisetw` | Last release 1.2.0 (2024-06-27); fixes from 2025-08 unreleased. |

### 15.2 Spikes (plan phase 0, on the host)

| # | Question | Fallback if it fails |
|---|---|---|
| S1 | Is `ProcessSequenceNumber` the start key, or is the start key f(`BootId`, sequence number)? (Compare `ProcessStartKey` and `ProcessSequenceNumber` from `PROCESS_TELEMETRY_ID_INFORMATION` and extended data, across processes and boots.) | All paths switch to the 0a fallback formula (§5.2). |
| S2 | Which `boot_time` source is stable for the boot and immune to clock changes? | (must find one; candidates compared in the spike) |
| S3 | Is the DNS 3008 header PID the requesting process? Which keyword is minimal? | Join with a sibling event's `ClientPID` (lower trust, §4.4). |
| S4 | Is `CapturedData` populated by default for SetValueKey? | Post-event read of the value, flagged as such, or no data. |
| S5 | Does Disconnect `size` carry byte totals? | Leave `bytes_in`/`bytes_out` absent. |
| S6 | Kernel-File: do 26/27/30 fire for failed operations? What is in `RenamePath.FilePath`? How are overwrites of existing files and delete-on-close seen? Does the `FILENAME` rundown map already-open handles? Are `Create` names ever 8.3 short names? | Enable `OperationEnd` and join on `Irp`; parse `Create` disposition for overwrites; document remaining gaps. |
| S7 | Kernel-Registry: are `KeyName` and `BaseName` + `RelativeName` full paths? Does any event reveal key rename? | `KeyObject → name` map from CreateKey/OpenKey; rename documented as a v1 gap. |
| S8 | Classic Process Start/DCStart: fields, version and `UserSID` layout on current builds (both pointer sizes); Launch join unambiguous; canary-launch cost. | Design adjusts in §5.2 / §9.2. |
| S9 | UDP event rate and agent CPU under heavy QUIC streaming (E8 gate). | `network.udp = false` by default. |
| S10 | Kernel-File and TCP event rates and agent CPU under a `cargo build` plus browsing; first-launch enrichment miss rate. | §13 fallbacks; raise the Launch completion deadline. |

### 15.3 Results

Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spikes-plan.md)). Host: Windows 11 build 26200. VM `edr-test`: build 26300 (Windows Update had moved it on from the 26100 recorded at the 0b build). Raw evidence stays in the git-ignored `spikes/results/`.

| # | Answer | Evidence | Design consequence |
|---|---|---|---|
| S1 | **Start key = `(BootId << 48) \| ProcessSequenceNumber`**, where BootId is the kernel's value (`KUSER_SHARED_DATA.BootId`). It is not the bare sequence number. | Host: `ProcessStartKey` matched the formula for 313 of 313 readable processes (none matched the bare sequence number). For 21 of 21 traced launches, Kernel-Process ProcessStart v4's `ProcessSequenceNumber` with the formula gave exactly the extended-data start key on that process's own events. VM: the formula held for 170 of 170 processes, then 96 of 96 after a reboot, with the high 16 bits stepping 6 → 7 with BootId. The one unreadable process is PID 0 (Idle). | §5.2 row 2: Launch start key = `(BootId << 48) \| ProcessSequenceNumber`; other events use the extended data, so all paths give the same uid. BootId is read from `KUSER_SHARED_DATA` (offset `0x2c4` on 26200 and 26300). The registry copy can lag (S2), so it is never used. BootId has 16 bits in the key. |
| S2 | **`boot_time` = creation time of the System process (PID 4)**, from `GetProcessTimes` (needs the SYSTEM or admin rights the agent has). | VM, 100 ns resolution: identical across +2 h and −1 day clock changes (time sync and w32time off), a time-zone change, a save/resume, and repeated separate reads; different after a reboot. Rejected: `SystemTimeOfDayInformation.BootTime`, WMI `LastBootUpTime`, and now − (unbiased) interrupt time all shifted by exactly each clock change (+7199.8 s, −79 200 s). The Registry and `smss.exe` creation times also passed, but need a lookup by name. Kernel-General event 12 `StartTime` also passed, but an attacker can clear the log. | §6.1: `boot_time` = PID 4 creation time (FILETIME, u64 LE in the 0a formula). One fixed-PID call, no event log. |
| S3 | **The 3008 header PID and start key are the requesting process. The minimal keyword is `0x8000000000000000` (the Operational channel).** | Host, `DnsQuery_W` from a test process: 6 of 6 lookups produced exactly one 3008 (v0) with the requester's header PID and extended-data start key: fresh, cached, cache-bypass, AAAA, a CNAME chain, and NXDOMAIN (`QueryStatus` 9003). Events 3009–3020 are logged by the DNS Client service with `ClientPID` = the requester. A WOW64 caller (`Resolve-DnsName` from 32-bit Windows PowerShell) was attributed the same way, with a 32-bit event header (3008 has no pointer fields, so the layout is unchanged). Keyword masks tried alone: only `0x8000000000000000` delivered 3008 (6 of 6); `0x100`, `0x4000000000000000`, `0x10000000`, `0x100000000` and `0x200000000` delivered none. `QueryResults` format: `addr;addr;` with IPv6 in text form; CNAMEs as `type: 5 <name>;` entries before the addresses; empty on NXDOMAIN. | §5.4: actor = header PID + start key; no sibling join, so the §16 note about `ClientPID` forgery does not apply. §4.2: DNS-Client keyword `0x8000000000000000`, event 3008 only. The `QueryResults` parser handles the `type: N <name>;` form (and is fuzzed). |
| S4 | **`CapturedData` is never populated.** | Host, all Kernel-Registry keywords: 11 of 11 SetValueKey (5, v0) events had `CapturedDataSize` = 0 and empty `PreviousData`, for REG_SZ, EXPAND_SZ, MULTI_SZ, DWORD, QWORD, BINARY 16 B and BINARY 8 KiB. `Type` and `DataSize` are correct. | **Decided 2026-10-02: read after the event.** Right after a successful SetValueKey whose key path is resolved, the agent reads the value (≤ 4 KiB, `data_truncated` as before) and marks it with an additive flag, `reg_value.data_read_after` (§10). The read is racy: a value deleted or changed before the read is missed or read in its new state (the window is set out in §7.5: about 0.1–0.3 s on the fast path, about 1 s otherwise). Cost is low (SetValueKey ≈ 2/s host-wide). The driver (sub-project 6) later supplies the exact data. |
| S5 | **Disconnect `size` does not carry byte totals.** | Host, loopback only (decided after a VM-side variant was dropped): a client sent exactly 1 000 000 bytes and received 3 000 000. Both disconnects (13, v0), one per side, had `size` = 0, and so did the connect (12) and accept (15). Header vs payload: the accept was logged under the client's header PID while its payload `PID` named the server. | §7.3: `bytes_in`/`bytes_out` stay absent (the spec's fallback). §5.3 confirmed: network events use the payload `PID`. Not tested over a real NIC; a zero on loopback rules out totals as the field's meaning. |
| S6 | **`CreateNewFile` fires only on success; `DeletePath` and `RenamePath` fire before the outcome; `RenamePath.FilePath` is the new name. Delete-on-close and overwrites have no dedicated event. `Create` names can be 8.3 short names. There is no rundown.** | Host, keywords `0x1EF0` (§4.2 set + FILENAME + OP_END), no ID filter, 0 events lost. **Failures:** a `CREATE_NEW` on an existing file or in a missing directory gave `Create` (12) then `OperationEnd` (24, same `Irp`, `0xC0000035` / `0xC000003A`) and no 30. A delete of a read-only file gave 26 then 24 with `0xC0000121`. A rename onto an existing name gave 27 then 24 with `0xC0000035`. A delete refused for sharing failed at the open (12 + 24 `0xC0000043`, no 26). **Rename:** 27's `FilePath` is the target name; NameDelete (11) old / NameCreate (10) new follow a successful rename. **Overwrite:** `CREATE_ALWAYS` / `TRUNCATE_EXISTING` on an existing file give 12 (disposition open / open-if) plus `SetInformation` (17) with `InfoClass` 19 (end of file), no 30. **Delete:** `DeleteFileW` → 26 (`InfoClass` 64); disposition delete → 26 (13); POSIX delete → 26 (64); `FILE_FLAG_DELETE_ON_CLOSE` → no 26, only `CreateOptions` bit `0x1000` on 12. **Timestomp / attributes:** 17 with `InfoClass` 4. **Writes:** one 16 per write call on the handle's `FileObject`, then Cleanup (13), Close (14). **Names:** opening through a short name logs `…\ATLASS~1\…\LONG-F~1.TXT`; streams log as written (`h.txt:secret`, `h.txt::$DATA`). **Rundown:** capture-state on FILENAME produced nothing; a handle opened before the session was never named (only its 16/22/13 appeared). | §4.2: add `OperationEnd` (24, keyword OP_END `0x40`); §5.5: drop 12/26/27 when a failed 24 for their `Irp` arrives within a stream-time confirm window (refined in rev 3, §5.5); 30 needs no join. §5.1 Rename: new name from 27, old name from the `FileObject` map. §5.1 Create: overwrite = 17 with class 19 on a mapped handle, emitted as File Update. §5.1 Delete: also emit Delete at Cleanup for a handle created with `FILE_DELETE_ON_CLOSE`. §7.2: expand 8.3 components before watchlist matching (`GetLongPathNameW`, §7.2). §7.1: **handles opened before the agent: decided 2026-10-02, seed the `FileObject` map from the handle table** (verified the same day: the handle table's object address equals Kernel-File's `FileObject`; a write on a handle opened before any trace was named `…\held.txt` from the snapshot, while ETW never saw its Create). Only disk files are named (`GetFileType` = disk, then `GetFinalPathNameByHandleW`), from a worker thread with a timeout, because some handle types can block a name query. Host cost: 10 494 file handles; 3 830 disk files named in 1.18 s (3 timeouts at 200 ms); 4 477 non-disk; 2 174 in processes that cannot be opened. The opener (actor) of a seeded handle is the owning process. Writes on handles that stay unnamed are still counted `unknown_file_object`. |
| S7 | **Names are handle-relative, and the handle identity is per handle, not per key. No event reveals a key rename. There is no rundown.** | Host. `KeyName` (3, 5, 6) and `BaseName` (1, 2) are always empty. `RelativeName` (1, 2) is relative to `BaseObject` unless it starts with `\REGISTRY\`. Replaying a `KeyObject → name` map built from successful CreateKey/OpenKey resolved 30 of 30 successful writes to full paths. `KeyObject` is per handle: three processes opening `\REGISTRY\MACHINE` got three different values. `RegRenameKey` produced only OpenKey, QueryKey and CloseKey; the new name appears nowhere. `EVENT_CONTROL_CODE_CAPTURE_STATE` produced no rundown (counts per ID unchanged within noise). Failures carry `Status` (`0xC0000034` not found). Opening `CurrentControlSet` first returns `0x104` (`STATUS_REPARSE`), then succeeds as `\REGISTRY\MACHINE\SYSTEM\ControlSet001\…`. WOW64 views appear as `…\WOW6432Node\…`. CreateKey `Disposition`: 1 = created, 2 = opened. `RegCreateKeyEx` of a nested path logs one CreateKey per new level. Volume (whole host, all keywords): OpenKey ≈ 1 500/s, QueryValueKey ≈ 1 550/s, QueryKey ≈ 1 200/s, CloseKey ≈ 1 100/s, CreateKey ≈ 60/s; the §4.2 set alone ≈ 68/s. | §4.2: add OpenKey (2, keyword `0x2000`); the map needs it, and S10 measures with it. §5.5: `KeyObject → name` map from successful CreateKey/OpenKey, bounded like the file map; keep only `Status == 0` (this also drops `STATUS_REPARSE`); normalize `ControlSet00N` per §5.5. §16: key rename is a confirmed v1 gap. **Handles opened before the agent started: decided 2026-10-02, seed from the handle table plus a floor** (see the note below the table). |
| S8 | **Classic Process Start is v4 and carries `CommandLine` and `UserSID`. The PID + 200 ms join is unambiguous. A launch canary costs 0.082% of one core.** | Host: 200 launches 8-wide plus 1 trailing command: 201 of 201 Kernel-Process starts had exactly one classic partner, 1–56 µs apart (avg 3 µs). 5 WOW64 launches (`SysWOW64\cmd.exe`): same result and the same payload layout (the kernel logs with its own pointer size; header flags `0x0340`). Layout (v4, 64-bit): `UniqueProcessKey` @0 (ptr), `ProcessId` @8, `ParentId` @12, `SessionId` @16, `ExitStatus` @20, `DirectoryTableBase` @24 (ptr), `Flags` @32, `UserSID` @36 as `TOKEN_USER` (16 bytes: kernel pointer, attributes, padding) with the SID at @52 (8 + 4·n bytes), then `ImageFileName` (ANSI, NUL-terminated), `CommandLine` (UTF-16, NUL-terminated), `PackageFullName`, `ApplicationId`. TDH renders `UserSID` as an account name and PIDs as hex. Rundown: 318 DCStart (opcode 3) for 317 running processes; 312 carry a command line. Canary: 60 launches at 1/s with Defender on cost 20.9 ms (child) + 6.2 ms (spawner) + 22.1 ms (Defender) = 49 ms each. | §5.2 join kept (200 ms is far above the observed 56 µs). Parser layout above for plan 1b; `UserSID` is parsed from the raw SID, never via TDH. §9.2: **add a process-launch canary** (0.082% ≤ the 0.1% threshold, D3): the agent starts itself with a no-op argument every 60 s and expects its Launch within 10 s. |
| S9 | **UDP passes the E8 gate: with real QUIC streaming the probe used 0.495% of one core and lost nothing.** | Host. Calibration: 2 000 datagrams/s for 60 s → 120 022 send events, 0.65 µs of probe CPU per event (parse + `--model`). Confirmation (3 min, full §4.2 set with UDP, both sessions, a 4K video actively streaming over QUIC): UDP receive (43) avg 2 594/s, peak 9 845/s; send (42) avg 275/s, peak 1 048/s; probe CPU 0.495% of one core; 0 events and 0 buffers lost. Two 10-minute runs earlier the same day (UDP off / on) gave 1.044% / 1.195% with 0 lost, but the video was not actually streaming over QUIC then (UDP ≈ 0.6/s), so they are not the gate evidence. Those runs also measured the whole host: Session A ≈ 10 000 events/s on average (OpenKey ≈ 5 600/s, peak 32 235/s; Kernel-File Create/Cleanup/Close ≈ 1 400/s each; Write ≈ 300/s); Session B ≈ 4/s. The default buffers (min 44, max 256, 64 KB, 22 logical CPUs) lost nothing. | §7.3 / E8: **UDP stays on by default** (`network.udp = true`); D3 threshold met (≤ 2.5%, 0 lost). §4.1: buffer defaults confirmed as is. |
| S10 | **Within budget: probe CPU 0.39–0.42% of one core under a clean build plus browsing, working set 18–26 MB, nothing lost; no first launch missed the 1 s enrichment deadline; builds run ≈ 3% slower (median) with the sessions on.** | Host, full §4.2 set incl. OpenKey, OperationEnd not yet added, UDP on, both sessions, `--model`. Two 15-minute runs (clean `cargo build --workspace --release`, then normal browsing): 0.385% and 0.422% (with `--enrich`) of one core; 0 events and 0 buffers lost; Session A avg 2 708/s and 1 974/s, peaks 52 122/s and 39 251/s (OpenKey avg 1 832/s, peak 42 307/s; Kernel-File Create ≈ 200/s, peak 9 261/s; Write ≈ 190/s, peak 13 398/s; Module Load ≈ 10/s, peak 1 059/s). Enrichment: 38 first launches (153 repeats), p50 30 ms, p95 522 ms, p99 540 ms, max 540 ms; 0 over 1 s, 2 over 500 ms; signatures 19 embedded, 7 catalog, 12 unsigned. Kernel-side cost (clean builds alternating without/with the sessions, 9 pairs over two batches on a busy laptop): median per-pair slowdown 3.2%, mean 5.6% (t = 2.45; two pairs at 15–19% where the untraced build was unusually fast); the steadier second batch alone: median 2.4%, mean 4.1% (t = 1.75). Buffer growth ceiling (all Cleanups counted as Updates, every CreateKey as new, 250 B per event): 150–235 MB per hour of build + browsing. | §13: no fallback needed (CPU far below 2% / 5%, working set far below 150 MB, 0 lost). §6.3: Launch deadline stays 1 s (0 misses ≤ the 5% threshold, D3). Kernel-side cost **accepted 2026-10-02** (≈ 3% typical; the mean straddles the 5% D3 line only through noisy pairs); sub-project 8 re-measures on an idle machine. Sub-project 2: size storage from the ceiling above as an upper bound; the 1 GiB rolling buffer holds at least 4–7 hours of sustained heavy use. |

**S7 follow-up, seeding the key map (decided 2026-10-02, verified the same day).** `SystemExtendedHandleInformation` lists every handle with its kernel object address. For Key handles (type index found from a key handle of the agent's own), that address **is** Kernel-Registry's `KeyObject`/`BaseObject`. Test: a process opened `HKCU\Software` before any trace ran, then created a subkey relative to it. ETW never saw that handle opened, but the handle-table map named it (`\REGISTRY\USER\<SID>\Software`). Seeding works by duplicating each key handle into the agent (`PROCESS_DUP_HANDLE`) and calling `NtQueryKey(KeyNameInformation)`. The object addresses are visible only with `SeDebugPrivilege` enabled (LocalSystem has it; elevated admins must enable it). Host cost, all processes: 15 156 key handles, 42 ms to read the table plus 98 ms to name them; 14 139 named (93%); 955 sit in processes that cannot be opened (protected processes) and 62 failed to duplicate or query. Consequences for plan 1b:
- Order at start: start the sessions, then seed, so no handle falls between the snapshot and the first event. (Refined in rev 3, §7.4: misses re-read the whole table at most once per second, with a negative cache.)
- (Superseded in rev 3, §7.4: the map does use CloseKey, ignoring the agent's own closes, so it holds only live handles.)
- Floor (E11): an event whose key stays unresolved is emitted with the known relative name and the additive `path_unresolved` flag (§10.4), never dropped, and counted (`registry_unresolved`).

**S2, the BootId sources disagree.** In the same boot, `KUSER_SHARED_DATA.BootId` = `PROCESS_TELEMETRY_ID_INFORMATION.BootId` = the start key's high 16 bits, but the registry's `PrefetchParameters\BootId` was one lower in the VM (5 vs 6; after the reboot, 6 vs 7). On the host, where a user had signed in, all three matched. The likely reason is that the registry copy is written only once the boot is marked successful (after an interactive sign-in); the VM had none after its baseline. The mechanism was not tested further: `boot_id` includes `boot_time`, so a repeated BootId cannot merge two boots, but the agent reads BootId only from `KUSER_SHARED_DATA`. This refines 0a §4.4, which calls the two "the same counter".

## 16. Known Limitations

- **No boot-time coverage:** events before the agent starts (early boot, or while the service is stopped) are not captured. An AutoLogger session is sub-project 8.
- **DNS bypassing the Windows client** (browser DoH, custom resolvers) produces no DNS events (0a §5.8). UDP flows (on by default) still show the traffic.
- **DNS events come from a user-mode provider:** a process can forge them (under its own PID only) or suppress its own by patching `EtwEventWrite`.
- **Registry key rename** is invisible in v1: no event carries the new name (S7).
- **Registry value data is read after the event** (§7.5): within about 0.1–0.3 s on the fast path, about 1 s otherwise. A value changed in that window is reported in its new state (type and length checks catch many cases) or as unavailable. An attacker can set a value and replace it at once. Values with types above `REG_QWORD` are emitted without data (`raw_type`, §7.5).
- **A lost CreateKey/OpenKey (or file Create)** leaves a stale map entry. When the address is reused, the stale name can be attached to the new handle: a wrong path, not an unresolved one. The Sensor Health loss counters flag the affected interval.
- **Slow file operations:** a delete or rename that fails after the failure-confirm window is reported as having happened (counted as `file_op_late_failure`).
- **Handles opened with `FILE_DELETE_ON_CLOSE` before the agent started** produce no Delete, because seeding cannot see the flag.
- **Unresolvable handles:** handles in processes the agent cannot open (protected processes), and name queries that block, stay unnamed after seeding (§7.4). Their registry events carry `path_unresolved`; their file writes are counted, not emitted.
- **Hive export** (`reg save HKLM\SAM`) has an ETW event but no schema class yet: a later additive class.
- **An open is not a read:** watchlist `Open` events show access intent. 8.3 names are expanded before matching (§7.2); a name that cannot be expanded (e.g. a file deleted at once) is matched as logged.
- **Network byte counts** are absent: Kernel-Network's Disconnect `size` is not a byte total (S5; checked on loopback only).
- **Self-deleting binaries** may be unhashable; **a binary swapped on disk** between launch and hashing is hashed as the replacement.
- **Work done on the agent's behalf** by other processes (CryptSvc catalog lookups) appears as their activity.
- **Argument spoofing:** a process created suspended whose command line is rewritten before it runs shows the original (fake) command line. The kernel driver (sub-project 6) does not fix this either; detection relies on the behaviour that follows.
- **An admin attacker can blind ETW.** This sub-project detects and reports it (§9); resisting it is sub-project 8, and the driver (sub-project 6) adds an independent sensor.

## 17. Review Log

**Revision 1 → 2 (2026-10-01), after an independent review.** Main changes:
- Ordering stage moved **before** the stateful pipeline; completion stage after it (§3.2). This corrects approved Section 1.
- E11 (unresolvable-actor policy) decided by the user; lookup-at-timestamp and built-in identities (§5.3).
- One uid formula across all paths (§5.2); defined time base (§3.3).
- Session B uses the legacy flag on all builds (§4.1); user-mode provider queue and rate limit (§4.4).
- Security: install path and data-directory ACL checks (§11.1); hash cache keyed on USN (§6.3).
- Gap markers are Sensor Health events (§8.1, §9.3); rolling retention while there is no transport (§8.4); disk-full behaviour (§8.3).
- Spike S10 (file/TCP volume) and the §13 fallback order; runtime manifest-prefix check (§4.3).
- Smaller fixes: registry path handling, watchlist matching, File Update edge cases, canaries, signature error mapping, CI fuzz wiring, single-instance mutex, recovery backoff, DoD definitions, DNS 9501, integrity −28672, buffer counts.

**Revision 2 → 3 (2026-10-02), after the spikes (§15.3).** The results and the user's decisions during the spikes are folded into the design:
- Decisions E12 (registry value read after the event), E13 (handle-table seeding of the file and key maps, `path_unresolved` floor), E14 (process-launch canary); E8's gate passed.
- §4.2: exact keyword masks; `OP_END`/24 and `OpenKey`/2 added; DNS on the Operational keyword only.
- §5: start key formula settled; failure handling by `Status` (registry) and the OperationEnd join (files); rename, overwrite and delete-on-close mappings.
- §6.1: BootId from `KUSER_SHARED_DATA` (checked against telemetry), `boot_time` from the System process.
- New §7.4 (key map, seeding) and §7.5 (value reads); §7.2 8.3 expansion; §7.3 UDP on, byte counts absent.
- New counters (§9.3); §10.4 registry schema fields; tests (§12) and DoD (§14) cover the new mechanisms; §13 records the measurements; §16 updated.
- **After an independent review of the first draft** (1 blocker, 12 major, 9 minor):
  - E12's read was really about 1 s after the write, not "right after". The user re-decided: a fast path in Session A's callback (§7.5).
  - Seeder snapshots apply in stream-time order, with a negative cache, no-access duplicates, cancelled stuck queries and a CPU budget (§7.4).
  - Provisional file entries keep Updates on seeded handles, and a confirmed rename updates the map (§7.1).
  - OperationEnd handling is now failure-only with a stream-time confirm window covering Create, Delete and Rename (§5.5).
  - The key map stores parent links and uses CloseKey (§4.2, §7.4).
  - Value-read flags and checks are defined, and the read is hardened (§7.5).
  - 8.3 expansion runs on a worker (§7.2). Self-filtering moves to emission and covers the canary child (§5.5, §9.2).
  - Per-reason deadlines and the stage protocol are defined (§3.2); counters, fallbacks and limitations are completed.
