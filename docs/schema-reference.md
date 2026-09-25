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
| File System Activity | 1001 | 1 System | Create 1, Read 2, Update 3, Delete 4, Rename 5, SetAttributes 6 | `file_*.json` |
| Registry Key Activity | 201001 | 1 System | Create 1, Delete 4, Rename 5 | `registry_key_*.json` |
| Registry Value Activity | 201002 | 1 System | Set 2, Delete 4 | `registry_value_*.json` |
| DNS Activity | 4003 | 4 Network | Response 2 | `dns_response.json` |

`type_uid = class_uid × 100 + activity_id`. Call `EventKind::ocsf_ids()` to get all four numbers.

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
| all others | none |

### Registry Key Activity

All activities: `actor.process`, `reg_key.path` (for Rename, the new path).

| Activity | Extra fields |
|---|---|
| Rename | `prev_reg_key.path` (the original path) |
| Create, Delete | none |

### Registry Value Activity

All activities: `actor.process`, `reg_value.path` (containing key), `reg_value.name` (empty means the default value).

| Activity | Extra fields |
|---|---|
| Set | `reg_value.type` (Windows `REG_*` constant 0–11, **not** the OCSF `type_id`), `reg_value.data` (raw bytes), `data_truncated` |
| Delete | none |

### DNS Activity

| Activity | Fields |
|---|---|
| Response | `actor.process`, `query.hostname`, `query.type` (numeric RR type), `rcode` (optional u16), `platform_status` (optional u32, raw Windows status), `answers[]` (`type` u16, `data` string) |

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
| whole encoded event | 256 KiB (checked before decoding) |

## Validation errors

`SchemaError { field_path, kind }`, displayed as `process.file.path: TooLarge`.

| Kind | Meaning |
|---|---|
| `Missing` | Required field or oneof absent, or an enum set to `*_UNSPECIFIED`. A class or activity from a newer schema also shows up as `kind`/`activity` Missing. |
| `UnknownEnum` | Enum number this build does not know (`reg_value.type` > 11 too). |
| `TooLarge` | Over a limit above. The whole-event limit is reported at path `event`. |
| `Malformed` | Wrong byte length, out-of-range number, not a UUIDv7, or undecodable protobuf (including invalid UTF-8). The last two are reported at path `event`. |

Operational rule: **upgrade the server before agents**, because newer agent events are rejected rather than guessed at.

## Evolution

Within `atlas.events.v1`, changes are additive only: new fields, classes and enum values. Field numbers are never
reused, and removed fields become `reserved`. A breaking change means a new `atlas.events.v2`.

## Known limitations

- Windows strings with unpaired UTF-16 surrogates are converted lossily (U+FFFD) by the sensor.
- DNS resolvers that bypass the Windows DNS client (e.g. a browser's own DNS-over-HTTPS) produce no DNS events.
- Items for sub-project 1 to verify in the VM: whether `ProcessSequenceNumber` equals the start key, the
  `boot_time` source, and the header PID in DNS event 3008.

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
