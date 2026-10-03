# Sub-project 1 — ETW Sensor Design (Agent Core)

**Status:** Approved (2026-10-01), revision 2. Revision 1 had an independent review; its findings are folded in (§17). Implementation is planned in two parts: **plan 1a** (spikes S1–S10, §15.2) and then **plan 1b** (the build, written from the spike results). Each is reviewed and approved before it runs. Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
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
| E8 | Network: TCP Open/Close; UDP flows synthesized in the agent, behind a measured CPU gate | UDP C2 is real; per-packet events never enabled. |
| E9 | Sensor health monitoring in this sub-project: watchdog, canary, health records in the buffer | Blinding is a standard intrusion step; silent loss makes an EDR untrustworthy. |
| E11 | Never drop an event for lack of context: if the actor's uid is computable, emit with what is known (empty path/name) and count it; drop and count only when no uid can be computed | Incomplete telemetry beats missing telemetry; dropping creates a blind spot an attacker can aim at (very short-lived processes, activity right after a blinding). |

## 3. Architecture

### 3.1 Crates

| Crate | Platform | Contents |
|---|---|---|
| `atlas-etw` | `parse` portable; `session` `#[cfg(windows)]` | **`parse`**: pure functions `&[u8]` (+ provider, event ID, version, per-event pointer size) → typed `RawEvent`. No `unsafe`. **`session`**: start, enable, consume and stop sessions; extended-data extraction (start key). All of the sensor's `unsafe` lives here, behind a safe API. |
| `atlas-buffer` | portable | Segment log (§8). No Windows or ETW knowledge; stores opaque records. |
| `atlas-agent` | binary; Windows functionality behind `#[cfg(windows)]` | Ordering stage, pipeline, process cache, coalescers, flow table, enrichment, completion stage, watchdog, config, service wrapper, CLI. Compiles on Linux (all portable modules tested there); on non-Windows `main` exits with "unsupported platform". |

### 3.2 Threads and data flow

```
ETW (Session A: manifest providers; Session B: system logger, process events)
   │
[1] one consumer thread per session (ProcessTrace callback)
      parse → RawEvent (+ header: pid, tid, raw QPC timestamp, start key)
      → kernel queue (Kernel-* + Session B)    ┐ bounded, never block;
      → user-mode queue (DNS-Client), per-PID  ┘ full ⇒ drop + count
        rate limit (§4.4)
   │
[2] ordering stage: merges all queues; holds each event until
      now − event time ≥ hold (default 750 ms); releases in timestamp order
      late arrivals (older than the last released) pass through at once, counted
   │
[3] pipeline thread: owns ALL mutable state, sees events in time order
      process cache · FileObject→path map · Update coalescer · UDP flow table
      Launch join · actor resolution · self-filter · path normalization
      → domain Events (some marked "pending": enrichment or join outstanding)
   │                                    [4] enrichment workers (2, below-normal priority)
   │                                        SHA-256 + Authenticode, identity cache (§6.3)
[5] completion stage: emits in order; a pending event holds the line until
      complete or its deadline (default 1 s after entering), then goes as-is
   │
      agent-side detection hook (empty; sub-project 3)
   │
[6] buffer writer thread → segment log (§8)

[7] watchdog thread: session/provider checks, canaries, counters, restart (§9)
```

**Order before state.** The stateful pipeline [3] must see events in timestamp order. Otherwise an ImageLoad that arrives before its ProcessStart misses the cache, and a Cleanup that arrives before its Write loses the Update. Stage [2] provides that order. Real-time ETW delivers per-CPU buffers at the session flush timer (250 ms, §4.1), so an event older than flush timer + scheduling slack has almost certainly arrived. The 750 ms hold is three flush periods. A late event (one older than what [2] has already released) is processed immediately, out of order, and counted. It is never dropped.

**Single owner of state.** One pipeline thread removes locking and its race conditions; per-event work is a few hash-map operations. Sharding by process is the escape hatch if profiling ever demands it.

**Completion stage.** Enrichment (§6.3) and the Launch join (§5.2) finish after the pipeline has produced the event. Stage [5] keeps output in timestamp order: an incomplete event blocks the ones behind it until it completes or reaches its deadline, then goes out with what it has. End-to-end latency is ≈ hold + time to complete, about 1–2 s at worst. Prevention (sub-project 7) uses the driver path, not this one.

**Backpressure.** The ETW callback must stay fast: a slow consumer makes ETW drop events in the kernel. The callback only parses and enqueues. Queues never block (kernel queue default 65,536 entries; user-mode queue 8,192); drops are counted per queue. User-mode providers get their own queue so a process flooding forged DNS-Client events (§4.4) cannot push kernel events out.

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
| Buffers | 64 KB each; min = 2 × CPU count; max = max(256, 4 × CPU count) (tuned by the performance spikes) | 64 KB; small |
| Enabled | Providers in §4.2, each with `EVENT_ENABLE_PROPERTY_PROCESS_START_KEY` | `EnableFlags = EVENT_TRACE_FLAG_PROCESS` only |

**Session B uses the legacy flag on every Windows build.** System-logger sessions with `EnableFlags` work from Windows 8 onward, so one code path covers Windows 10 22H2 (build 19045) and Windows 11, and that path is the one CI tests. The newer System Process Provider route (`EnableTraceEx2`, builds ≥ 20348) is not used. Process events are low-volume, so the lack of event-ID filtering here costs little. At session start the kernel emits process rundown events (`DCStart`) for every running process, including its command line; the cache uses them (§6.2).

