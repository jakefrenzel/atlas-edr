# Atlas Event Schema Reference (`atlas.events.v1`)

The field-by-field reference for every Atlas event. The design rationale is in
[the 0a spec](specs/2026-09-24-event-schema-design.md). The sources of truth are the `.proto` files in
`crates/atlas-proto/proto/atlas/events/v1/` and the domain types in `crates/atlas-schema/src/`.

A worked example of every class and activity, in protobuf-JSON, is in
`crates/atlas-schema/tests/fixtures/` (for example `process_launch.json`). In protobuf-JSON, bytes are base64,
64-bit integers are strings, and enums are names.

Field paths below are the ones `SchemaError` reports.

## Envelope (every event)

| Path | Type | Required | Notes |
|---|---|---|---|
| `event_id` | 16 bytes | yes | UUIDv7 (RFC 9562 variant). Used to remove duplicates. |
| `time` | i64 | yes | Nanoseconds since the Unix epoch, UTC. |
| `sensor` | enum | yes | `ETW` or `DRIVER`. |
| `device.uid` | 16 bytes | yes | Random at agent install. The server checks it against the mTLS identity. |
| `device.boot_id` | 16 bytes | yes | Opaque per-boot id. |
| `kind` | oneof | yes | One of the seven classes below. |

## Process identity

`process.uid = BLAKE3("atlas.process.v1" ‖ device.uid ‖ boot_id ‖ start_key as u64 LE)[0..16]`, computed by
`atlas_schema::process_uid`. Conformance vectors (lowercase hex):

| device.uid | boot_id | start_key | process.uid |
|---|---|---|---|
| 16 × `00` | 16 × `00` | `0` | `8a01e78b10f07bca76e121414f3c9f00` |
| `000102…0f` | `101112…1f` | `0x0001_0000_0000_002a` | `b1e0e2e71592001f8cc1b00c7846432e` |

## Shared objects

**`ProcessRef`** (the actor core, carried as `actor.process` by every class):

| Field | Type | Required |
|---|---|---|
| `uid` | 16 bytes | yes |
| `pid` | u32 | yes |
| `file` | `File` | yes (path/name may be empty, e.g. the System process) |
| `user` | `User` | no |

**`Process`**: every `ProcessRef` field, plus `cmd_line` (string, may be empty), `cmd_line_truncated` (bool),
`created_time` (i64 ns), `integrity` (optional enum: Untrusted, Low, Medium, High, System, Protected), and
`parent_process` (optional `ProcessRef`, the parent *as the OS records it*).

**`File`**: `path`, `name` (strings), optional `hashes.sha256` (exactly 32 bytes), optional `signature`
(`signer` optional string, `status` enum: Valid, Invalid, Unsigned).

**`User`**: `uid` (SID string), `name` (`DOMAIN\user`).

**`NetworkEndpoint`**: `ip` (4 or 16 bytes), `port` (0–65535).

## Classes

| Class | OCSF `class_uid` | Category | Activities (`activity_id`) | Fixture(s) |
|---|---|---|---|---|
| Process Activity | 1007 | 1 System | Launch 1, Terminate 2 | `process_*.json` |
| Module Activity | 1005 | 1 System | Load 1 | `module_load.json` |
| Network Activity | 4001 | 4 Network | Open 1, Close 2 | `network_*.json` |
| File System Activity | 1001 | 1 System | Create 1, Read 2, Update 3, Delete 4, Rename 5, SetAttributes 6, Open 14 | `file_*.json` |
| Registry Key Activity | 201001 | 1 System | Create 1, Delete 4, Rename 5 | `registry_key_*.json` |
| Registry Value Activity | 201002 | 1 System | Set 2, Delete 4 | `registry_value_*.json` |
| DNS Activity | 4003 | 4 Network | Response 2 | `dns_response.json` |
| Event Log Activity | 1008 | 1 System | Stop 7, Restart 8, Disable 10 | `event_log_*.json` |
| Sensor Health (Atlas extension 500) | 50006001 | 6 Application Activity | Report 1 | `sensor_health_report.json` |

`type_uid = class_uid × 100 + activity_id`. Call `EventKind::ocsf_ids()` to get all four numbers. Sensor Health's
`type_uid` is 5000600101, so it needs a 64-bit integer.

### Process Activity

