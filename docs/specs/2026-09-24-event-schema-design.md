# Sub-project 0a — Event Schema Design

**Status:** Implemented (2026-09-24). Reference: docs/schema-reference.md
**Roadmap:** Sub-project 0 was split into **0a — Event schema** (this spec) and **0b — Scaffolding** (monorepo, Docker Compose, Hyper-V VM, CI).
**Depends on:** nothing. **Depended on by:** every other sub-project.

---

## 1. Purpose & Scope

Define the normalized event model that all Atlas telemetry uses: produced by sensors (ETW now, kernel driver later, Linux eBPF someday), buffered on the agent, sent over the wire, validated by the server, matched by detection rules, and stored in ClickHouse.

**In scope**
- `atlas-proto` crate: protobuf definitions of all events (the wire/buffer contract).
- `atlas-schema` crate: typed Rust domain model, conversions, validation, `process.uid` derivation.
- Seven v1 event classes (§5).
- Test suite, fuzz target, benchmarks, schema reference doc.
- A bare Cargo workspace sufficient to build the two crates (full scaffolding is 0b).

**Out of scope** (owned elsewhere)
- gRPC service definitions, enrollment, ClickHouse DDL → sub-project 2.
- Sigma field mapping, process-context cache → sub-project 3.
- Which events the sensor emits, hashing/signature caching, file-read volume policy → sub-project 1.
- OCSF / ECS exporters → later, on demand.
- CI wiring of `buf breaking` → 0b.

## 2. Key Decisions (from brainstorm)

| # | Decision | Rationale |
|---|---|---|
| D1 | **Own typed model, modeled on OCSF**; export adapters (OCSF, ECS) later | Internal model stays clean and ours; OCSF class structure maps onto Rust enums and ClickHouse tables; OCSF is Windows/Linux-neutral; OCSF export becomes near-identity. Lab-SIEM users (mostly ECS/Elastic) served by an adapter. |
| D2 | **Protobuf = wire contract; Rust domain types = code model; `TryFrom` between them** | Protobuf gives a versioned, language-neutral contract for gRPC and the disk buffer. Domain types give real enums and enforced required fields. Wire→domain conversion is the single validation choke point for untrusted agent data. |
| D3 | **Deterministic process identity**: `uid = hash(device.uid, boot_id, start_key)` | Stateless; survives agent restarts; ETW sensor and future driver compute the same id independently. |
| D4 | **v1 classes**: Process, Module, Network, File System, Registry Key, Registry Value, DNS | Exactly what sub-project 1's ETW sensor emits; DNS added for C2/exfil detection value. |
| D5 | **Actor-core denormalization**: every event carries a small `actor.process` core; full process detail only in Process Launch | Covers the fields most Sigma file/registry/network rules reference while keeping high-volume classes small. Rules needing more use a process-context cache keyed by `uid` (sub-project 3). |

**Pinned OCSF version:** OCSF **1.9.0** (released 2026-08-03). All numeric IDs in this spec are from that release.

**Minimum Windows version:** Windows 10 1703 (required by `PsGetProcessStartKey` for the future driver; practical targets are Windows 10 22H2 and Windows 11).

## 3. Architecture

### 3.1 Crates

**`atlas-proto`** — generated code only.
- `.proto` sources under `crates/atlas-proto/proto/atlas/events/v1/`:
  - `objects.proto` — shared objects (`ProcessRef`, `Process`, `File`, `User`, `NetworkEndpoint`, …)
  - `process.proto`, `module.proto`, `network.proto`, `file.proto`, `registry.proto`, `dns.proto` — one per class family
  - `event.proto` — `Event` envelope with `oneof kind` over the seven classes
- Package `atlas.events.v1`.
- Compiled in `build.rs` with **`prost-build` + `protox`** (pure-Rust protobuf compiler — no `protoc` binary on Windows or CI).
- No hand-written logic.
- Wire layout: `meta` fields are flattened into `Event` (fields 1–3); per-activity fields live in oneof sub-messages mirroring the domain enums.