At start, a leftover session with our name (from a crash) is stopped and recreated. Session names are fixed so the watchdog and the blinding test can address them. The watchdog records each session's `LoggerId` at creation (§9.1).

### 4.2 Providers and filters (Session A)

Each provider is enabled with the listed keywords and an `EVENT_FILTER_TYPE_EVENT_ID` allow-list. **The event-ID filter is evaluated when each event is written**, so events that are filtered out still cost CPU in the kernel I/O path, though they never reach user mode. That cost is what spike S10 measures.

| Provider | Keywords | Event IDs allowed |
|---|---|---|
| Microsoft-Windows-Kernel-Process | `WINEVENT_KEYWORD_PROCESS`, `WINEVENT_KEYWORD_IMAGE` | 1 ProcessStart, 2 ProcessStop, 5 ImageLoad |
| Microsoft-Windows-Kernel-File | `CREATE`, `CREATE_NEW_FILE`, `WRITE`, `DELETE_PATH`, `RENAME_SETLINK_PATH`, `FILEIO` | 12 Create, 13 Cleanup, 14 Close, 16 Write, 17 SetInformation, 26 DeletePath, 27 RenamePath, 30 CreateNewFile; (+24 OperationEnd and/or 10 NameCreate if spike S6 requires them) |
| Microsoft-Windows-Kernel-Registry | per-operation keywords for the events listed | 1 CreateKey, 3 DeleteKey, 5 SetValueKey, 6 DeleteValueKey (+ 2 OpenKey if S7 shows names need a handle map; + a rename source if S7 finds one) |
| Microsoft-Windows-Kernel-Network | `IPV4`, `IPV6` | TCP 12/28 connect, 13/29 disconnect, 15/31 accept; UDP 42/43/58/59 (only if UDP is enabled, §7.3) |
| Microsoft-Windows-DNS-Client | (spike S3 sets the minimal keyword) | 3008 (+ one sibling event carrying `ClientPID`, if S3 requires it) |

`FILEIO` is needed because Cleanup, Close and SetInformation sit under it.

### 4.3 Parsers

- One parser per (provider, event ID, version) that we consume. Pointer-sized fields take the pointer size from **each event's** header flags (32-bit processes log 32-bit pointers from user-mode providers).
- **Unknown higher versions** of a known event: the first time a (provider, ID, version) is seen, the agent calls `TdhGetEventInformation` once and checks that our newest known layout is a strict prefix of it (same field names and types, in order; ETW manifests append fields). The verdict is cached. A prefix → parse with our layout; otherwise → count `unknown_version` and drop. TDH is never used per event.
- Strings: kept as raw UTF-16 slices until an event is emitted (most `Create` events are only matched and mapped, never emitted). At emit, converted lossily (U+FFFD for unpaired surrogates, 0a §6.1) and truncated to the schema limits with the matching `*_truncated` flag.
- Parsers never trust lengths in the payload: every read is bounds-checked, and a malformed event yields an error that is counted, never a panic.
- **TDH as oracle:** a Windows test decodes the `.etl` fixtures with TDH and compares field-by-field with our parsers (§12.2). The same comparison runs locally against live events on the host, so the host's build is checked without committing host recordings.

### 4.4 Trust in user-mode providers

DNS-Client logs from user mode. Any process can register its provider GUID and write forged events, and a process can patch its own `EtwEventWrite` to suppress real ones. Consequences:
- DNS events go to their own queue with a per-PID token-bucket limit (default 100 events/s per PID; excess dropped and counted per PID). A forger cannot starve kernel events.
- A forged event's header PID is the forger's own, so forgery under header-PID attribution can only claim lookups for itself. If S3 forces attribution via a payload `ClientPID`, forged events could name other processes; the spec then marks those events with lower trust (§16).

## 5. Provider → Schema Mapping

### 5.1 Mapping table

| Schema event | Source (event ID) | Mapping rules |
|---|---|---|
| Process Launch | Kernel-Process 1 (v3+) ⨝ Session B classic Process Start | §5.2. |
| Process Terminate | Kernel-Process 2 | `process` from the cache entry live at the event time; on a miss, built from the event's own `ProcessID`, sequence number and `ImageName`. `exit_code` from `ExitCode`. |
| Module Load | Kernel-Process 5 | `actor` = payload `ProcessID` (§5.3); `module.file` from `ImageName`; `base_address` = `ImageBase`; hashes/signature per §6.3. |
| File Create | Kernel-File 30 `CreateNewFile` | New files only; overwrite of an existing file per spike S6. |
| File Update | Kernel-File 16 `Write` | Coalesced per handle (§7.1). |
| File Delete | Kernel-File 26 `DeletePath` | `file.path` from `FilePath`; delete-on-close per S6. |
| File Rename | Kernel-File 27 `RenamePath` | One name from `FilePath`, the other from the `FileObject` map; which is which is S6. |
| File SetAttributes | Kernel-File 17 `SetInformation` | Only `InfoClass` = `FileBasicInformation` (timestamps and attributes; timestomping). |
| File Open (new, §10.1) | Kernel-File 12 `Create` | Only paths matching the watchlist (§7.2). |
| Registry Key Create | Kernel-Registry 1 | Only `Disposition` = `REG_CREATED_NEW_KEY`; path per §5.5. |
| Registry Key Delete | Kernel-Registry 3 | Path per §5.5. |
| Registry Key Rename | — | **v1 gap** unless spike S7 finds a source (§16). |
| Registry Value Set | Kernel-Registry 5 | `type`; `data` from `CapturedData` (truncated to 4 KiB with `data_truncated`), per S4. |
| Registry Value Delete | Kernel-Registry 6 | `reg_value.path` + `name`. |
| Network Open / Close | Kernel-Network | §7.3. |
| DNS Response | DNS-Client 3008 | §5.4. |
| Event Log Activity, Sensor Health (new, §10.2–10.3) | watchdog, pipeline counters | §9. |