| Activity | Fields |
|---|---|
| Launch | `actor.process` (the real creator), `process` (full `Process`) |
| Terminate | `process` (`ProcessRef`), `exit_code` (optional i32) |

A mismatch between `actor.process` and `process.parent_process` suggests PPID spoofing, but it isn't proof:
UAC elevation and WerFault produce mismatches legitimately.

### Module Activity

| Activity | Fields |
|---|---|
| Load | `actor.process`, `module.file`, `base_address` (u64) |

### Network Activity

All activities: `actor.process`, `src_endpoint`, `dst_endpoint`, `protocol` (TCP/UDP), `direction`
(Inbound/Outbound).

| Activity | Extra fields |
|---|---|
| Open | none |
| Close | `bytes_in`, `bytes_out` (optional u64) |

### File System Activity

All activities: `actor.process`, `file` (for Rename, the original).

| Activity | Extra fields |
|---|---|
| Rename | `file_result` (the file after the rename) |
| Open | none. A handle was opened on a watchlisted path: access intent, not proof of a read. |
| all others | none |

### Registry Key Activity

All activities: `actor.process`, `reg_key.path` (for Rename, the new path), and `path_unresolved` (bool). When
`path_unresolved` is true, `reg_key.path` holds only what the sensor saw (a relative name, or empty), not a full path.

| Activity | Extra fields |
|---|---|
| Rename | `prev_reg_key.path` (the original path) |
| Create, Delete | none |

### Registry Value Activity

All activities: `actor.process`, `reg_value.path` (containing key), `reg_value.name` (empty means the default value),
and `path_unresolved` (bool, as for keys, about `reg_value.path`).

| Activity | Extra fields |
|---|---|
| Set | `reg_value.type` (Windows `REG_*` constant 0–11, **not** the OCSF `type_id`), or instead `raw_type` (u32 above 11): exactly one is present, and neither is `reg_value.type: Missing` rather than `REG_NONE`. `reg_value.data` (raw bytes), `data_truncated`, `data_read_after` (bool: the data was read from the registry after the event, not captured with it), `data_unavailable` (bool: no data was obtained; `data` is then empty and `data_truncated` false) |
| Delete | none |

### DNS Activity

| Activity | Fields |
|---|---|
| Response | `actor.process`, `query.hostname`, `query.type` (numeric RR type), `rcode` (optional u16), `platform_status` (optional u32, raw Windows status), `answers[]` (`type` u16, `data` string) |

### Event Log Activity

Tampering seen on the agent's own ETW sessions (sensor spec §9).

| Field | Type | Required |
|---|---|---|
| `actor.process` | `ProcessRef` | no (the watchdog sees the effect, not who caused it) |
| `log_name` | string ≤ 256 B | yes, non-empty: the session name |
| `log_provider` | string ≤ 256 B | non-empty for Disable: the provider name |
| `status_code` | u32 | no: the Win32 error or NTSTATUS that revealed the change |

| Activity | Meaning |
|---|---|
| Stop | A session was found stopped, or replaced by another session with its name. |
| Restart | The agent recreated it. |
| Disable | A provider was disabled or changed in the session, or its canary went silent. |

### Sensor Health

The agent's own loss, quality, housekeeping, resource and buffer figures, every 60 s (sensor spec §9.3). No actor.
`interval_start` (i64 ns) starts the interval; the event's `time` ends it. One activity, Report, with five groups
(field lists in `sensor_health.proto`) and an optional gap:

| Group | Holds |
|---|---|
| `loss` | ETW events and buffers lost per session, queue drops, DNS rate-limit drops, `actor_dropped` per class, buffer backlog drops, events skipped because the ETW callback panicked |
| `quality` | late arrivals, parse errors, unknown versions, join misses, `actor_unresolved` per class, unresolved file objects and registry paths, value-read failures, invalid buffer records, enrichment misses, ambiguous registry value names |
| `housekeeping` | evictions from every bounded map and cache, retention evictions, dropped failed file operations, seeding results |
| `resources` | agent CPU time in the interval (ns), working set (bytes) |
| `buffer` | write errors, recoveries, `failing`, rejected records, segments that could not be deleted, startup recovery (truncated bytes, cursor reset, foreign segments), corrupt segments, disk bytes |
| `gap` | events deleted from the buffer before delivery: `events`, and `first_time` / `last_time` (both or neither, ordered) |