**`atlas-schema`** — hand-written domain model.
- `Event { meta: EventMeta, device: DeviceContext, kind: EventKind }`
- `enum EventKind { Process(ProcessActivity), Module(ModuleActivity), Network(NetworkActivity), File(FileSystemActivity), RegistryKey(RegistryKeyActivity), RegistryValue(RegistryValueActivity), Dns(DnsActivity) }`
- Each class struct carries a Rust `enum` of its activities; activity-specific fields live on the activity enum variants so that required-ness is enforced by the type system (e.g. `FileAction::Rename { file_result: File }`).
- OCSF numeric IDs (`class_uid`, `category_uid`, `activity_id`, `type_uid`) are **derived by methods** on the domain types, never stored as separate data.
- Conversions:
  - **Domain → wire: `From` (infallible).** Used by the agent to encode.
  - **Wire → domain: `TryFrom` (fallible, validating).** Used by the server on ingest and by the agent when replaying its disk buffer (a corrupt file is untrusted input too).
- `process_uid(...)` derivation function (§4.3).

### 3.2 Data flow

```
ETW sensor → domain Event → encode (proto bytes) → disk buffer → gRPC → server
                                                                          │
                                        TryFrom (validate) ← decode ←─────┘
                                                │
                                   domain Event → detection / ClickHouse
```

Agent-side detection runs on domain events *before* encoding.

## 4. Common Fields & Identity

### 4.1 `meta` (every event)

| Field | Type | Notes |
|---|---|---|
| `event_id` | UUIDv7 (16 bytes) | Generated by the agent at event creation. Server de-duplicates on it (the buffer delivers at-least-once). Time-ordered → efficient ClickHouse key. |
| `time` | `i64` ns since Unix epoch, UTC | When the event occurred (from the ETW event header). Nanoseconds preserve ordering of bursts (launch + module loads within 1 ms). OCSF exporter truncates to ms. Server ingest time is a storage column, not a schema field. |
| `sensor` | enum `Etw \| Driver` | Which sensor produced the event. Needed for debugging and cross-sensor dedup once the driver exists. |

### 4.2 `device` (every event)

| Field | Type | Notes |
|---|---|---|
| `device.uid` | UUID (16 bytes) | Random, generated once at agent install, persisted. Stable across reboots, renames, re-enrollment. |
| `device.boot_id` | 16 bytes, opaque | Unique per boot of this device, stable for the whole boot (including across agent restarts). 16 bytes so a Linux sensor can use `/proc/sys/kernel/random/boot_id` (a UUID) directly. Windows derivation in §4.4. |

Hostname and OS details are **not** carried per event; the server knows them from enrollment and exporters re-attach them.

**Security rule:** the server must not trust `device.uid` as sent. It is checked against the agent's authenticated mTLS identity and mismatches are rejected (enforced in sub-project 2). The schema crate exposes `device.uid` so the server can perform the check.

### 4.3 Process identity

```
process.uid = BLAKE3( "atlas.process.v1" ‖ device.uid[16] ‖ boot_id[16] ‖ start_key[u64 LE] )[0..16]
```

- 128-bit, stored as 16 bytes; rendered as lowercase hex in text contexts.
- `"atlas.process.v1"` is a domain-separation tag: a future formula change bumps the tag, so old and new ids cannot collide.
- **Computed only in Rust user mode.** The future kernel driver forwards the raw start key; it never hashes (thin kernel).
- Linux sensor (future) supplies its own `start_key` equivalent (e.g. derived from pid + start time); formula unchanged.
- `start_key` source on Windows: §4.4.

### 4.4 Windows identity sources

**`start_key`**: the Windows *process start key*, treated as an **opaque `u64`**. Sources disagree on how it packs a boot counter and a sequence number, so its internal layout is never relied on. Three sources return it:
- Kernel: `PsGetProcessStartKey` (documented `ntddk.h` export, Windows 10 1703+). The future driver forwards this raw value.
- ETW: any provider can be enabled with `EVENT_ENABLE_PROPERTY_PROCESS_START_KEY` (Windows 10 1507+). This stamps the start key of the logging process onto every event as extended data, which gives the actor's start key on file, registry, network, module and DNS events.
- The new process's start key on Process Launch: `Microsoft-Windows-Kernel-Process` ProcessStart v3+ carries `ProcessSequenceNumber` / `ParentProcessSequenceNumber`. **It is unverified whether that field equals `PsGetProcessStartKey` or is only its sequence portion.** Sub-project 1 must confirm this in the VM. If it doesn't match, query `ProcessTelemetryIdInformation` for the new pid, or use the fallback formula below.