### 5.2 Process Launch and process identity

**One uid formula everywhere.** `process.uid` must come out the same whether a process appears as a Launch, an actor, a parent or a Terminate; otherwise events cannot be tied together. So the start-key source is chosen once, by spike S1, and applies to every path:

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
- **`cmd_line`, `user`** from Session B's classic Process Start (`CommandLine`; `UserSID` → `user.uid`, with `user.name` from `LookupAccountSid`, cached). The SID sits inside a `TOKEN_USER`-shaped blob whose layout depends on pointer size (S8).
- **Join:** the pipeline creates the cache entry from whichever half arrives first and marks the Launch pending. The halves match by PID with timestamps within 200 ms (a PID cannot be reused while its process is alive; S8 confirms). If the partner has not arrived by the completion deadline, the Launch is emitted with what exists and `launch_join_miss` increments. For a missing Session B half, the agent first tries `PROCESS_TELEMETRY_ID_INFORMATION` (its `CommandLineOffset`) while the process may still be running.
- Hashes and signature for `process.file` per §6.3.

### 5.3 Actor attribution

| Event kind | Actor source | Why |
|---|---|---|
| Process, image load, file create/delete/rename/setinfo/open, registry, DNS (if S3 passes) | Event header PID + start-key extended data | Logged synchronously in the caller's context. |
| Network | Payload `PID` → cache | Logged in arbitrary/system context; the header is not the owner. |
| Module Load | Payload `ProcessID` → cache | The image is mapped into that process; the header is usually the same, but the payload is authoritative. |
| File Update | The process that opened the handle (from event 12) | The cache manager issues many writes later from System. |

**Lookup rule:** "the cache entry for this PID that was live **at the event's timestamp**", which includes entries in their 30 s post-exit retention. Because [3] runs in time order, this is a straightforward interval check.

**Unresolvable actors (E11).**
1. Built-in identities: PID 0 (Idle), PID 4 (System), and the minimal processes (Secure System, Registry, Memory Compression) get synthetic `ProcessRef`s with their well-known names and a fixed empty path, seeded at start.
2. A miss with a known start key (any synchronous event): live lookup via `ProcessTelemetryIdInformation` (accepted only if its start key matches). If that fails, emit with the computed uid, the PID, and empty `file.path`/`file.name`; count `actor_unresolved` per class. Empty strings are valid under 0a.
3. A miss with no start key (network events for a PID with no live-at-time entry): uid cannot be computed → drop and count `actor_dropped` per class.

### 5.4 DNS Response

- `query.hostname` = `QueryName`; `query.type` = `QueryType`.
- `platform_status` = `QueryStatus` (always kept). `rcode` mapped for known codes: 0 and 9501 (no records) → NOERROR (0); 9001 → FORMERR (1); 9002 → SERVFAIL (2); 9003 → NXDOMAIN (3); 9004 → NOTIMP (4); 9005 → REFUSED (5); otherwise absent.
- `answers[]` parsed from `QueryResults` (format confirmed by fixtures; the parser is fuzzed; at most 64 entries per 0a).
- **Actor:** the event header PID if spike S3 confirms it is the requesting process; otherwise joined to a sibling DNS-Client event carrying `ClientPID` for the same `QueryName` (§4.4 trust note).

### 5.5 Failures and normalization

- **Failed operations are dropped.** Registry: by `Status`. Kernel-File: spike S6 decides whether pre-operation events can report failed operations; if so, `OperationEnd` (24) is enabled and joined on `Irp`.
- **File paths:** NT device paths (`\Device\HarddiskVolume3\…`) → drive paths (`C:\…`) via a device map from `QueryDosDeviceW`, built at start and refreshed every 60 s and on a lookup miss (at most once per 5 s). Prefixes match only on a path-component boundary (`HarddiskVolume1` never matches `HarddiskVolume10\…`). Unmappable paths (shadow copies, network redirectors, unmounted volumes) stay as NT paths.
- **Registry paths:** S7 determines whether `KeyName` (events 3, 5, 6) and `BaseName` + `RelativeName` (event 1) give full paths. If they are handle-relative, a `KeyObject → name` map is built from CreateKey/OpenKey, bounded like the file map. Then: `\REGISTRY\MACHINE\…` → `HKLM\…`; `\REGISTRY\USER\<SID>\…` → `HKU\<SID>\…`; and `HKLM\SYSTEM\ControlSet00N\…` → `HKLM\SYSTEM\CurrentControlSet\…` when N is the current control set (from `HKLM\SYSTEM\Select\Current`, read at start). These are the forms Sigma uses.
- **Self-filtering:** events whose actor is the agent's own process (by start key) are dropped, including the agent's own buffer and log writes. Canary events are consumed by the watchdog, never emitted (§9.2). Activity done on the agent's behalf by other processes (e.g. CryptSvc catalog lookups during signature checks) cannot be self-filtered and is documented (§16).

## 6. Identity and Enrichment

### 6.1 Device and boot identity