Counters count occurrences during the interval; gauges (`failing`, stuck helpers, negative-cache size, working set,
disk bytes) are sampled at its end. An absent field was not measured; `0` means measured and none occurred.
`actor_dropped` / `actor_unresolved` are lists of `{class_uid, count}`, at most 32 entries.

## The `atlas` extension namespace

Fields with no OCSF equivalent, which exporters render under `atlas`: `atlas.path_unresolved` (both registry
classes), `atlas.data_read_after`, `atlas.data_unavailable` and `atlas.raw_type` (Registry Value Set). Sensor Health
is an Atlas extension *class*: extension uid 500, not registered with OCSF (decision log, 2026-10-04).

## Limits

Sensors truncate to fit and set the matching `*_truncated` flag, using `atlas_schema::limits::truncate_utf8`.
The validator rejects anything over a limit with `TooLarge`.

| Field | Limit |
|---|---|
| `cmd_line` | 64 KiB |
| any path or name (`file.path`, `file.name`, registry paths, `reg_value.name`) | 32 KiB |
| `reg_value.data` | 4 KiB |
| `query.hostname`, each `answers[].data` | 1 KiB |
| `answers[]` | 64 entries |
| `user.uid` (SID) | 256 B |
| `user.name` | 1 KiB |
| `file.signature.signer` | 1 KiB |
| `log_name`, `log_provider` | 256 B |
| `actor_dropped`, `actor_unresolved` (Sensor Health) | 32 entries each |
| whole encoded event | 256 KiB (checked before decoding) |

## Validation errors

`SchemaError { field_path, kind }`, displayed as `process.file.path: TooLarge`.

| Kind | Meaning |
|---|---|
| `Missing` | Required field or oneof absent, or an enum set to `*_UNSPECIFIED`. A class or activity from a newer schema also shows up as `kind`/`activity` Missing. |
| `UnknownEnum` | Enum number this build does not know (`reg_value.type` > 11 too). |
| `TooLarge` | Over a limit above. The whole-event limit is reported at path `event`. |
| `Malformed` | Wrong byte length, out-of-range number, not a UUIDv7, or undecodable protobuf (including invalid UTF-8). The last two are reported at path `event`. |

Rules added by sub-project 1 (paths as reported):

| Path | Kind | When |
|---|---|---|
| `reg_value.raw_type` | Malformed | `raw_type` is 0–11, or set together with `type` |
| `reg_value.data_unavailable` | Malformed | set while `data` is non-empty or `data_truncated` is true |
| `log_name` | Missing / TooLarge | empty / over 256 B |
| `log_provider` | Missing / TooLarge | empty on Disable / over 256 B |
| `loss.actor_dropped`, `quality.actor_unresolved` | TooLarge | more than 32 entries |
| `gap.last_time` | Malformed | before `gap.first_time` |
| `gap` | Malformed | only one of the two times present |

Operational rule: **upgrade the server before agents**, because newer agent events are rejected rather than guessed at.

## Evolution

Within `atlas.events.v1`, changes are additive only: new fields, classes and enum values. Field numbers are never
reused, and removed fields become `reserved`. A breaking change means a new `atlas.events.v2`.

## Known limitations

- Windows strings with unpaired UTF-16 surrogates are converted lossily (U+FFFD) by the sensor.
- DNS resolvers that bypass the Windows DNS client (e.g. a browser's own DNS-over-HTTPS) produce no DNS events.
- Registry value data is read by the sensor after the event (`data_read_after`), so a value changed at once is
  reported in its new state or as unavailable (sensor spec §16).

## Performance baseline

Recorded 2026-09-24 on the dev PC (release-mode `cargo bench`, 17 mixed samples; see `crates/atlas-schema/benches/codec.rs`).
It is not a gate. Performance budgets belong to sub-project 8.

| Benchmark | Throughput |
|---|---|
| `encode` | ~1.48 M events/s |
| `decode_and_validate` | ~185 K events/s |
| `protobuf_decode_only` | ~588 K events/s |

Validation currently costs about twice as much as raw protobuf decoding. That's worth profiling in sub-project 8.

Fuzzing: pending. The `decode_event` cargo-fuzz target (`crates/atlas-schema/fuzz/`) is committed and compiles, but the
10-minute nightly run moves to sub-project 0b's CI. Until then, the stable hostile-input property tests
(15,000 random, corrupted and truncated inputs per run) guard the decoder.