**`boot_id`**: derived by the agent (never the driver) as `BLAKE3("atlas.boot.v1" ‖ BootId[u32 LE] ‖ boot_time)[0..16]`. `BootId` is `KUSER_SHARED_DATA.BootId`, which is readable from user and kernel mode on Windows 10+ and is the same counter as the `PrefetchParameters\BootId` registry value. The counter alone is not enough: it only increments on *successful* boots, so a failed boot can repeat a value. Sub-project 1 picks a `boot_time` source that stays stable for the whole boot and is unaffected by clock changes, and documents it. The schema only requires the properties in §4.2.

**Fallback** (only if no start-key source is reliable for some event path): `start_key := hash(pid, process creation time)`. It keeps the same stateless, cross-sensor properties but has a theoretical collision risk. If adopted, it gets its own domain tag (`atlas.process.v1-fallback`) so ids from the two formulas never mix silently.

## 5. Event Classes

### 5.0 OCSF version and IDs

OCSF 1.9.0. `type_uid = class_uid × 100 + activity_id` (OCSF requires producers to compute it this way). Every OCSF class also defines `0 = Unknown` and `99 = Other`; Atlas does **not** emit either (§6.1).

| Class | `class_uid` | `category_uid` | Activities used in v1 (`activity_id`) |
|---|---|---|---|
| Process Activity | 1007 | 1 | Launch 1, Terminate 2 |
| Module Activity | 1005 | 1 | Load 1 |
| Network Activity | 4001 | 4 | Open 1, Close 2 |
| File System Activity | 1001 | 1 | Create 1, Read 2, Update 3, Delete 4, Rename 5, Set Attributes 6 |
| Registry Key Activity (win extension) | 201001 | 1 | Create 1, Delete 4, Rename 5 |
| Registry Value Activity (win extension) | 201002 | 1 | Set 2, Delete 4 |
| DNS Activity | 4003 | 4 | Response 2 |

OCSF defines further activities, such as Process Inject, Network Refuse/Listen, Module Unload and Registry Key Modify. They are added later, as additive changes, when a sensor can produce them.

**Where Atlas's internal representation deliberately differs from OCSF** (the OCSF exporter converts each of these):
- `uid` fields are 16 bytes internally; OCSF uses strings, so the exporter renders lowercase hex.
- `time` is in ns internally; OCSF uses ms.
- `reg_value.type` uses the Windows `REG_*` constants internally. OCSF's `type_id` has its own numbering (e.g. OCSF `REG_SZ` = 10), so the exporter maps between them.
- `reg_value.data` is raw bytes internally; the exporter renders OCSF's typed `reg_*_data` fields.
- `query.type` is the numeric RR type internally; OCSF uses a string (`"AAAA"`).
- `protocol` / `direction` are Rust enums internally; the exporter emits `connection_info.protocol_name` / `direction_id`.

### 5.1 Shared objects

**`ProcessRef`** — the actor core, carried by every event as `actor.process`:

| Field | Type | Required |
|---|---|---|
| `uid` | 16 bytes | yes |
| `pid` | `u32` | yes |
| `file.path` | string | yes |
| `file.name` | string | yes |
| `user` | `User` | no (not always resolvable) |

**`User`**: `uid` = Windows SID string (stable identity), `name` = `DOMAIN\user`.

**`Process`** — full detail, used only in Process Launch (`process`):
- all `ProcessRef` fields, plus
- `cmd_line` (string, required; may be empty)
- `cmd_line_truncated` (bool)
- `created_time` (`i64` ns)
- `integrity` (enum `Untrusted | Low | Medium | High | System | Protected`; optional)
- `parent_process` (`ProcessRef`, optional: absent when the OS reports no parent, e.g. early-boot processes; the *claimed* parent — see §5.2)
- `file.hashes` (SHA-256; optional — sensor decides caching)
- `file.signature` (signer string + status enum `Valid | Invalid | Unsigned`; optional)

**`File`**: `path`, `name` (required); `hashes`, `signature` (optional).

**`NetworkEndpoint`**: `ip` (IPv4 or IPv6, stored as 4/16 bytes), `port` (`u16`).

### 5.2 Process Activity