- `device.uid`: random UUID generated on first run, persisted in `C:\ProgramData\Atlas\device.json`, written once.
- `device.boot_id`: per 0a §4.4, `BLAKE3("atlas.boot.v1" ‖ BootId ‖ boot_time)[0..16]`. The `boot_time` source is chosen by spike S2 (stable for the whole boot, unaffected by clock changes). Spikes run before the agent is built, so the agent always has a decided source.

### 6.2 Process cache

- **Seeded** at start from Session B's process rundown (`DCStart`: PID, parent, command line, user SID) plus `ProcessTelemetryIdInformation` per PID (start key, create time, image path). Seeded processes are cached, not emitted as Launch events.
- **Maintained** from Launch and Terminate events.
- **Keyed by start key** (or by fallback uid if S1 forces it), never by PID; a PID index holds each PID's entries with their live intervals.
- **Retention:** entries stay 30 s after Terminate (late events still resolve), then are removed.
- **Bounded:** a hard entry cap with an eviction counter, so lost Terminate events cannot grow memory without limit.

### 6.3 Hashes and signatures

- Applies to `process.file` on Launch and `module.file` on Module Load.
- **Cache key:** volume serial + 128-bit file ID (`FILE_ID_INFO`) + **the file's USN** (`FSCTL_READ_FILE_USN_DATA`). The USN changes on every modification and, unlike last-write time, cannot be set back by a user. On volumes without a USN journal, results are not cached. Entries for a path are also invalidated when the pipeline sees an Update, Rename, SetAttributes or Delete on it.
- **Work** runs on 2 below-normal-priority worker threads, never on an ETW or pipeline thread. Files are opened with full sharing (`FILE_SHARE_READ | WRITE | DELETE`) and read sequentially.
- **SHA-256** for files up to the size cap (default 100 MB); larger files skip hashing.
- **Signature:** `WinVerifyTrust` with `WTD_UI_NONE`, `WTD_REVOKE_NONE`, `WTD_CACHE_ONLY_URL_RETRIEVAL` (no network calls from the sensor), and catalog lookup (`CryptCATAdmin*`) for catalog-signed OS files. `signer` = the leaf certificate's subject CN. Mapping: valid chain → `Valid`; no signature (`TRUST_E_NOSIGNATURE`) → `Unsigned`; a signature that fails verification (bad digest, untrusted root, explicit distrust, revoked per local cache) → `Invalid`; any operational error (file locked, CryptSvc unavailable, timeout) → signature **absent**, counted.
- **Deadline:** the completion stage's (§3.2). A late result is cached for the next event on the same file. Spike S10 measures how often a first-ever launch misses its deadline; if often, the deadline for Launch events is raised.
- **Known gaps:** a binary deleted right after launch may be unreadable. Hashing re-opens the file by path, so a binary renamed away and replaced in between is hashed as the replacement (§16).

## 7. Coalescing, Watchlist, Flows

### 7.1 File Update coalescing

- Event 12 (`Create`) records `FileObject → (path, actor)`. A Create on a `FileObject` already in the map replaces the entry (address reuse after a lost Close) and counts it.
- The first event 16 (`Write`) on a mapped `FileObject` marks it written; further writes are absorbed.
- Event 13 (`Cleanup`, last handle closed) emits **one** File Update for a written handle; event 14 (`Close`) removes the map entry.
- Writes after Cleanup (lazy-writer flushes, memory-mapped paging writes) are absorbed into the already-emitted Update and counted.
- Writes on a `FileObject` not in the map (handles opened before the agent or a session restart) cannot be attributed to a path → counted `unknown_file_object`, dropped. S6 checks whether the `FILENAME` keyword's rundown (`NameCreate` for already-open files) can close this gap; an attacker who blinds the sensor and keeps a handle open is otherwise invisible for that handle (§16).
- The map is bounded (cap + LRU eviction + counter); entries from failed Creates age out this way if `OperationEnd` is not enabled.

### 7.2 Watchlist

- Patterns are **volume-relative** globs (`\Windows\System32\config\SAM`, `\Users\*\AppData\Local\Google\Chrome\User Data\*\Login Data`), case-insensitive, compiled once into a single `GlobSet`. They are matched against the path with its volume prefix removed, so they also match shadow copies (`\Device\HarddiskVolumeShadowCopyN\…`), the classic route for stealing SAM and NTDS.
- Alternate data streams are stripped before matching (`file.txt:stream`, `file::$DATA` match `file.txt` / `file`). 8.3 short names in `Create` paths are a known gap unless S6 shows a cheap expansion (§16).
- Built-in default list (replaceable or extendable in config): Chromium `Login Data`, `Cookies`, `Local State` and Firefox `logins.json`, `key4.db` under `\Users\*\…`; `SAM`, `SECURITY`, `SYSTEM` hives and `*.sav`/`*.bak` copies under `\Windows\System32\config\`; `\Windows\NTDS\ntds.dit`; `\Users\*\.ssh\*`; `\Users\*\.aws\credentials`, `\Users\*\.azure\*`, `\Users\*\AppData\Roaming\gcloud\*`; `*.kdbx`.
- A `Create` on a matching path emits File System Activity **Open** (§10.1). An open is not proof of a read; rules should treat it as access intent.
- Repeated opens of the same path by the same process are coalesced to one per 60 s.

### 7.3 Network

- **TCP:** connect (12/28) → Open outbound; accept (15/31) → Open inbound; disconnect (13/29) → Close. `bytes_in`/`bytes_out` filled only if spike S5 shows the Disconnect `size` field carries totals. Per-packet events are never enabled.
- **UDP flows:** datagram events (42/43/58/59) feed a flow table keyed by (actor uid, local endpoint, remote endpoint). The first datagram emits Open (direction from send vs. receive); a Close is emitted after 60 s idle (configurable), timestamped at the last datagram. The table is bounded (cap + eviction counter; evicted flows emit Close).
- **Performance gate (E8):** spike S9 measures UDP event rate and agent CPU during heavy QUIC streaming. Over budget (§13) → UDP is disabled by default (`network.udp = false`) and a decision-log entry is written.
- Ports and addresses are decoded in network byte order (confirmed by fixtures).

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
- Matching is by path + the agent's own start key as actor (not by value), so it does not depend on S4. A kernel provider event attributed to the agent cannot be forged by another process.
- A miss → Event Log Activity **Disable** for that provider. This catches a provider that silently stops delivering, which the session checks cannot see.
- Canary events are consumed by the watchdog and never emitted (no daily noise).
- Kernel-Process and Kernel-Network rely on the watchdog's provider checks; a process-launch canary is added only if S8 shows its cost is negligible.
- `HKLM\SOFTWARE\Atlas` and the canary directory carry the same SYSTEM + Administrators ACL as §11.1.

### 9.3 Sensor Health records

Emitted every 60 s (and immediately on a non-zero change in a loss counter):
- **Loss:** ETW events lost, real-time buffers lost, queue drops (per queue), DNS rate-limit drops, `actor_dropped`, buffer backlog drops, gap events (§8.4).
- **Quality:** late arrivals, parse errors, `unknown_version`, `launch_join_miss`, `actor_unresolved`, `unknown_file_object`, enrichment misses and errors.
- **Housekeeping:** cache/map/flow-table evictions, `retention_evictions`.
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

## 11. Configuration, Service, CLI

### 11.1 Install locations and permissions

- **Binary:** `service install` copies the executable to `C:\Program Files\Atlas\atlas-agent.exe` (writable only by administrators) and registers that path, quoted. It never registers the path it was run from (`target\`, `Downloads\`, …).
- **Data:** `C:\ProgramData\Atlas\` holds `buffer\`, `canary\`, `agent.toml`, `device.json`, `cursor`, and `logs\`. `C:\ProgramData` lets ordinary users create folders, so a non-admin could pre-create `Atlas\` with their own ACL or plant a junction, turning the agent's deletes and renames into SYSTEM-level primitives. Therefore:
  - `service install` creates the directory with owner Administrators and a **protected** DACL (no inheritance): SYSTEM and Administrators full control, nothing else.
  - **On every start** the agent verifies the owner, the protected DACL, and that neither the directory nor anything it opens inside is a reparse point (files opened with `FILE_FLAG_OPEN_REPARSE_POINT` and checked). On failure it refuses to start and reports to the Windows Event Log (its own log directory is untrusted at that point).

### 11.2 `agent.toml`

- Keys: buffer cap, retention/overflow policy, `transport` (`none` in this sub-project), ordering hold, completion deadline, enrichment size cap, watchlist (replace or extend the default), per-class enable, `network.udp`, UDP idle timeout, DNS per-PID rate, log level.
- Built-in defaults; the file only overrides them. Validated at start: an invalid file stops startup with a clear error in the Windows Event Log (Application, source `Atlas`). No hot reload in v1.

### 11.3 CLI

| Command | Behaviour |
|---|---|
| `atlas-agent run` | Console mode (admin required); Ctrl+C triggers a clean stop. Refuses to start if another instance holds `Global\Atlas-Agent` (single-instance mutex), so it cannot tear down the service's sessions. |
| `atlas-agent service install \| uninstall \| start \| stop` | Install per §11.1; LocalSystem, auto-start. Recovery: restart after 5 s, then 60 s, then every 10 min, so a bad config does not loop hot. Uses the `windows-service` crate. |
| `atlas-agent dump [--follow] [--from <cursor>] [--class <name>]` | JSON lines from the buffer; read-only (never acks); §8.5 concurrency rules. |

### 11.4 Lifecycle

- **Start:** take the mutex → verify the data directory (§11.1) → load config → load `device.json` → compute `boot_id` → start sessions (recreating stale ones) → seed the cache from the rundown → start threads.
- **Clean stop:** flush both sessions → drain the ordering and completion stages → flush the buffer → stop the sessions. No in-flight event is lost on a clean stop.
- **Diagnostic log:** `tracing` with a rolling file in `logs\`; its writes are self-filtered (§5.5).

## 12. Testing

### 12.1 Tier 1 — pure, Linux CI

- Parser unit tests per event and version, built from fixture byte strings, for both pointer sizes where relevant.
- **Property tests:** the ordering stage releases in timestamp order and passes late events through; the completion stage preserves order; actor lookup-at-timestamp resolves correctly across PID reuse; the coalescer emits exactly one Update per written handle; every UDP Open gets exactly one Close; retention and head + tail overflow never delete the pinned head; buffer round-trip; recovery after truncation at **every** byte offset of a segment (simulated torn writes).
- **Fuzz targets** (each its own fuzz workspace under its crate, run 10 min nightly): `parse_any` (dispatches over every parser), `dns_query_results`, `buffer_recover` (arbitrary bytes as a segment file).
- **CI wiring** (part of the plan): `fuzz.yml` becomes a matrix over (fuzz workspace, target) with per-target corpus cache keys, crash artifacts, and PR path filters per crate; `ci.yml` runs `cargo check` on each fuzz workspace and `cargo audit` on each fuzz lockfile; `atlas-agent` builds on the Linux job.

### 12.2 Tier 2 — `.etl` replay, Windows CI

- Fixtures are recorded **on a GitHub Windows runner, not the host**: a `workflow_dispatch` job runs the scripted scenario on a clean, throwaway runner with file-mode sessions and uploads the `.etl` files. The repo is public, and a host recording would leak usernames, paths and DNS history. Fixtures are reviewed, then committed.
- Replay feeds the real consumer (`OpenTrace` on a file) through the full pipeline and compares against expected domain events.
- TDH oracle: every fixture event is also decoded with TDH and compared field-by-field with our parser (and locally against live host events, §4.3).

### 12.3 Tier 3 — live sessions, Windows CI

- Tests start real sessions on `windows-latest` (the runner is admin), run the scripted scenario (spawn a process with a known command line, create/write/rename/delete a file, open a watchlisted path, create/set/delete a registry key and value, open a TCP connection, send UDP, resolve a name), and assert the expected normalized events, filtered to the test's own process tree, with timeouts.
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

Late arrivals, enrichment misses and DNS rate-limit drops are reported, not gated. The plan also records events per second per class and buffer growth per day, which sub-project 2 needs to size ClickHouse.

**Fallbacks if over budget** (applied in order, each one a decision-log entry): UDP off (S9); then File Update and SetAttributes off (drops the `FILEIO` and `WRITE` keywords, the largest kernel-side cost; Create/Delete/Rename/Open remain); then per-class disable in config as a last resort.

## 14. Definition of Done

### 14.1 Done means

1. Every spike in §15.2 is resolved and written into §15.3; fallbacks taken are in the decision log.
2. CI runs tiers 1–3 green on every push; the new fuzz targets run nightly.
3. §10 schema additions are in `atlas.events.v1`, pass `buf breaking`, and have golden fixtures and `schema-reference.md` entries.
4. **24-hour host run as a service:** no crashes, within the §13 budget (from the agent's own Sensor Health records), zero lost events as defined in §13. A `dump` shows every class the tier-3 scenario produces, plus Sensor Health records.
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
| S9 | **UDP passes the E8 gate: with real QUIC streaming the probe used 0.495% of one core and lost nothing.** | Host. Calibration: 2 000 datagrams/s for 60 s → 120 022 send events, 0.65 µs of probe CPU per event (parse + `--model`). Confirmation (3 min, full §4.2 set with UDP, both sessions, a 4K video actively streaming over QUIC): UDP receive (43) avg 2 594/s, peak 9 845/s; send (42) avg 275/s, peak 1 048/s; probe CPU 0.495% of one core; 0 events and 0 buffers lost. Two 10-minute runs earlier the same day (UDP off / on) gave 1.044% / 1.195% with 0 lost, but the video was not actually streaming over QUIC then (UDP ≈ 0.6/s), so they are not the gate evidence. Those runs also measured the whole host: Session A ≈ 10 000 events/s on average (OpenKey ≈ 5 600/s, peak 32 235/s; Kernel-File Create/Cleanup/Close ≈ 1 400/s each; Write ≈ 300/s); Session B ≈ 4/s. The default buffers (min 44, max 256, 64 KB, 22 logical CPUs) lost nothing. | §7.3 / E8: **UDP stays on by default** (`network.udp = true`); D3 threshold met (≤ 2.5%, 0 lost). §4.1: buffer defaults confirmed as is. |
| S6 | Kernel-File: do 26/27/30 fire for failed operations? What is in `RenamePath.FilePath`? How are overwrites of existing files and delete-on-close seen? Does the `FILENAME` rundown map already-open handles? Are `Create` names ever 8.3 short names? | Enable `OperationEnd` and join on `Irp`; parse `Create` disposition for overwrites; document remaining gaps. |
| S5 | **Disconnect `size` does not carry byte totals.** | Host, loopback only (decided after a VM-side variant was dropped): a client sent exactly 1 000 000 bytes and received 3 000 000. Both disconnects (13, v0), one per side, had `size` = 0, and so did the connect (12) and accept (15). Header vs payload: the accept was logged under the client's header PID while its payload `PID` named the server. | §7.3: `bytes_in`/`bytes_out` stay absent (the spec's fallback). §5.3 confirmed: network events use the payload `PID`. Not tested over a real NIC; a zero on loopback rules out totals as the field's meaning. |
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
| S8 | **Classic Process Start is v4 and carries `CommandLine` and `UserSID`. The PID + 200 ms join is unambiguous. A launch canary costs 0.082% of one core.** | Host: 200 launches 8-wide plus 1 trailing command: 201 of 201 Kernel-Process starts had exactly one classic partner, 1–56 µs apart (avg 3 µs). 5 WOW64 launches (`SysWOW64\cmd.exe`): same result and the same payload layout (the kernel logs with its own pointer size; header flags `0x0340`). Layout (v4, 64-bit): `UniqueProcessKey` @0 (ptr), `ProcessId` @8, `ParentId` @12, `SessionId` @16, `ExitStatus` @20, `DirectoryTableBase` @24 (ptr), `Flags` @32, `UserSID` @36 as `TOKEN_USER` (16 bytes: kernel pointer, attributes, padding) with the SID at @52 (8 + 4·n bytes), then `ImageFileName` (ANSI, NUL-terminated), `CommandLine` (UTF-16, NUL-terminated), `PackageFullName`, `ApplicationId`. TDH renders `UserSID` as an account name and PIDs as hex. Rundown: 318 DCStart (opcode 3) for 317 running processes; 312 carry a command line. Canary: 60 launches at 1/s with Defender on cost 20.9 ms (child) + 6.2 ms (spawner) + 22.1 ms (Defender) = 49 ms each. | §5.2 join kept (200 ms is far above the observed 56 µs). Parser layout above for plan 1b; `UserSID` is parsed from the raw SID, never via TDH. §9.2: **add a process-launch canary** (0.082% ≤ the 0.1% threshold, D3): the agent starts itself with a no-op argument every 60 s and expects its Launch within 10 s. |
| S3 | **The 3008 header PID and start key are the requesting process. The minimal keyword is `0x8000000000000000` (the Operational channel).** | Host, `DnsQuery_W` from a test process: 6 of 6 lookups produced exactly one 3008 (v0) with the requester's header PID and extended-data start key: fresh, cached, cache-bypass, AAAA, a CNAME chain, and NXDOMAIN (`QueryStatus` 9003). Events 3009–3020 are logged by the DNS Client service with `ClientPID` = the requester. A WOW64 caller (`Resolve-DnsName` from 32-bit Windows PowerShell) was attributed the same way, with a 32-bit event header (3008 has no pointer fields, so the layout is unchanged). Keyword masks tried alone: only `0x8000000000000000` delivered 3008 (6 of 6); `0x100`, `0x4000000000000000`, `0x10000000`, `0x100000000` and `0x200000000` delivered none. `QueryResults` format: `addr;addr;` with IPv6 in text form; CNAMEs as `type: 5 <name>;` entries before the addresses; empty on NXDOMAIN. | §5.4: actor = header PID + start key; no sibling join, so the §16 note about `ClientPID` forgery does not apply. §4.2: DNS-Client keyword `0x8000000000000000`, event 3008 only. The `QueryResults` parser handles the `type: N <name>;` form (and is fuzzed). |
| S4 | **`CapturedData` is never populated.** | Host, all Kernel-Registry keywords: 11 of 11 SetValueKey (5, v0) events had `CapturedDataSize` = 0 and empty `PreviousData`, for REG_SZ, EXPAND_SZ, MULTI_SZ, DWORD, QWORD, BINARY 16 B and BINARY 8 KiB. `Type` and `DataSize` are correct. | **Decided 2026-10-02: read after the event.** Right after a successful SetValueKey whose key path is resolved, the agent reads the value (≤ 4 KiB, `data_truncated` as before) and marks it with an additive flag, `reg_value.data_read_after` (§10). The read is racy: a value deleted or changed within milliseconds is missed or read in its new state. Cost is low (SetValueKey ≈ 2/s host-wide). The driver (sub-project 6) later supplies the exact data. |
| S7 | **Names are handle-relative, and the handle identity is per handle, not per key. No event reveals a key rename. There is no rundown.** | Host. `KeyName` (3, 5, 6) and `BaseName` (1, 2) are always empty. `RelativeName` (1, 2) is relative to `BaseObject` unless it starts with `\REGISTRY\`. Replaying a `KeyObject → name` map built from successful CreateKey/OpenKey resolved 30 of 30 successful writes to full paths. `KeyObject` is per handle: three processes opening `\REGISTRY\MACHINE` got three different values. `RegRenameKey` produced only OpenKey, QueryKey and CloseKey; the new name appears nowhere. `EVENT_CONTROL_CODE_CAPTURE_STATE` produced no rundown (counts per ID unchanged within noise). Failures carry `Status` (`0xC0000034` not found). Opening `CurrentControlSet` first returns `0x104` (`STATUS_REPARSE`), then succeeds as `\REGISTRY\MACHINE\SYSTEM\ControlSet001\…`. WOW64 views appear as `…\WOW6432Node\…`. CreateKey `Disposition`: 1 = created, 2 = opened. `RegCreateKeyEx` of a nested path logs one CreateKey per new level. Volume (whole host, all keywords): OpenKey ≈ 1 500/s, QueryValueKey ≈ 1 550/s, QueryKey ≈ 1 200/s, CloseKey ≈ 1 100/s, CreateKey ≈ 60/s; the §4.2 set alone ≈ 68/s. | §4.2: add OpenKey (2, keyword `0x2000`); the map needs it, and S10 measures with it. §5.5: `KeyObject → name` map from successful CreateKey/OpenKey, bounded like the file map; keep only `Status == 0` (this also drops `STATUS_REPARSE`); normalize `ControlSet00N` per §5.5. §16: key rename is a confirmed v1 gap. **Handles opened before the agent started: decided 2026-10-02, seed from the handle table plus a floor** (see the note below the table). |
| S6 | **`CreateNewFile` fires only on success; `DeletePath` and `RenamePath` fire before the outcome; `RenamePath.FilePath` is the new name. Delete-on-close and overwrites have no dedicated event. `Create` names can be 8.3 short names. There is no rundown.** | Host, keywords `0x1EF0` (§4.2 set + FILENAME + OP_END), no ID filter, 0 events lost. **Failures:** a `CREATE_NEW` on an existing file or in a missing directory gave `Create` (12) then `OperationEnd` (24, same `Irp`, `0xC0000035` / `0xC000003A`) and no 30. A delete of a read-only file gave 26 then 24 with `0xC0000121`. A rename onto an existing name gave 27 then 24 with `0xC0000035`. A delete refused for sharing failed at the open (12 + 24 `0xC0000043`, no 26). **Rename:** 27's `FilePath` is the target name; NameDelete (11) old / NameCreate (10) new follow a successful rename. **Overwrite:** `CREATE_ALWAYS` / `TRUNCATE_EXISTING` on an existing file give 12 (disposition open / open-if) plus `SetInformation` (17) with `InfoClass` 19 (end of file), no 30. **Delete:** `DeleteFileW` → 26 (`InfoClass` 64); disposition delete → 26 (13); POSIX delete → 26 (64); `FILE_FLAG_DELETE_ON_CLOSE` → no 26, only `CreateOptions` bit `0x1000` on 12. **Timestomp / attributes:** 17 with `InfoClass` 4. **Writes:** one 16 per write call on the handle's `FileObject`, then Cleanup (13), Close (14). **Names:** opening through a short name logs `…\ATLASS~1\…\LONG-F~1.TXT`; streams log as written (`h.txt:secret`, `h.txt::$DATA`). **Rundown:** capture-state on FILENAME produced nothing; a handle opened before the session was never named (only its 16/22/13 appeared). | §4.2: add `OperationEnd` (24, keyword OP_END `0x40`); §5.5: hold 26/27 until their 24 (by `Irp`, within the completion deadline) and drop on a non-success status; 30 needs no join. §5.1 Rename: new name from 27, old name from the `FileObject` map. §5.1 Create: overwrite = 17 with class 19 on a mapped handle, emitted as File Update. §5.1 Delete: also emit Delete at Cleanup for a handle created with `FILE_DELETE_ON_CLOSE`. §7.2: expand 8.3 components before watchlist matching (NameCreate maps `FileKey` → long name; else `GetLongPathNameW`). §7.1: **handles opened before the agent: decided 2026-10-02, seed the `FileObject` map from the handle table** (verified the same day: the handle table's object address equals Kernel-File's `FileObject`; a write on a handle opened before any trace was named `…\held.txt` from the snapshot, while ETW never saw its Create). Only disk files are named (`GetFileType` = disk, then `GetFinalPathNameByHandleW`), from a worker thread with a timeout, because some handle types can block a name query. Host cost: 10 494 file handles; 3 830 disk files named in 1.18 s (3 timeouts at 200 ms); 4 477 non-disk; 2 174 in processes that cannot be opened. The opener (actor) of a seeded handle is the owning process. Writes on handles that stay unnamed are still counted `unknown_file_object`. |

**S7 follow-up, seeding the key map (decided 2026-10-02, verified the same day).** `SystemExtendedHandleInformation` lists every handle with its kernel object address. For Key handles (type index found from a key handle of the agent's own), that address **is** Kernel-Registry's `KeyObject`/`BaseObject`. Test: a process opened `HKCU\Software` before any trace ran, then created a subkey relative to it. ETW never saw that handle opened, but the handle-table map named it (`\REGISTRY\USER\<SID>\Software`). Seeding works by duplicating each key handle into the agent (`PROCESS_DUP_HANDLE`) and calling `NtQueryKey(KeyNameInformation)`. The object addresses are visible only with `SeDebugPrivilege` enabled (LocalSystem has it; elevated admins must enable it). Host cost, all processes: 15 156 key handles, 42 ms to read the table plus 98 ms to name them; 14 139 named (93%); 955 sit in processes that cannot be opened (protected processes) and 62 failed to duplicate or query. Consequences for plan 1b:
- Order at start: start the sessions, then seed, so no handle falls between the snapshot and the first event. On a miss, re-read the table for that address, at most once per second.
- The map needs no CloseKey: every handle created while the session runs arrives through Create/OpenKey, which overwrites a reused address.
- Floor (E11): an event whose key stays unresolved is emitted with the known relative name and an additive `unresolved` path flag (§10), never dropped, and counted (`registry_unresolved`).

**S2, the BootId sources disagree.** In the same boot, `KUSER_SHARED_DATA.BootId` = `PROCESS_TELEMETRY_ID_INFORMATION.BootId` = the start key's high 16 bits, but the registry's `PrefetchParameters\BootId` was one lower in the VM (5 vs 6; after the reboot, 6 vs 7). On the host, where a user had signed in, all three matched. The likely reason is that the registry copy is written only once the boot is marked successful (after an interactive sign-in); the VM had none after its baseline. The mechanism was not tested further: `boot_id` includes `boot_time`, so a repeated BootId cannot merge two boots, but the agent reads BootId only from `KUSER_SHARED_DATA`. This refines 0a §4.4, which calls the two "the same counter".

## 16. Known Limitations

- **No boot-time coverage:** events before the agent starts (early boot, or while the service is stopped) are not captured. An AutoLogger session is sub-project 8.
- **DNS bypassing the Windows client** (browser DoH, custom resolvers) produces no DNS events (0a §5.8). UDP flows (if enabled) still show the traffic.
- **DNS events come from a user-mode provider:** a process can forge them (under its own PID) or suppress its own by patching `EtwEventWrite`. If S3 forces `ClientPID` attribution, forged events can name other processes.
- **Registry key rename** may be invisible in v1 (S7).
- **Hive export** (`reg save HKLM\SAM`) has an ETW event but no schema class yet: a later additive class.
- **An open is not a read:** watchlist `Open` events show access intent. 8.3 short names may evade the watchlist (S6).
- **Handles opened before the agent started** (or before a session restart) produce no File Update events unless S6 finds a rundown that maps them.
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