| Activity | Fields |
|---|---|
| Launch | `process` (full `Process`), `actor.process` (the **creator**) |
| Terminate | `process` (`ProcessRef`), `exit_code` (`i32`, optional) |

`process.parent_process` is the parent PID as recorded by the OS, which can be spoofed (PPID spoofing). `actor.process` is the process that actually issued the creation. When they differ, that is itself a detection signal, though **not proof**. Legitimate mismatches exist (UAC elevation via the AppInfo service, WerFault), so rules in sub-project 3 need an allow-list.

### 5.3 Module Activity

| Activity | Fields |
|---|---|
| Load | `module.file` (`File`), `module.base_address` (`u64`), `actor.process` |

### 5.4 Network Activity

| Activity | Fields |
|---|---|
| Open | `src_endpoint`, `dst_endpoint`, `protocol` (enum `Tcp \| Udp`), `direction` (enum `Inbound \| Outbound`), `actor.process` |
| Close | as Open, plus `bytes_in` / `bytes_out` (`u64`, optional) |

### 5.5 File System Activity

| Activity | Fields |
|---|---|
| Create, Read, Update, Delete, SetAttributes | `file`, `actor.process` |
| Rename | `file` (original), `file_result` (new), `actor.process` |

Whether the sensor emits Read is a sub-project 1 decision; the schema defines it.

### 5.6 Registry Key Activity

| Activity | Fields |
|---|---|
| Create, Delete | `reg_key.path`, `actor.process` |
| Rename | `reg_key.path` (new), `prev_reg_key.path` (original), `actor.process` |

### 5.7 Registry Value Activity

| Activity | Fields |
|---|---|
| Set | `reg_value.path` (key path), `reg_value.name`, `reg_value.type` (enum of `REG_*` types; carries explicit presence on the wire because `REG_NONE` = 0, and absent is rejected as `Missing`), `reg_value.data` (bytes), `reg_value.data_truncated` (bool), `actor.process` |
| Delete | `reg_value.path`, `reg_value.name`, `actor.process` |

### 5.8 DNS Activity

| Activity | Fields |
|---|---|
| Response | `query.hostname`, `query.type` (`u16` RR type), `rcode` (`u16`, optional), `platform_status` (`u32`, optional), `answers[]` (`type: u16`, `data: string`), `actor.process` |

Only Response is modeled: the Windows DNS-Client ETW provider (event 3008) reports query and results together in one completion event.

`rcode` is the DNS response code. Windows does not report it directly: event 3008 gives a Win32/DNS status code (e.g. 9003 = name does not exist). The sensor maps known codes to `rcode` and always keeps the raw value in `platform_status`.

**Known limitation:** resolvers that bypass the Windows DNS client, such as a browser's built-in DNS-over-HTTPS, produce no DNS events.

### 5.9 Extension namespace

An `atlas` extension namespace is reserved for Atlas-specific fields with no OCSF equivalent. It is **empty in v1**.

## 6. Validation & Errors

### 6.1 Checks performed by wire → domain `TryFrom`

- **Structure:** `oneof kind` is set; the activity is valid for the class; every field required for that class/activity is present (§5).
- **Enums:** unknown enum values are **rejected** (not mapped to "Other"). Operational rule: **upgrade the server before agents.** Every proto enum has a mandatory proto3 zero value named `*_UNSPECIFIED`, and it is also rejected. An unset enum never silently becomes a real value.
- **Newer-schema data:** a class or activity this build does not know (an unknown oneof field) is rejected as `Missing` on `kind` / `activity`.
- **Identifiers:** `event_id` is a well-formed UUIDv7; every `uid` / `device.uid` is exactly 16 bytes.
- **Size limits** (an honest sensor truncates to fit and sets the matching `*_truncated` flag; the validator rejects anything over):

| Field | Limit |
|---|---|
| `cmd_line` | 64 KiB |
| any path | 32 KiB |
| `reg_value.data` | 4 KiB |
| `query.hostname`, `answers[].data` | 1 KiB each |
| `answers[]` | 64 entries |
| `user.uid` (SID) | 256 B |
| `user.name` | 1 KiB |
| `file.signature.signer` | 1 KiB |
| encoded event | 256 KiB (checked before decode) |

- **Text:** protobuf `string` must be valid UTF-8. Windows UTF-16 strings containing unpaired surrogates are converted lossily by the sensor (U+FFFD). **Known limitation:** an attacker can craft names that lose information in conversion; recorded for sub-project 3.

### 6.2 Error type

```rust
pub struct SchemaError { pub field_path: String, pub kind: SchemaErrorKind }
pub enum SchemaErrorKind { Missing, UnknownEnum, TooLarge, Malformed }
```

e.g. `process.file.path: TooLarge`. The crate reports; callers decide policy. Intended server policy (sub-project 2): reject the individual event, never the stream; count failures per agent; a sustained malformed rate from one host is a tamper signal.

## 7. Schema Evolution

- Within `atlas.events.v1`, changes are **additive only**: new fields, classes, enum values.
- Field numbers are never renumbered or reused; removed fields become `reserved`.
- Enforced in CI by `buf breaking` against `main` (wired in 0b).
- A breaking change creates package `atlas.events.v2`, run side-by-side with v1 during migration.
- No per-event schema-version field; the agent declares its schema version once per connection (sub-project 2).

## 8. Testing & Definition of Done

### 8.1 Tests

- **Round-trip property tests** (`proptest`): arbitrary valid domain `Event` → wire → domain equals the original.
- **Validator tests:** table-driven; one negative case per rule in §6.1, asserting exact `field_path` and `kind`.
- **Fuzzing** (`cargo-fuzz`): arbitrary bytes → decode → `TryFrom` must never panic, hang, or allocate unboundedly. Runs on Linux CI (crates are platform-neutral).
- **Golden fixtures:** one protobuf-JSON file per class/activity in `crates/atlas-schema/tests/fixtures/`, generated from the named samples with `ATLAS_UPDATE_FIXTURES=1` and reviewed by hand before commit; decoded and validated in tests; double as documentation.
- **OCSF ID table test:** derived `class_uid` / `activity_id` / `type_uid` equal the pinned OCSF values (§5.0).
- **`process.uid` test vectors:** fixed inputs → known outputs, so any other implementation can prove conformance.
- **Benchmarks** (`criterion`): encode, decode, validate throughput recorded as a baseline. Not a gate (performance budgets are sub-project 8).

### 8.2 Definition of done

1. `atlas-proto` and `atlas-schema` build in a minimal Cargo workspace; all seven classes are defined in both proto and domain.
2. All tests in §8.1 pass; the fuzz target runs clean for a fixed budget (10 minutes locally).
3. `docs/schema-reference.md` lists every class, field, and limit, with the golden examples.
4. Decision log and roadmap in `docs/architecture-overview.md` updated.

## 9. Verification Notes

Checked 2026-09-24 against schema.ocsf.io, the OCSF GitHub releases, Microsoft Learn, and reverse-engineering references (Geoff Chappell, the jdu2600 ETW manifest dump).

| Claim | Result |
|---|---|
| OCSF latest release | 1.9.0 (2026-08-03). |
| Class/activity IDs, `type_uid` formula | Confirmed (§5.0). |
| OCSF field names used in §5 | Confirmed. Corrections applied: registry key rename uses `prev_reg_key`; OCSF `query.type` is a string; OCSF registry `type_id` is not `REG_*`. |
| `PsGetProcessStartKey` | Documented export, Windows 10 1703+. |
| ETW start key | `EVENT_ENABLE_PROPERTY_PROCESS_START_KEY`, Windows 10 1507+. |
| Kernel-Process ProcessStart start key | v3+ has `ProcessSequenceNumber`. Whether it equals the start key is **unverified**, so sub-project 1 checks it in the VM. |
| Start-key uniqueness | At least unique per boot (Microsoft docs disagree on whether it is unique across boots). It is treated as opaque and combined with `boot_id`. |
| BootId | Registry value confirmed (a counter of successful boots). `KUSER_SHARED_DATA.BootId` is readable from both modes. |
| DNS-Client event 3008 | Query name, type, status and results are in one event. The status is a Win32 code, not an rcode. The requester PID in the header is widely relied on but not documented, so sub-project 1 verifies it. |
| PPID spoofing signal | Header PID is the creator and the payload has the claimed parent. Practitioner-confirmed, not Microsoft-documented. |

**Items handed to sub-project 1 for in-VM verification:** `ProcessSequenceNumber` vs start key, the `boot_time` source, the DNS 3008 header PID.
