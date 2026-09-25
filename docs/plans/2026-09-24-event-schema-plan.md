# Sub-project 0a — Event Schema Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `atlas-proto` and `atlas-schema` crates. Together they define all seven v1 Atlas event classes as protobuf (the wire contract) and as typed Rust domain types, joined by a validating conversion layer, with tests, fuzzing, benchmarks and a reference doc.

**Architecture:** `atlas-proto` holds the `.proto` files, compiled at build time by `protox` + `prost-build`; it contains no hand-written logic. `atlas-schema` holds the domain model:
- one module per OCSF class
- infallible `From<domain> for wire`
- validating `from_wire` functions that report the exact field path of any error
- a top-level `TryFrom<wire::Event> for Event`, plus `encode_event` / `decode_event` entry points

OCSF ids are derived from the domain types, never stored.

**Tech Stack:** Rust 1.97 (edition 2024, MSVC). Crates: prost 0.14, prost-build 0.14, protox 0.9, blake3 1.8, uuid 1.26 (`v7`), thiserror 2, proptest 1.11, prost-reflect 0.16 (`serde`), serde_json 1, criterion 0.8, and libfuzzer-sys 0.4 (fuzz crate only).

**Spec:** `docs/specs/2026-09-24-event-schema-design.md`. Read it before starting. Section numbers below (§) refer to it.

**Verification note:** All code in this plan was compiled and tested in a scratch workspace on 2026-09-24:
- clippy is clean
- 35 unit tests, all integration tests and the benchmark pass
- the `process.uid` test vectors are real outputs

The fuzz target compiles but has not been run yet (Docker Desktop was not running at plan time).

## Global Constraints

- Toolchain: stable Rust `1.97` (`rust-version = "1.97"`), edition `2024`, `x86_64-pc-windows-msvc`. Nightly is used only inside a Docker container for `cargo fuzz`.
- No `protoc` binary: protobuf is compiled by `protox` in `build.rs`.
- Protobuf package: `atlas.events.v1`. Every enum's zero value is `*_UNSPECIFIED`.
- OCSF version: **1.9.0**. Numeric ids exactly as in §5.0 (`type_uid = class_uid × 100 + activity_id`).
- `process.uid = BLAKE3("atlas.process.v1" ‖ device.uid[16] ‖ boot_id[16] ‖ start_key u64 LE)[0..16]`.
- Limits (UTF-8 bytes / counts): `cmd_line` 64 KiB; any path or name 32 KiB; `reg_value.data` 4 KiB; `query.hostname` and each `answers[].data` 1 KiB; `answers[]` 64; whole encoded event 256 KiB.
- Domain → wire is infallible. Wire → domain rejects:
  - an unset enum (`*_UNSPECIFIED`) as `Missing`
  - an unknown enum number as `UnknownEnum`
  - oversize values as `TooLarge`
  - bad lengths, ranges, UUID versions and undecodable bytes as `Malformed`
- Error field paths use domain names (`process.file.path`, `answers[2].data`, `reg_value.type`). Whole-buffer errors use `event`.
- Formatting: the repo `rustfmt.toml` (`max_width = 120`, `use_small_heuristics = "Max"`). Before the final commit: `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` must pass.
- Shell: commands are given for PowerShell (the project's primary shell). `cargo` commands are identical in bash.
- Every commit message ends with:
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TqnCZsFoaabfHYyb1ieVfc
  ```

## Review Focus

The spec implies these five inputs, but no single class's round-trip would exercise them. Each one has a pinned test in the task that owns the code:

1. **An event from a newer agent carrying a class this build doesn't know** (oneof field 17+) must be rejected as `kind: Missing`, not accepted or panicked on. See Task 7, `class_from_a_newer_schema_is_rejected_as_missing_kind`.
2. **An event from a newer agent carrying an unknown activity inside a known class** must be rejected as `activity: Missing`. See Task 7, `activity_from_a_newer_schema_is_rejected_as_missing_activity`.
3. **Invalid UTF-8 inside a string field**, which could come from a hostile agent or from lossy Windows name conversion done wrong, must yield `event: Malformed`, never a panic. See Task 7, `invalid_utf8_in_a_string_is_malformed`.
4. **Legitimately empty strings** (the System process has no image path; some processes have an empty command line) must be accepted, since proto3 cannot tell empty from absent. See Task 7, `empty_strings_are_allowed`.
5. **A value exactly at a limit** must be accepted, and one byte over rejected. Sensor truncation must never split a UTF-8 character. See Task 7, `values_exactly_at_limits_are_accepted`, and Task 2, `never_splits_a_multibyte_char`.

## Deliberate clarifications of the spec (applied to the spec text in Task 11)

- **Wire layout:** `meta` is flattened into `Event` fields 1–3. Activity-specific fields live in per-activity oneof sub-messages, which mirror the domain enums. Class-wide fields (`actor`, `file`, endpoints, …) sit on the class message.
- **`process.parent_process` is optional.** Processes from early boot have no parent. The PPID-spoofing comparison applies only when it is present.
- **An enum left at `*_UNSPECIFIED` reports `Missing`.** A number this build doesn't know reports `UnknownEnum`. A class or activity from a newer schema reports `Missing` on `kind` / `activity`.
- **Golden fixtures are generated, then reviewed.** They come from the named samples via `ATLAS_UPDATE_FIXTURES=1`, and a human reviews the generated files before committing. This avoids typos from hand-writing base64.
- **Stable-Rust hostile-input property tests** run on every `cargo test`, in addition to the `cargo-fuzz` target.

## File Structure

```
Cargo.toml                         workspace root, shared dependency versions
rustfmt.toml                       formatting rules
.gitignore
crates/atlas-proto/
  Cargo.toml
  build.rs                         protox → prost-build; also writes the descriptor set
  src/lib.rs                       `pub mod v1` (generated) + FILE_DESCRIPTOR_SET
  proto/atlas/events/v1/
    objects.proto                  User, Hashes, Signature, File, ProcessRef, Process, NetworkEndpoint
    process.proto module.proto network.proto file.proto registry.proto dns.proto
    event.proto                    Sensor, Device, Event envelope (oneof over 7 classes)
  tests/smoke.rs
crates/atlas-schema/
  Cargo.toml
  src/lib.rs                       public API re-exports
  src/error.rs                     SchemaError, SchemaErrorKind
  src/limits.rs                    size limits + truncate_utf8
  src/ids.rs                       DeviceUid, BootId, ProcessUid, EventId, process_uid()
  src/convert.rs                   pub(crate) validation helpers (paths, bounds, enums)
  src/objects.rs                   shared objects + conversions (+ test_support)
  src/classes/mod.rs
  src/classes/{process,module,network,file,registry,dns}.rs   one OCSF class each
  src/event.rs                     Event, EventMeta, Sensor, Device, EventKind; TryFrom gate
  src/ocsf.rs                      OcsfIds derivation
  src/codec.rs                     encode_event / decode_event
  tests/common/mod.rs              named samples + proptest strategies
  tests/roundtrip.rs tests/validation.rs tests/ocsf_ids.rs tests/hostile_input.rs tests/golden.rs
  tests/fixtures/*.json            17 golden protobuf-JSON events
  benches/codec.rs
  fuzz/                            standalone cargo-fuzz crate (nightly, run in Docker)
docs/schema-reference.md
```

---

### Task 1: Workspace skeleton and `atlas-proto`

**Files:**
- Create: `Cargo.toml`, `rustfmt.toml`, `.gitignore`
- Create: `crates/atlas-proto/Cargo.toml`, `crates/atlas-proto/build.rs`, `crates/atlas-proto/src/lib.rs`
- Create: `crates/atlas-proto/proto/atlas/events/v1/{objects,process,module,network,file,registry,dns,event}.proto`
- Test: `crates/atlas-proto/tests/smoke.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `atlas_proto::v1::*`, the prost-generated types. Messages are `Event`, `Device`, `ProcessActivity`, `ProcessLaunch`, `ProcessTerminate`, `ModuleActivity`, `ModuleLoad`, `NetworkActivity`, `NetworkOpen`, `NetworkClose`, `FileSystemActivity`, `FileCreate`/`FileRead`/`FileUpdate`/`FileDelete`/`FileRename`/`FileSetAttributes`, `RegistryKeyActivity`, `RegistryKeyCreate`/`Delete`/`Rename`, `RegistryValueActivity`, `RegistryValueSet`/`Delete`, `DnsActivity`, `DnsResponse`, `DnsAnswer`, `User`, `Hashes`, `Signature`, `File`, `ProcessRef`, `Process` and `NetworkEndpoint`.
  - Enums: `Sensor`, `SignatureStatus`, `Integrity`, `NetworkProtocol`, `NetworkDirection`.
  - Oneof modules: `event::Kind`, and `<class>_activity::Activity`, e.g. `process_activity::Activity::Launch`.
  - Prost renames the proto field `type` to `r#type`.
  - `atlas_proto::FILE_DESCRIPTOR_SET: &[u8]`.

- [ ] **Step 1: Create the workspace root files**

`Cargo.toml`:
```toml
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
edition = "2024"
rust-version = "1.97"
license = "MIT OR Apache-2.0"
publish = false

[workspace.dependencies]
atlas-proto = { path = "crates/atlas-proto" }
blake3 = "1.8"
criterion = "0.8"
prost = "0.14"
prost-build = "0.14"
prost-reflect = { version = "0.16", features = ["serde"] }
proptest = "1.11"
protox = "0.9"
serde_json = "1"
thiserror = "2"
uuid = { version = "1.26", features = ["v7"] }
```

`rustfmt.toml`:
```toml
max_width = 120
use_small_heuristics = "Max"
```

`.gitignore`:
```text
/target
**/fuzz/target
**/fuzz/corpus
**/fuzz/artifacts
```

- [ ] **Step 2: Create the crate manifest and the failing smoke test**

`crates/atlas-proto/Cargo.toml`:
```toml
[package]
name = "atlas-proto"
version = "0.1.0"
description = "Generated protobuf types for the Atlas event wire contract (atlas.events.v1)."
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
prost.workspace = true

[build-dependencies]
prost.workspace = true
prost-build.workspace = true
protox.workspace = true
```

`crates/atlas-proto/tests/smoke.rs`:
```rust
//! The generated types exist, encode, and decode; the descriptor set is embedded.

use atlas_proto::v1;
use prost::Message;

#[test]
fn event_round_trips_through_bytes() {
    let event = v1::Event {
        event_id: vec![1; 16],
        time: 5,
        sensor: v1::Sensor::Etw as i32,
        device: Some(v1::Device { uid: vec![2; 16], boot_id: vec![3; 16] }),
        kind: Some(v1::event::Kind::Dns(v1::DnsActivity {
            hostname: "example.com".into(),
            query_type: 28,
            ..Default::default()
        })),
    };
    let bytes = event.encode_to_vec();
    assert_eq!(v1::Event::decode(bytes.as_slice()).unwrap(), event);
}

#[test]
fn descriptor_set_is_embedded() {
    let fds = atlas_proto::FILE_DESCRIPTOR_SET;
    assert!(fds.windows(b"atlas.events.v1".len()).any(|w| w == b"atlas.events.v1"));
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p atlas-proto`
Expected: FAIL. Cargo can't build because the crate has no `src/lib.rs` (a "no targets specified"/"can't find library" error), or with `unresolved import atlas_proto`.

- [ ] **Step 4: Write the proto files**

`crates/atlas-proto/proto/atlas/events/v1/objects.proto`:
```proto
// Shared objects used by several event classes.
// Modeled on OCSF 1.9.0 objects; see docs/specs/2026-09-24-event-schema-design.md section 5.1.
syntax = "proto3";

package atlas.events.v1;

// OCSF `user`.
message User {
  // Windows SID string, e.g. "S-1-5-21-...". Stable identity.
  string uid = 1;
  // "DOMAIN\\user".
  string name = 2;
}

// OCSF `fingerprint` subset.
message Hashes {
  // SHA-256 digest, exactly 32 bytes when present.
  optional bytes sha256 = 1;
}

enum SignatureStatus {
  SIGNATURE_STATUS_UNSPECIFIED = 0;
  SIGNATURE_STATUS_VALID = 1;
  SIGNATURE_STATUS_INVALID = 2;
  SIGNATURE_STATUS_UNSIGNED = 3;
}

// OCSF `digital_signature` subset.
message Signature {
  // Signer subject; absent when unsigned.
  optional string signer = 1;
  SignatureStatus status = 2;
}

// OCSF `file`.
message File {
  string path = 1;
  string name = 2;
  // Optional enrichment; the sensor decides when to compute it.
  optional Hashes hashes = 3;
  optional Signature signature = 4;
}

// The actor core carried by every event (spec D5). OCSF `process` subset.
message ProcessRef {
  // 16-byte process uid: BLAKE3("atlas.process.v1" || device.uid || boot_id || start_key)[0..16].
  bytes uid = 1;
  uint32 pid = 2;
  // Executable; only path and name are expected here.
  File file = 3;
  optional User user = 4;
}

enum Integrity {
  INTEGRITY_UNSPECIFIED = 0;
  INTEGRITY_UNTRUSTED = 1;
  INTEGRITY_LOW = 2;
  INTEGRITY_MEDIUM = 3;
  INTEGRITY_HIGH = 4;
  INTEGRITY_SYSTEM = 5;
  INTEGRITY_PROTECTED = 6;
}

// Full process detail. Only carried by Process Launch.
message Process {
  bytes uid = 1;
  uint32 pid = 2;
  File file = 3;
  optional User user = 4;
  string cmd_line = 5;
  bool cmd_line_truncated = 6;
  // Nanoseconds since the Unix epoch, UTC.
  int64 created_time = 7;
  optional Integrity integrity = 8;
  // The parent as recorded by the OS (can be spoofed; compare with the Launch actor).
  ProcessRef parent_process = 9;
}

// OCSF `network_endpoint` subset.
message NetworkEndpoint {
  // 4 bytes (IPv4) or 16 bytes (IPv6), network byte order.
  bytes ip = 1;
  // 0..=65535.
  uint32 port = 2;
}
```

`crates/atlas-proto/proto/atlas/events/v1/process.proto`:
```proto
// OCSF Process Activity (class_uid 1007).
syntax = "proto3";

package atlas.events.v1;

import "atlas/events/v1/objects.proto";

message ProcessActivity {
  oneof activity {
    ProcessLaunch launch = 1;       // activity_id 1
    ProcessTerminate terminate = 2; // activity_id 2
  }
}

message ProcessLaunch {
  // The process that actually issued the creation (the real creator).
  ProcessRef actor = 1;
  // The new process, full detail.
  Process process = 2;
}

message ProcessTerminate {
  ProcessRef process = 1;
  optional int32 exit_code = 2;
}
```

`crates/atlas-proto/proto/atlas/events/v1/module.proto`:
```proto
// OCSF Module Activity (class_uid 1005).
syntax = "proto3";

package atlas.events.v1;

import "atlas/events/v1/objects.proto";

message ModuleActivity {
  ProcessRef actor = 1;
  oneof activity {
    ModuleLoad load = 2; // activity_id 1
  }
}

message ModuleLoad {
  File file = 1;
  uint64 base_address = 2;
}
```

`crates/atlas-proto/proto/atlas/events/v1/network.proto`:
```proto
// OCSF Network Activity (class_uid 4001).
syntax = "proto3";

package atlas.events.v1;

import "atlas/events/v1/objects.proto";

enum NetworkProtocol {
  NETWORK_PROTOCOL_UNSPECIFIED = 0;
  NETWORK_PROTOCOL_TCP = 1;
  NETWORK_PROTOCOL_UDP = 2;
}

enum NetworkDirection {
  NETWORK_DIRECTION_UNSPECIFIED = 0;
  NETWORK_DIRECTION_INBOUND = 1;
  NETWORK_DIRECTION_OUTBOUND = 2;
}

message NetworkActivity {
  ProcessRef actor = 1;
  NetworkEndpoint src_endpoint = 2;
  NetworkEndpoint dst_endpoint = 3;
  NetworkProtocol protocol = 4;
  NetworkDirection direction = 5;
  oneof activity {
    NetworkOpen open = 6;   // activity_id 1
    NetworkClose close = 7; // activity_id 2
  }
}

message NetworkOpen {}

message NetworkClose {
  optional uint64 bytes_in = 1;
  optional uint64 bytes_out = 2;
}
```

`crates/atlas-proto/proto/atlas/events/v1/file.proto`:
```proto
// OCSF File System Activity (class_uid 1001).
syntax = "proto3";

package atlas.events.v1;

import "atlas/events/v1/objects.proto";

message FileSystemActivity {
  ProcessRef actor = 1;
  // For Rename: the original file.
  File file = 2;
  oneof activity {
    FileCreate create = 3;                // activity_id 1
    FileRead read = 4;                    // activity_id 2
    FileUpdate update = 5;                // activity_id 3
    FileDelete delete = 6;                // activity_id 4
    FileRename rename = 7;                // activity_id 5
    FileSetAttributes set_attributes = 8; // activity_id 6
  }
}

message FileCreate {}
message FileRead {}
message FileUpdate {}
message FileDelete {}

message FileRename {
  // The file after the rename (OCSF `file_result`).
  File file_result = 1;
}

message FileSetAttributes {}
```

`crates/atlas-proto/proto/atlas/events/v1/registry.proto`:
```proto
// OCSF Windows extension: Registry Key Activity (201001) and Registry Value Activity (201002).
syntax = "proto3";

package atlas.events.v1;

import "atlas/events/v1/objects.proto";

message RegistryKeyActivity {
  ProcessRef actor = 1;
  // OCSF `reg_key.path`. For Rename: the new path.
  string path = 2;
  oneof activity {
    RegistryKeyCreate create = 3; // activity_id 1
    RegistryKeyDelete delete = 4; // activity_id 4
    RegistryKeyRename rename = 5; // activity_id 5
  }
}

message RegistryKeyCreate {}
message RegistryKeyDelete {}

message RegistryKeyRename {
  // OCSF `prev_reg_key.path`: the original path.
  string prev_path = 1;
}

message RegistryValueActivity {
  ProcessRef actor = 1;
  // OCSF `reg_value.path`: the containing key.
  string key_path = 2;
  // OCSF `reg_value.name`. Empty string means the default value of the key.
  string name = 3;
  oneof activity {
    RegistryValueSet set = 4;       // activity_id 2
    RegistryValueDelete delete = 5; // activity_id 4
  }
}

message RegistryValueSet {
  // Windows REG_* constant (REG_NONE = 0 .. REG_QWORD = 11). Not an OCSF type_id.
  uint32 type = 1;
  bytes data = 2;
  bool data_truncated = 3;
}

message RegistryValueDelete {}
```

`crates/atlas-proto/proto/atlas/events/v1/dns.proto`:
```proto
// OCSF DNS Activity (class_uid 4003).
syntax = "proto3";

package atlas.events.v1;

import "atlas/events/v1/objects.proto";

message DnsActivity {
  ProcessRef actor = 1;
  // OCSF `query.hostname`.
  string hostname = 2;
  // Numeric DNS RR type (e.g. 28 = AAAA). 0..=65535.
  uint32 query_type = 3;
  oneof activity {
    DnsResponse response = 4; // activity_id 2
  }
}

message DnsAnswer {
  // Numeric DNS RR type. 0..=65535.
  uint32 type = 1;
  string data = 2;
}

message DnsResponse {
  // DNS response code, when the sensor could map the platform status. 0..=65535.
  optional uint32 rcode = 1;
  // Raw platform status (Windows: Win32/DNS status from DNS-Client event 3008).
  optional uint32 platform_status = 2;
  repeated DnsAnswer answers = 3;
}
```

`crates/atlas-proto/proto/atlas/events/v1/event.proto`:
```proto
// The Atlas event envelope. The package version is part of the contract:
// within atlas.events.v1 all changes are additive only (spec section 7).
syntax = "proto3";

package atlas.events.v1;

import "atlas/events/v1/dns.proto";
import "atlas/events/v1/file.proto";
import "atlas/events/v1/module.proto";
import "atlas/events/v1/network.proto";
import "atlas/events/v1/process.proto";
import "atlas/events/v1/registry.proto";

enum Sensor {
  SENSOR_UNSPECIFIED = 0;
  SENSOR_ETW = 1;
  SENSOR_DRIVER = 2;
}

message Device {
  // 16-byte random id generated at agent install.
  bytes uid = 1;
  // 16-byte opaque per-boot id.
  bytes boot_id = 2;
}

message Event {
  // 16-byte UUIDv7.
  bytes event_id = 1;
  // Nanoseconds since the Unix epoch, UTC.
  int64 time = 2;
  Sensor sensor = 3;
  Device device = 4;
  oneof kind {
    ProcessActivity process = 10;
    ModuleActivity module = 11;
    NetworkActivity network = 12;
    FileSystemActivity file = 13;
    RegistryKeyActivity registry_key = 14;
    RegistryValueActivity registry_value = 15;
    DnsActivity dns = 16;
  }
}
```

- [ ] **Step 5: Write `build.rs` and `src/lib.rs`**

`crates/atlas-proto/build.rs`:
```rust
use std::{env, fs, path::PathBuf};

const PROTO_ROOT: &str = "proto";
const PROTO_FILES: &[&str] = &[
    "atlas/events/v1/objects.proto",
    "atlas/events/v1/process.proto",
    "atlas/events/v1/module.proto",
    "atlas/events/v1/network.proto",
    "atlas/events/v1/file.proto",
    "atlas/events/v1/registry.proto",
    "atlas/events/v1/dns.proto",
    "atlas/events/v1/event.proto",
];

fn main() {
    println!("cargo:rerun-if-changed={PROTO_ROOT}");

    // protox is a pure-Rust protobuf compiler: no protoc binary needed.
    let fds = protox::compile(PROTO_FILES, [PROTO_ROOT]).expect("failed to compile .proto files");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    // Keep the descriptor set so tests can do protobuf-JSON via prost-reflect.
    fs::write(out_dir.join("atlas_events_v1.fds.bin"), prost::Message::encode_to_vec(&fds))
        .expect("failed to write descriptor set");

    prost_build::Config::new().compile_fds(fds).expect("failed to generate Rust code");
}
```

`crates/atlas-proto/src/lib.rs`:
```rust
//! Generated protobuf types for the Atlas event wire contract.
//!
//! Nothing in this crate is hand-written logic. The `.proto` files under
//! `proto/atlas/events/v1/` are the source of truth; see
//! `docs/specs/2026-09-24-event-schema-design.md`.

/// `atlas.events.v1` — generated by prost.
#[allow(clippy::all)]
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/atlas.events.v1.rs"));
}

/// Encoded `FileDescriptorSet` for `atlas.events.v1` (used for protobuf-JSON in tests and tools).
pub const FILE_DESCRIPTOR_SET: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/atlas_events_v1.fds.bin"));
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p atlas-proto`
Expected: PASS. Both `event_round_trips_through_bytes` and `descriptor_set_is_embedded` pass. The first build downloads crates and takes a minute or two.

- [ ] **Step 7: Commit**

```powershell
git add Cargo.toml rustfmt.toml .gitignore Cargo.lock crates/atlas-proto
git commit -m "feat(atlas-proto): add atlas.events.v1 protobuf contract" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01TqnCZsFoaabfHYyb1ieVfc"
```

---

### Task 2: `atlas-schema` foundation: errors, limits, identifiers

**Files:**
- Create: `crates/atlas-schema/Cargo.toml`, `crates/atlas-schema/src/lib.rs`
- Create: `crates/atlas-schema/src/error.rs`, `crates/atlas-schema/src/limits.rs`, `crates/atlas-schema/src/ids.rs` (each with inline `#[cfg(test)]` tests)

**Interfaces:**
- Consumes: nothing from Task 1 yet (the manifest declares `atlas-proto` for later tasks).
- Produces:
  - `SchemaError { pub field_path: String, pub kind: SchemaErrorKind }` with `SchemaError::new(impl Into<String>, SchemaErrorKind)`. It displays as `"{path}: {kind:?}"`.
  - `SchemaErrorKind::{Missing, UnknownEnum, TooLarge, Malformed}`.
  - `limits::{CMD_LINE_MAX, PATH_MAX, REG_DATA_MAX, DNS_HOSTNAME_MAX, DNS_ANSWER_DATA_MAX, DNS_ANSWERS_MAX, EVENT_MAX}` (`usize`), and `limits::truncate_utf8(&str, usize) -> (&str, bool)`.
  - `DeviceUid`, `BootId`, `ProcessUid`, each with `from_bytes([u8; 16])` and `as_bytes() -> &[u8; 16]`. `Display` is lowercase hex.
  - `EventId`, with `new_v7()`, `from_uuid(Uuid) -> Option<Self>` (v7 + RFC variant only), `as_uuid()` and `as_bytes()`.
  - `process_uid(&DeviceUid, &BootId, u64) -> ProcessUid`.

- [ ] **Step 1: Create the crate manifest and `lib.rs`**

`crates/atlas-schema/Cargo.toml` (all dependencies now, so later tasks don't touch the manifest until Task 10's `[[bench]]`):
```toml
[package]
name = "atlas-schema"
version = "0.1.0"
description = "Typed Atlas event domain model, validation, and wire conversions."
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
atlas-proto.workspace = true
blake3.workspace = true
prost.workspace = true
thiserror.workspace = true
uuid.workspace = true

[dev-dependencies]
criterion.workspace = true
proptest.workspace = true
prost-reflect.workspace = true
serde_json.workspace = true
```

`crates/atlas-schema/src/lib.rs`:
```rust
//! Atlas event schema: the typed domain model every Atlas component uses.
//!
//! Design: `docs/specs/2026-09-24-event-schema-design.md`.

mod error;
mod ids;
pub mod limits;

pub use error::{SchemaError, SchemaErrorKind};
pub use ids::{BootId, DeviceUid, EventId, ProcessUid, process_uid};
```

- [ ] **Step 2: Write the failing tests**

Create each file with only its test module for now.

`crates/atlas-schema/src/error.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_path_colon_kind() {
        let e = SchemaError::new("process.file.path", SchemaErrorKind::TooLarge);
        assert_eq!(e.to_string(), "process.file.path: TooLarge");
    }
}
```

`crates/atlas-schema/src/limits.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_string_is_untouched() {
        assert_eq!(truncate_utf8("abc", 3), ("abc", false));
    }

    #[test]
    fn long_ascii_is_cut_exactly() {
        assert_eq!(truncate_utf8("abcdef", 4), ("abcd", true));
    }

    #[test]
    fn never_splits_a_multibyte_char() {
        // "é" is 2 bytes; a 2-byte budget after "a" would split it.
        assert_eq!(truncate_utf8("aé", 2), ("a", true));
        // "😀" is 4 bytes.
        assert_eq!(truncate_utf8("😀x", 3), ("", true));
    }
}
```

`crates/atlas-schema/src/ids.rs`. The two hex strings in `process_uid_test_vectors` are the real outputs of the formula; they pin it for every future implementation:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn seq(start: u8) -> [u8; 16] {
        std::array::from_fn(|i| start + i as u8)
    }

    #[test]
    fn uid_display_is_lowercase_hex() {
        let id = DeviceUid::from_bytes(seq(0xf0));
        assert_eq!(id.to_string(), "f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        assert_eq!(format!("{id:?}"), "DeviceUid(f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff)");
    }

    #[test]
    fn event_id_new_is_v7() {
        let id = EventId::new_v7();
        assert!(EventId::from_uuid(*id.as_uuid()).is_some());
    }

    #[test]
    fn event_id_rejects_non_v7() {
        assert!(EventId::from_uuid(Uuid::nil()).is_none());
        let v4 = Uuid::from_bytes([
            0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x41, 0x11, 0x81, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
        ]);
        assert_eq!(v4.get_version_num(), 4);
        assert!(EventId::from_uuid(v4).is_none());
    }

    /// Conformance vectors: any other implementation of the formula must
    /// produce these exact outputs.
    #[test]
    fn process_uid_test_vectors() {
        let zero = process_uid(&DeviceUid::from_bytes([0; 16]), &BootId::from_bytes([0; 16]), 0);
        assert_eq!(zero.to_string(), "8a01e78b10f07bca76e121414f3c9f00");

        let v = process_uid(&DeviceUid::from_bytes(seq(0x00)), &BootId::from_bytes(seq(0x10)), 0x0001_0000_0000_002a);
        assert_eq!(v.to_string(), "b1e0e2e71592001f8cc1b00c7846432e");
    }

    #[test]
    fn process_uid_depends_on_every_input() {
        let d = DeviceUid::from_bytes(seq(0x00));
        let b = BootId::from_bytes(seq(0x10));
        let base = process_uid(&d, &b, 42);
        assert_ne!(base, process_uid(&DeviceUid::from_bytes(seq(0x01)), &b, 42));
        assert_ne!(base, process_uid(&d, &BootId::from_bytes(seq(0x11)), 42));
        assert_ne!(base, process_uid(&d, &b, 43));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p atlas-schema --lib`
Expected: FAIL to compile, with errors such as `cannot find struct SchemaError`, `cannot find function truncate_utf8` and `cannot find type DeviceUid`.

- [ ] **Step 4: Write the implementations**

Insert each block **above** the `#[cfg(test)]` module in the same file.

`crates/atlas-schema/src/error.rs`:
```rust
//! Validation errors produced by wire → domain conversion.

use std::fmt;

/// Why a wire event was rejected, and where.
///
/// `field_path` uses the domain field names, e.g. `process.file.path`,
/// `answers[3].data`, `device.uid`. Errors about the whole encoded event use
/// the path `event`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field_path}: {kind}")]
pub struct SchemaError {
    pub field_path: String,
    pub kind: SchemaErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaErrorKind {
    /// A required field, message, oneof, or enum value is absent
    /// (an enum set to its `*_UNSPECIFIED` zero value counts as absent).
    Missing,
    /// An enum or oneof carries a value this build does not know.
    UnknownEnum,
    /// A field or the whole event exceeds its size limit (see `limits`).
    TooLarge,
    /// The value is present but structurally invalid (wrong byte length,
    /// out-of-range number, undecodable protobuf).
    Malformed,
}

impl fmt::Display for SchemaErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl SchemaError {
    pub fn new(field_path: impl Into<String>, kind: SchemaErrorKind) -> Self {
        Self { field_path: field_path.into(), kind }
    }
}
```

`crates/atlas-schema/src/limits.rs`:
```rust
//! Size limits enforced on decode (spec section 6.1).
//!
//! An honest sensor truncates to fit (use [`truncate_utf8`]) and sets the
//! matching `*_truncated` flag; the validator rejects anything over a limit.

/// `process.cmd_line`, in UTF-8 bytes.
pub const CMD_LINE_MAX: usize = 64 * 1024;
/// Any path or name: file paths/names, registry key paths, registry value names.
pub const PATH_MAX: usize = 32 * 1024;
/// `reg_value.data`, in bytes.
pub const REG_DATA_MAX: usize = 4 * 1024;
/// `query.hostname`, in UTF-8 bytes.
pub const DNS_HOSTNAME_MAX: usize = 1024;
/// Each `answers[].data`, in UTF-8 bytes.
pub const DNS_ANSWER_DATA_MAX: usize = 1024;
/// Number of `answers[]` entries.
pub const DNS_ANSWERS_MAX: usize = 64;
/// A whole encoded event, checked before protobuf decoding.
pub const EVENT_MAX: usize = 256 * 1024;

/// Truncates `s` to at most `max_bytes` UTF-8 bytes without splitting a
/// character. Returns the kept prefix and whether anything was cut.
pub fn truncate_utf8(s: &str, max_bytes: usize) -> (&str, bool) {
    if s.len() <= max_bytes {
        return (s, false);
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}
```

`crates/atlas-schema/src/ids.rs`:
```rust
//! Identifiers: event ids, device/boot ids, and deterministic process uids
//! (spec sections 4.1–4.3).

use std::fmt;

use uuid::{Uuid, Variant};

macro_rules! uid16 {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name([u8; 16]);

        impl $name {
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }

            pub const fn as_bytes(&self) -> &[u8; 16] {
                &self.0
            }
        }

        /// Lowercase hex, 32 characters.
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                for b in self.0 {
                    write!(f, "{b:02x}")?;
                }
                Ok(())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self)
            }
        }
    };
}

uid16!(
    /// Random id generated once at agent install (`device.uid`).
    DeviceUid
);
uid16!(
    /// Opaque per-boot id (`device.boot_id`). Derivation is the sensor's job.
    BootId
);
uid16!(
    /// Deterministic process id (`process.uid`); see [`process_uid`].
    ProcessUid
);

/// `meta.event_id`: always a UUIDv7 (RFC 9562 variant).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct EventId(Uuid);

impl EventId {
    /// A fresh time-ordered id for a newly created event.
    pub fn new_v7() -> Self {
        Self(Uuid::now_v7())
    }

    /// Accepts only UUIDv7 with the RFC 9562 variant.
    pub fn from_uuid(uuid: Uuid) -> Option<Self> {
        (uuid.get_version_num() == 7 && uuid.get_variant() == Variant::RFC4122).then_some(Self(uuid))
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// Domain-separation tag for the v1 process uid formula. Changing the formula
/// means changing the tag, so ids from different formulas never collide.
const PROCESS_UID_TAG: &[u8] = b"atlas.process.v1";

/// `process.uid = BLAKE3("atlas.process.v1" ‖ device.uid ‖ boot_id ‖ start_key LE)[0..16]`.
///
/// `start_key` is the Windows process start key, treated as an opaque u64.
pub fn process_uid(device: &DeviceUid, boot: &BootId, start_key: u64) -> ProcessUid {
    let mut hasher = blake3::Hasher::new();
    hasher.update(PROCESS_UID_TAG);
    hasher.update(device.as_bytes());
    hasher.update(boot.as_bytes());
    hasher.update(&start_key.to_le_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    ProcessUid(out)
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p atlas-schema --lib`
Expected: PASS, 9 tests. That includes `process_uid_test_vectors` (`8a01e78b…` and `b1e0e2e7…`) and `never_splits_a_multibyte_char`.

- [ ] **Step 6: Commit**

```powershell
git add crates/atlas-schema Cargo.lock
git commit -m "feat(atlas-schema): add errors, limits, and deterministic identifiers" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01TqnCZsFoaabfHYyb1ieVfc"
```

---

### Task 3: Validation helpers and shared objects

**Files:**
- Create: `crates/atlas-schema/src/convert.rs`
- Create: `crates/atlas-schema/src/objects.rs` (with the `test_support` module and tests)
- Modify: `crates/atlas-schema/src/lib.rs`

**Interfaces:**
- Consumes: `SchemaError`/`SchemaErrorKind` (Task 2), `limits::*` (Task 2), `ProcessUid` (Task 2), and `atlas_proto::v1` (Task 1).
- Produces:
  - `pub(crate)` helpers in `convert`:
    - `type Result<T> = Result<T, SchemaError>`
    - `join(parent, field) -> String`
    - `err(parent, field, kind) -> Result<T>`
    - `require(Option<T>, parent, field)`
    - `bounded(String, max, parent, field)`
    - `fixed::<N>(Vec<u8>, parent, field) -> Result<[u8; N]>`
    - `u16_field(u32, parent, field)`
    - `wire_enum(raw, map, parent, field)`
  - Domain objects `User`, `Hashes`, `SignatureStatus`, `Signature`, `File`, `ProcessRef`, `Integrity`, `Process` and `NetworkEndpoint`, each with `From<T> for wire::T`.
  - `pub(crate)` validators:
    - `File::from_wire(w, path)` and `File::required(Option<w>, parent, field)`
    - `ProcessRef::from_wire(w, path)` and `ProcessRef::required(...)`
    - `Process::from_wire(w, path)`
    - `NetworkEndpoint::required(...)`
  - `#[cfg(test)] objects::test_support::{file(&str) -> File, proc_ref() -> ProcessRef}` for the class tests.

- [ ] **Step 1: Write the helpers and register the modules**

These helpers are not tested on their own. They get exercised through every object and class test.

`crates/atlas-schema/src/convert.rs`:
```rust
//! Shared helpers for wire → domain validation.
//!
//! Field paths are built only when an error is produced, so the happy path
//! does not allocate for them.

use crate::error::{SchemaError, SchemaErrorKind};

pub(crate) type Result<T> = std::result::Result<T, SchemaError>;

/// `parent.field`, or just `field` at the root.
pub(crate) fn join(parent: &str, field: &str) -> String {
    if parent.is_empty() { field.to_owned() } else { format!("{parent}.{field}") }
}

pub(crate) fn err<T>(parent: &str, field: &str, kind: SchemaErrorKind) -> Result<T> {
    Err(SchemaError::new(join(parent, field), kind))
}

/// A required sub-message or oneof.
pub(crate) fn require<T>(value: Option<T>, parent: &str, field: &str) -> Result<T> {
    match value {
        Some(v) => Ok(v),
        None => err(parent, field, SchemaErrorKind::Missing),
    }
}

/// A string with a byte-length limit.
pub(crate) fn bounded(value: String, max: usize, parent: &str, field: &str) -> Result<String> {
    if value.len() > max { err(parent, field, SchemaErrorKind::TooLarge) } else { Ok(value) }
}

/// A fixed-size byte field.
pub(crate) fn fixed<const N: usize>(value: Vec<u8>, parent: &str, field: &str) -> Result<[u8; N]> {
    match value.try_into() {
        Ok(v) => Ok(v),
        Err(_) => err(parent, field, SchemaErrorKind::Malformed),
    }
}

/// A `uint32` wire field that must fit in `u16`.
pub(crate) fn u16_field(value: u32, parent: &str, field: &str) -> Result<u16> {
    match u16::try_from(value) {
        Ok(v) => Ok(v),
        Err(_) => err(parent, field, SchemaErrorKind::Malformed),
    }
}

/// A proto enum field: 0 (`*_UNSPECIFIED`) is `Missing`, an unrecognized
/// number is `UnknownEnum`. `map` returns `None` for the unspecified variant.
pub(crate) fn wire_enum<W, D>(raw: i32, map: impl FnOnce(W) -> Option<D>, parent: &str, field: &str) -> Result<D>
where
    W: TryFrom<i32>,
{
    match W::try_from(raw) {
        Ok(w) => match map(w) {
            Some(d) => Ok(d),
            None => err(parent, field, SchemaErrorKind::Missing),
        },
        Err(_) => err(parent, field, SchemaErrorKind::UnknownEnum),
    }
}
```

In `crates/atlas-schema/src/lib.rs`, add the module declarations and export:

```rust
mod convert;
mod objects;

pub use objects::{File, Hashes, Integrity, NetworkEndpoint, Process, ProcessRef, Signature, SignatureStatus, User};
```

- [ ] **Step 2: Write the failing tests**

Create `crates/atlas-schema/src/objects.rs` containing only the test code:
```rust
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub fn file(path: &str) -> File {
        File { path: path.into(), name: "x.exe".into(), hashes: None, signature: None }
    }

    pub fn proc_ref() -> ProcessRef {
        ProcessRef { uid: ProcessUid::from_bytes([7; 16]), pid: 42, file: file("C:\\x.exe"), user: None }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    fn kind_at<T: std::fmt::Debug>(r: Result<T>) -> (String, SchemaErrorKind) {
        let e = r.unwrap_err();
        (e.field_path, e.kind)
    }

    #[test]
    fn file_round_trips_with_enrichment() {
        let f = File {
            hashes: Some(Hashes { sha256: Some([1; 32]) }),
            signature: Some(Signature { signer: None, status: SignatureStatus::Unsigned }),
            ..file("C:\\a.dll")
        };
        assert_eq!(File::from_wire(f.clone().into(), "file").unwrap(), f);
    }

    #[test]
    fn file_path_over_limit_is_too_large() {
        let w = wire::File { path: "a".repeat(PATH_MAX + 1), ..file("x").into() };
        assert_eq!(kind_at(File::from_wire(w, "file")), ("file.path".into(), SchemaErrorKind::TooLarge));
    }

    #[test]
    fn sha256_must_be_32_bytes() {
        let w = wire::File { hashes: Some(wire::Hashes { sha256: Some(vec![0; 31]) }), ..file("x").into() };
        assert_eq!(kind_at(File::from_wire(w, "file")), ("file.hashes.sha256".into(), SchemaErrorKind::Malformed));
    }

    #[test]
    fn signature_status_unspecified_is_missing_and_unknown_is_unknown() {
        let w = |status| wire::File { signature: Some(wire::Signature { signer: None, status }), ..file("x").into() };
        assert_eq!(kind_at(File::from_wire(w(0), "f")), ("f.signature.status".into(), SchemaErrorKind::Missing));
        assert_eq!(kind_at(File::from_wire(w(99), "f")), ("f.signature.status".into(), SchemaErrorKind::UnknownEnum));
    }

    #[test]
    fn process_ref_uid_must_be_16_bytes() {
        let w = wire::ProcessRef { uid: vec![0; 15], ..proc_ref().into() };
        assert_eq!(
            kind_at(ProcessRef::from_wire(w, "actor.process")),
            ("actor.process.uid".into(), SchemaErrorKind::Malformed)
        );
    }

    #[test]
    fn process_round_trips_and_integrity_zero_is_missing() {
        let p = Process {
            uid: ProcessUid::from_bytes([1; 16]),
            pid: 1,
            file: file("C:\\p.exe"),
            user: None,
            cmd_line: String::new(),
            cmd_line_truncated: false,
            created_time: 0,
            integrity: None,
            parent_process: Some(proc_ref()),
        };
        assert_eq!(Process::from_wire(p.clone().into(), "process").unwrap(), p);
        let w = wire::Process { integrity: Some(0), ..p.into() };
        assert_eq!(kind_at(Process::from_wire(w, "process")), ("process.integrity".into(), SchemaErrorKind::Missing));
    }

    #[test]
    fn endpoints_round_trip_v4_and_v6() {
        for ip in [IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), IpAddr::V6(Ipv6Addr::LOCALHOST)] {
            let e = NetworkEndpoint { ip, port: 443 };
            assert_eq!(NetworkEndpoint::required(Some(e.into()), "", "src_endpoint").unwrap(), e);
        }
    }

    #[test]
    fn endpoint_rejects_bad_ip_length_bad_port_and_absence() {
        let bad_ip = wire::NetworkEndpoint { ip: vec![1, 2, 3, 4, 5], port: 1 };
        assert_eq!(
            kind_at(NetworkEndpoint::required(Some(bad_ip), "", "dst_endpoint")),
            ("dst_endpoint.ip".into(), SchemaErrorKind::Malformed)
        );
        let bad_port = wire::NetworkEndpoint { ip: vec![1, 2, 3, 4], port: 70_000 };
        assert_eq!(
            kind_at(NetworkEndpoint::required(Some(bad_port), "", "src_endpoint")),
            ("src_endpoint.port".into(), SchemaErrorKind::Malformed)
        );
        assert_eq!(
            kind_at(NetworkEndpoint::required(None, "", "src_endpoint")),
            ("src_endpoint".into(), SchemaErrorKind::Missing)
        );
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p atlas-schema --lib objects`
Expected: FAIL to compile, e.g. `cannot find struct File in this scope`.

- [ ] **Step 4: Write the implementation above the test code in `objects.rs`**

```rust
//! Shared objects (spec section 5.1) and their wire conversions.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use atlas_proto::v1 as wire;

use crate::convert::{Result, bounded, err, fixed, join, require, u16_field, wire_enum};
use crate::error::SchemaErrorKind;
use crate::ids::ProcessUid;
use crate::limits::{CMD_LINE_MAX, PATH_MAX};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// Windows SID string.
    pub uid: String,
    /// `DOMAIN\user`.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hashes {
    pub sha256: Option<[u8; 32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureStatus {
    Valid,
    Invalid,
    Unsigned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub signer: Option<String>,
    pub status: SignatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub name: String,
    pub hashes: Option<Hashes>,
    pub signature: Option<Signature>,
}

/// The actor core carried by every event (spec D5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRef {
    pub uid: ProcessUid,
    pub pid: u32,
    pub file: File,
    pub user: Option<User>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Integrity {
    Untrusted,
    Low,
    Medium,
    High,
    System,
    Protected,
}

/// Full process detail; only carried by Process Launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub uid: ProcessUid,
    pub pid: u32,
    pub file: File,
    pub user: Option<User>,
    pub cmd_line: String,
    pub cmd_line_truncated: bool,
    /// Nanoseconds since the Unix epoch, UTC.
    pub created_time: i64,
    pub integrity: Option<Integrity>,
    /// The parent recorded by the OS (can be spoofed). `None` when the OS
    /// reports no parent (e.g. early boot processes).
    pub parent_process: Option<ProcessRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkEndpoint {
    pub ip: IpAddr,
    pub port: u16,
}

// ---- domain → wire (infallible) ----

impl From<User> for wire::User {
    fn from(v: User) -> Self {
        Self { uid: v.uid, name: v.name }
    }
}

impl From<Hashes> for wire::Hashes {
    fn from(v: Hashes) -> Self {
        Self { sha256: v.sha256.map(|h| h.to_vec()) }
    }
}

impl From<SignatureStatus> for wire::SignatureStatus {
    fn from(v: SignatureStatus) -> Self {
        match v {
            SignatureStatus::Valid => Self::Valid,
            SignatureStatus::Invalid => Self::Invalid,
            SignatureStatus::Unsigned => Self::Unsigned,
        }
    }
}

impl From<Signature> for wire::Signature {
    fn from(v: Signature) -> Self {
        Self { signer: v.signer, status: wire::SignatureStatus::from(v.status) as i32 }
    }
}

impl From<File> for wire::File {
    fn from(v: File) -> Self {
        Self { path: v.path, name: v.name, hashes: v.hashes.map(Into::into), signature: v.signature.map(Into::into) }
    }
}

impl From<ProcessRef> for wire::ProcessRef {
    fn from(v: ProcessRef) -> Self {
        Self { uid: v.uid.as_bytes().to_vec(), pid: v.pid, file: Some(v.file.into()), user: v.user.map(Into::into) }
    }
}

impl From<Integrity> for wire::Integrity {
    fn from(v: Integrity) -> Self {
        match v {
            Integrity::Untrusted => Self::Untrusted,
            Integrity::Low => Self::Low,
            Integrity::Medium => Self::Medium,
            Integrity::High => Self::High,
            Integrity::System => Self::System,
            Integrity::Protected => Self::Protected,
        }
    }
}

impl From<Process> for wire::Process {
    fn from(v: Process) -> Self {
        Self {
            uid: v.uid.as_bytes().to_vec(),
            pid: v.pid,
            file: Some(v.file.into()),
            user: v.user.map(Into::into),
            cmd_line: v.cmd_line,
            cmd_line_truncated: v.cmd_line_truncated,
            created_time: v.created_time,
            integrity: v.integrity.map(|i| wire::Integrity::from(i) as i32),
            parent_process: v.parent_process.map(Into::into),
        }
    }
}

impl From<NetworkEndpoint> for wire::NetworkEndpoint {
    fn from(v: NetworkEndpoint) -> Self {
        let ip = match v.ip {
            IpAddr::V4(a) => a.octets().to_vec(),
            IpAddr::V6(a) => a.octets().to_vec(),
        };
        Self { ip, port: u32::from(v.port) }
    }
}

// ---- wire → domain (validating) ----

impl User {
    pub(crate) fn from_wire(w: wire::User) -> Self {
        Self { uid: w.uid, name: w.name }
    }
}

impl Hashes {
    pub(crate) fn from_wire(w: wire::Hashes, path: &str) -> Result<Self> {
        let sha256 = match w.sha256 {
            Some(b) => Some(fixed::<32>(b, path, "sha256")?),
            None => None,
        };
        Ok(Self { sha256 })
    }
}

impl Signature {
    pub(crate) fn from_wire(w: wire::Signature, path: &str) -> Result<Self> {
        let status = wire_enum(
            w.status,
            |s: wire::SignatureStatus| match s {
                wire::SignatureStatus::Unspecified => None,
                wire::SignatureStatus::Valid => Some(SignatureStatus::Valid),
                wire::SignatureStatus::Invalid => Some(SignatureStatus::Invalid),
                wire::SignatureStatus::Unsigned => Some(SignatureStatus::Unsigned),
            },
            path,
            "status",
        )?;
        Ok(Self { signer: w.signer, status })
    }
}

impl File {
    pub(crate) fn from_wire(w: wire::File, path: &str) -> Result<Self> {
        Ok(Self {
            path: bounded(w.path, PATH_MAX, path, "path")?,
            name: bounded(w.name, PATH_MAX, path, "name")?,
            hashes: match w.hashes {
                Some(h) => Some(Hashes::from_wire(h, &join(path, "hashes"))?),
                None => None,
            },
            signature: match w.signature {
                Some(s) => Some(Signature::from_wire(s, &join(path, "signature"))?),
                None => None,
            },
        })
    }

    /// A required `File` sub-message at `parent.field`.
    pub(crate) fn required(w: Option<wire::File>, parent: &str, field: &str) -> Result<Self> {
        let w = require(w, parent, field)?;
        Self::from_wire(w, &join(parent, field))
    }
}

impl ProcessRef {
    pub(crate) fn from_wire(w: wire::ProcessRef, path: &str) -> Result<Self> {
        Ok(Self {
            uid: ProcessUid::from_bytes(fixed::<16>(w.uid, path, "uid")?),
            pid: w.pid,
            file: File::required(w.file, path, "file")?,
            user: w.user.map(User::from_wire),
        })
    }

    /// A required `ProcessRef` sub-message at `parent.field`.
    pub(crate) fn required(w: Option<wire::ProcessRef>, parent: &str, field: &str) -> Result<Self> {
        let w = require(w, parent, field)?;
        Self::from_wire(w, &join(parent, field))
    }
}

fn integrity_from_wire(raw: i32, path: &str) -> Result<Integrity> {
    wire_enum(
        raw,
        |i: wire::Integrity| match i {
            wire::Integrity::Unspecified => None,
            wire::Integrity::Untrusted => Some(Integrity::Untrusted),
            wire::Integrity::Low => Some(Integrity::Low),
            wire::Integrity::Medium => Some(Integrity::Medium),
            wire::Integrity::High => Some(Integrity::High),
            wire::Integrity::System => Some(Integrity::System),
            wire::Integrity::Protected => Some(Integrity::Protected),
        },
        path,
        "integrity",
    )
}

impl Process {
    pub(crate) fn from_wire(w: wire::Process, path: &str) -> Result<Self> {
        Ok(Self {
            uid: ProcessUid::from_bytes(fixed::<16>(w.uid, path, "uid")?),
            pid: w.pid,
            file: File::required(w.file, path, "file")?,
            user: w.user.map(User::from_wire),
            cmd_line: bounded(w.cmd_line, CMD_LINE_MAX, path, "cmd_line")?,
            cmd_line_truncated: w.cmd_line_truncated,
            created_time: w.created_time,
            integrity: match w.integrity {
                Some(raw) => Some(integrity_from_wire(raw, path)?),
                None => None,
            },
            parent_process: match w.parent_process {
                Some(p) => Some(ProcessRef::from_wire(p, &join(path, "parent_process"))?),
                None => None,
            },
        })
    }
}

impl NetworkEndpoint {
    pub(crate) fn required(w: Option<wire::NetworkEndpoint>, parent: &str, field: &str) -> Result<Self> {
        let w = require(w, parent, field)?;
        let path = join(parent, field);
        let ip = match w.ip.len() {
            4 => IpAddr::V4(Ipv4Addr::from(fixed::<4>(w.ip, &path, "ip")?)),
            16 => IpAddr::V6(Ipv6Addr::from(fixed::<16>(w.ip, &path, "ip")?)),
            _ => return err(&path, "ip", SchemaErrorKind::Malformed),
        };
        let port = u16_field(w.port, &path, "port")?;
        Ok(Self { ip, port })
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p atlas-schema --lib`
Expected: PASS, 17 tests (9 from Task 2 plus 8 object tests). The compiler will warn that `pub(crate)` validators like `ProcessRef::required` are unused. That's expected until Tasks 4–7 use them.

- [ ] **Step 6: Commit**

```powershell
git add crates/atlas-schema
git commit -m "feat(atlas-schema): add shared objects with validating wire conversion" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01TqnCZsFoaabfHYyb1ieVfc"
```

---

### Task 4: Process and Module classes

**Files:**
- Create: `crates/atlas-schema/src/classes/mod.rs`, `crates/atlas-schema/src/classes/process.rs`, `crates/atlas-schema/src/classes/module.rs`
- Modify: `crates/atlas-schema/src/lib.rs`

**Interfaces:**
- Consumes (Task 3): `ProcessRef`, `Process`, `File`, their `required` / `from_wire` validators, `require`, and `test_support`.
- Produces:
  - `classes::process::ProcessActivity::{Launch { actor: ProcessRef, process: Process }, Terminate { process: ProcessRef, exit_code: Option<i32> }}`
  - `classes::module::{ModuleActivity { actor, action }, ModuleAction::Load { file: File, base_address: u64 }}`
  - Each has `From<_> for wire::_`, plus `pub(crate) fn from_wire(wire::_) -> Result<Self>` whose error paths are relative to the event root.

- [ ] **Step 1: Register the module**

`crates/atlas-schema/src/classes/mod.rs`:
```rust
//! The seven v1 event classes (spec section 5). Each module holds the domain
//! types for one OCSF class plus their wire conversions.

pub mod module;
pub mod process;
```

In `crates/atlas-schema/src/lib.rs`, add `pub mod classes;` above `mod convert;`.

- [ ] **Step 2: Write the failing tests**

`crates/atlas-schema/src/classes/process.rs`, test code only for now:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SchemaErrorKind;
    use crate::ids::ProcessUid;
    use crate::limits::CMD_LINE_MAX;
    use crate::objects::Integrity;
    use crate::objects::test_support::{file, proc_ref};

    fn launch() -> ProcessActivity {
        ProcessActivity::Launch {
            actor: proc_ref(),
            process: Process {
                uid: ProcessUid::from_bytes([9; 16]),
                pid: 100,
                file: file("C:\\child.exe"),
                user: None,
                cmd_line: "child.exe /x".into(),
                cmd_line_truncated: false,
                created_time: 5,
                integrity: Some(Integrity::High),
                parent_process: Some(proc_ref()),
            },
        }
    }

    #[test]
    fn launch_and_terminate_round_trip() {
        for a in [launch(), ProcessActivity::Terminate { process: proc_ref(), exit_code: Some(-1) }] {
            assert_eq!(ProcessActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn launch_without_actor_is_missing_actor_process() {
        let mut w = wire::ProcessActivity::from(launch());
        let Some(W::Launch(l)) = w.activity.as_mut() else { unreachable!() };
        l.actor = None;
        let e = ProcessActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("actor.process", SchemaErrorKind::Missing));
    }

    #[test]
    fn cmd_line_over_limit_is_too_large() {
        let mut w = wire::ProcessActivity::from(launch());
        let Some(W::Launch(l)) = w.activity.as_mut() else { unreachable!() };
        l.process.as_mut().unwrap().cmd_line = "a".repeat(CMD_LINE_MAX + 1);
        let e = ProcessActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("process.cmd_line", SchemaErrorKind::TooLarge));
    }

    #[test]
    fn missing_activity_is_missing() {
        let e = ProcessActivity::from_wire(wire::ProcessActivity { activity: None }).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("activity", SchemaErrorKind::Missing));
    }
}
```

`crates/atlas-schema/src/classes/module.rs`, test code only for now:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SchemaErrorKind;
    use crate::objects::test_support::{file, proc_ref};

    fn load() -> ModuleActivity {
        ModuleActivity {
            actor: proc_ref(),
            action: ModuleAction::Load { file: file("C:\\amsi.dll"), base_address: 0x7ff0_0000 },
        }
    }

    #[test]
    fn load_round_trips() {
        assert_eq!(ModuleActivity::from_wire(load().into()).unwrap(), load());
    }

    #[test]
    fn load_without_file_is_missing_module_file() {
        let mut w = wire::ModuleActivity::from(load());
        let Some(W::Load(l)) = w.activity.as_mut() else { unreachable!() };
        l.file = None;
        let e = ModuleActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("module.file", SchemaErrorKind::Missing));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p atlas-schema --lib classes`
Expected: FAIL to compile, e.g. `cannot find type ProcessActivity`.

- [ ] **Step 4: Write the implementations above each test module**

`crates/atlas-schema/src/classes/process.rs`:
```rust
//! Process Activity (OCSF 1007), spec section 5.2.

use atlas_proto::v1 as wire;
use atlas_proto::v1::process_activity::Activity as W;

use crate::convert::{Result, require};
use crate::objects::{Process, ProcessRef};

// Launch is much larger than Terminate by design (full process detail).
// Events are moved, not stored in bulk arrays, so boxing buys nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessActivity {
    Launch {
        /// The process that actually issued the creation.
        actor: ProcessRef,
        /// The new process, full detail.
        process: Process,
    },
    Terminate {
        process: ProcessRef,
        exit_code: Option<i32>,
    },
}

impl From<ProcessActivity> for wire::ProcessActivity {
    fn from(v: ProcessActivity) -> Self {
        let activity = match v {
            ProcessActivity::Launch { actor, process } => {
                W::Launch(wire::ProcessLaunch { actor: Some(actor.into()), process: Some(process.into()) })
            }
            ProcessActivity::Terminate { process, exit_code } => {
                W::Terminate(wire::ProcessTerminate { process: Some(process.into()), exit_code })
            }
        };
        Self { activity: Some(activity) }
    }
}

impl ProcessActivity {
    pub(crate) fn from_wire(w: wire::ProcessActivity) -> Result<Self> {
        Ok(match require(w.activity, "", "activity")? {
            W::Launch(l) => Self::Launch {
                actor: ProcessRef::required(l.actor, "", "actor.process")?,
                process: Process::from_wire(require(l.process, "", "process")?, "process")?,
            },
            W::Terminate(t) => {
                Self::Terminate { process: ProcessRef::required(t.process, "", "process")?, exit_code: t.exit_code }
            }
        })
    }
}
```

`crates/atlas-schema/src/classes/module.rs`:
```rust
//! Module Activity (OCSF 1005), spec section 5.3.

use atlas_proto::v1 as wire;
use atlas_proto::v1::module_activity::Activity as W;

use crate::convert::{Result, require};
use crate::objects::{File, ProcessRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleActivity {
    pub actor: ProcessRef,
    pub action: ModuleAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleAction {
    Load { file: File, base_address: u64 },
}

impl From<ModuleActivity> for wire::ModuleActivity {
    fn from(v: ModuleActivity) -> Self {
        let activity = match v.action {
            ModuleAction::Load { file, base_address } => {
                W::Load(wire::ModuleLoad { file: Some(file.into()), base_address })
            }
        };
        Self { actor: Some(v.actor.into()), activity: Some(activity) }
    }
}

impl ModuleActivity {
    pub(crate) fn from_wire(w: wire::ModuleActivity) -> Result<Self> {
        let actor = ProcessRef::required(w.actor, "", "actor.process")?;
        let action = match require(w.activity, "", "activity")? {
            W::Load(l) => {
                ModuleAction::Load { file: File::required(l.file, "", "module.file")?, base_address: l.base_address }
            }
        };
        Ok(Self { actor, action })
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p atlas-schema --lib`
Expected: PASS, 23 tests.

- [ ] **Step 6: Commit**

```powershell
git add crates/atlas-schema
git commit -m "feat(atlas-schema): add Process and Module activity classes" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01TqnCZsFoaabfHYyb1ieVfc"
```

---

### Task 5: Network and File System classes

**Files:**
- Create: `crates/atlas-schema/src/classes/network.rs`, `crates/atlas-schema/src/classes/file.rs`
- Modify: `crates/atlas-schema/src/classes/mod.rs`

**Interfaces:**
- Consumes (Task 3): `ProcessRef`, `File`, `NetworkEndpoint::required`, `require`, `wire_enum`, and `test_support`.
- Produces:
  - `classes::network::{NetworkActivity { actor, src_endpoint, dst_endpoint, protocol, direction, action }, NetworkProtocol::{Tcp, Udp}, NetworkDirection::{Inbound, Outbound}, NetworkAction::{Open, Close { bytes_in: Option<u64>, bytes_out: Option<u64> }}}`
  - `classes::file::{FileSystemActivity { actor, file, action }, FileAction::{Create, Read, Update, Delete, Rename { file_result: File }, SetAttributes}}`
  - Each has `From` and `pub(crate) from_wire`.

- [ ] **Step 1: Register the modules**

Replace `crates/atlas-schema/src/classes/mod.rs` with:
```rust
//! The seven v1 event classes (spec section 5). Each module holds the domain
//! types for one OCSF class plus their wire conversions.

pub mod file;
pub mod module;
pub mod network;
pub mod process;
```

- [ ] **Step 2: Write the failing tests**

`crates/atlas-schema/src/classes/network.rs`, test code only for now:
```rust
#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;
    use crate::error::SchemaErrorKind;
    use crate::objects::test_support::proc_ref;

    fn activity(action: NetworkAction) -> NetworkActivity {
        let ep = |port| NetworkEndpoint { ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), port };
        NetworkActivity {
            actor: proc_ref(),
            src_endpoint: ep(50000),
            dst_endpoint: ep(443),
            protocol: NetworkProtocol::Udp,
            direction: NetworkDirection::Inbound,
            action,
        }
    }

    #[test]
    fn open_and_close_round_trip() {
        for a in [activity(NetworkAction::Open), activity(NetworkAction::Close { bytes_in: Some(1), bytes_out: None })]
        {
            assert_eq!(NetworkActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn unspecified_protocol_is_missing_and_unknown_direction_is_unknown() {
        let w = wire::NetworkActivity { protocol: 0, ..activity(NetworkAction::Open).into() };
        let e = NetworkActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("protocol", SchemaErrorKind::Missing));

        let w = wire::NetworkActivity { direction: 9, ..activity(NetworkAction::Open).into() };
        let e = NetworkActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("direction", SchemaErrorKind::UnknownEnum));
    }
}
```

`crates/atlas-schema/src/classes/file.rs`, test code only for now:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SchemaErrorKind;
    use crate::objects::test_support::{file, proc_ref};

    fn activity(action: FileAction) -> FileSystemActivity {
        FileSystemActivity { actor: proc_ref(), file: file("C:\\a.txt"), action }
    }

    #[test]
    fn every_action_round_trips() {
        let actions = [
            FileAction::Create,
            FileAction::Read,
            FileAction::Update,
            FileAction::Delete,
            FileAction::Rename { file_result: file("C:\\b.txt") },
            FileAction::SetAttributes,
        ];
        for action in actions {
            let a = activity(action);
            assert_eq!(FileSystemActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn rename_without_result_is_missing_file_result() {
        let mut w = wire::FileSystemActivity::from(activity(FileAction::Rename { file_result: file("C:\\b.txt") }));
        let Some(W::Rename(r)) = w.activity.as_mut() else { unreachable!() };
        r.file_result = None;
        let e = FileSystemActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("file_result", SchemaErrorKind::Missing));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p atlas-schema --lib classes`
Expected: FAIL to compile, e.g. `cannot find type NetworkActivity`.

- [ ] **Step 4: Write the implementations above each test module**

`crates/atlas-schema/src/classes/network.rs`:
```rust
//! Network Activity (OCSF 4001), spec section 5.4.

use atlas_proto::v1 as wire;
use atlas_proto::v1::network_activity::Activity as W;

use crate::convert::{Result, require, wire_enum};
use crate::objects::{NetworkEndpoint, ProcessRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkActivity {
    pub actor: ProcessRef,
    pub src_endpoint: NetworkEndpoint,
    pub dst_endpoint: NetworkEndpoint,
    pub protocol: NetworkProtocol,
    pub direction: NetworkDirection,
    pub action: NetworkAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkProtocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkAction {
    Open,
    Close { bytes_in: Option<u64>, bytes_out: Option<u64> },
}

impl From<NetworkActivity> for wire::NetworkActivity {
    fn from(v: NetworkActivity) -> Self {
        let protocol = match v.protocol {
            NetworkProtocol::Tcp => wire::NetworkProtocol::Tcp,
            NetworkProtocol::Udp => wire::NetworkProtocol::Udp,
        };
        let direction = match v.direction {
            NetworkDirection::Inbound => wire::NetworkDirection::Inbound,
            NetworkDirection::Outbound => wire::NetworkDirection::Outbound,
        };
        let activity = match v.action {
            NetworkAction::Open => W::Open(wire::NetworkOpen {}),
            NetworkAction::Close { bytes_in, bytes_out } => W::Close(wire::NetworkClose { bytes_in, bytes_out }),
        };
        Self {
            actor: Some(v.actor.into()),
            src_endpoint: Some(v.src_endpoint.into()),
            dst_endpoint: Some(v.dst_endpoint.into()),
            protocol: protocol as i32,
            direction: direction as i32,
            activity: Some(activity),
        }
    }
}

impl NetworkActivity {
    pub(crate) fn from_wire(w: wire::NetworkActivity) -> Result<Self> {
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            src_endpoint: NetworkEndpoint::required(w.src_endpoint, "", "src_endpoint")?,
            dst_endpoint: NetworkEndpoint::required(w.dst_endpoint, "", "dst_endpoint")?,
            protocol: wire_enum(
                w.protocol,
                |p: wire::NetworkProtocol| match p {
                    wire::NetworkProtocol::Unspecified => None,
                    wire::NetworkProtocol::Tcp => Some(NetworkProtocol::Tcp),
                    wire::NetworkProtocol::Udp => Some(NetworkProtocol::Udp),
                },
                "",
                "protocol",
            )?,
            direction: wire_enum(
                w.direction,
                |d: wire::NetworkDirection| match d {
                    wire::NetworkDirection::Unspecified => None,
                    wire::NetworkDirection::Inbound => Some(NetworkDirection::Inbound),
                    wire::NetworkDirection::Outbound => Some(NetworkDirection::Outbound),
                },
                "",
                "direction",
            )?,
            action: match require(w.activity, "", "activity")? {
                W::Open(_) => NetworkAction::Open,
                W::Close(c) => NetworkAction::Close { bytes_in: c.bytes_in, bytes_out: c.bytes_out },
            },
        })
    }
}
```

`crates/atlas-schema/src/classes/file.rs`:
```rust
//! File System Activity (OCSF 1001), spec section 5.5.

use atlas_proto::v1 as wire;
use atlas_proto::v1::file_system_activity::Activity as W;

use crate::convert::{Result, require};
use crate::objects::{File, ProcessRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSystemActivity {
    pub actor: ProcessRef,
    /// For `Rename`: the original file.
    pub file: File,
    pub action: FileAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileAction {
    Create,
    Read,
    Update,
    Delete,
    Rename { file_result: File },
    SetAttributes,
}

impl From<FileSystemActivity> for wire::FileSystemActivity {
    fn from(v: FileSystemActivity) -> Self {
        let activity = match v.action {
            FileAction::Create => W::Create(wire::FileCreate {}),
            FileAction::Read => W::Read(wire::FileRead {}),
            FileAction::Update => W::Update(wire::FileUpdate {}),
            FileAction::Delete => W::Delete(wire::FileDelete {}),
            FileAction::Rename { file_result } => W::Rename(wire::FileRename { file_result: Some(file_result.into()) }),
            FileAction::SetAttributes => W::SetAttributes(wire::FileSetAttributes {}),
        };
        Self { actor: Some(v.actor.into()), file: Some(v.file.into()), activity: Some(activity) }
    }
}

impl FileSystemActivity {
    pub(crate) fn from_wire(w: wire::FileSystemActivity) -> Result<Self> {
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            file: File::required(w.file, "", "file")?,
            action: match require(w.activity, "", "activity")? {
                W::Create(_) => FileAction::Create,
                W::Read(_) => FileAction::Read,
                W::Update(_) => FileAction::Update,
                W::Delete(_) => FileAction::Delete,
                W::Rename(r) => FileAction::Rename { file_result: File::required(r.file_result, "", "file_result")? },
                W::SetAttributes(_) => FileAction::SetAttributes,
            },
        })
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p atlas-schema --lib`
Expected: PASS, 27 tests.

- [ ] **Step 6: Commit**

```powershell
git add crates/atlas-schema
git commit -m "feat(atlas-schema): add Network and File System activity classes" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01TqnCZsFoaabfHYyb1ieVfc"
```

---

### Task 6: Registry and DNS classes

**Files:**
- Create: `crates/atlas-schema/src/classes/registry.rs`, `crates/atlas-schema/src/classes/dns.rs`
- Modify: `crates/atlas-schema/src/classes/mod.rs`

**Interfaces:**
- Consumes (Tasks 2–3): `ProcessRef`, `require`, `bounded`, `err`, `u16_field`, `limits::{PATH_MAX, REG_DATA_MAX, DNS_*}`, and `test_support`.
- Produces:
  - `classes::registry::{RegistryKeyActivity { actor, path, action }, RegistryKeyAction::{Create, Delete, Rename { prev_path: String }}, RegistryValueActivity { actor, key_path, name, action }, RegistryValueAction::{Set { value_type: RegValueType, data: Vec<u8>, data_truncated: bool }, Delete}, RegValueType (#[repr(u32)], Windows REG_* 0..=11, from_raw(u32) -> Option<Self>)}`
  - `classes::dns::{DnsActivity { actor, hostname, query_type: u16, action }, DnsAction::Response { rcode: Option<u16>, platform_status: Option<u32>, answers: Vec<DnsAnswer> }, DnsAnswer { rr_type: u16, data: String }}`

- [ ] **Step 1: Register the modules**

Replace `crates/atlas-schema/src/classes/mod.rs` with:
```rust
//! The seven v1 event classes (spec section 5). Each module holds the domain
//! types for one OCSF class plus their wire conversions.

pub mod dns;
pub mod file;
pub mod module;
pub mod network;
pub mod process;
pub mod registry;
```

- [ ] **Step 2: Write the failing tests**

`crates/atlas-schema/src/classes/registry.rs`, test code only for now:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::test_support::proc_ref;

    fn key(action: RegistryKeyAction) -> RegistryKeyActivity {
        RegistryKeyActivity { actor: proc_ref(), path: "HKLM\\SOFTWARE\\New".into(), action }
    }

    fn set(data: Vec<u8>) -> RegistryValueActivity {
        RegistryValueActivity {
            actor: proc_ref(),
            key_path: "HKCU\\Software\\Run".into(),
            name: "x".into(),
            action: RegistryValueAction::Set { value_type: RegValueType::Dword, data, data_truncated: false },
        }
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
        let delete = RegistryValueActivity { action: RegistryValueAction::Delete, ..set(vec![]) };
        for a in [set(vec![1, 0, 0, 0]), delete] {
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
    fn unknown_value_type_and_oversized_data_are_rejected() {
        let mut w = wire::RegistryValueActivity::from(set(vec![]));
        let Some(WV::Set(s)) = w.activity.as_mut() else { unreachable!() };
        s.r#type = 12;
        let e = RegistryValueActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("reg_value.type", SchemaErrorKind::UnknownEnum));

        let e = RegistryValueActivity::from_wire(set(vec![0; REG_DATA_MAX + 1]).into()).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("reg_value.data", SchemaErrorKind::TooLarge));
    }
}
```

`crates/atlas-schema/src/classes/dns.rs`, test code only for now:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::test_support::proc_ref;

    fn response(answers: Vec<DnsAnswer>) -> DnsActivity {
        DnsActivity {
            actor: proc_ref(),
            hostname: "example.com".into(),
            query_type: 1,
            action: DnsAction::Response { rcode: Some(3), platform_status: Some(9003), answers },
        }
    }

    fn answer(data: &str) -> DnsAnswer {
        DnsAnswer { rr_type: 1, data: data.into() }
    }

    #[test]
    fn response_round_trips() {
        let a = response(vec![answer("93.184.216.34"), answer("93.184.216.35")]);
        assert_eq!(DnsActivity::from_wire(a.clone().into()).unwrap(), a);
    }

    #[test]
    fn too_many_answers_is_too_large() {
        let e = DnsActivity::from_wire(response(vec![answer("x"); DNS_ANSWERS_MAX + 1]).into()).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("answers", SchemaErrorKind::TooLarge));
    }

    #[test]
    fn answer_errors_carry_their_index() {
        let long = "a".repeat(DNS_ANSWER_DATA_MAX + 1);
        let e = DnsActivity::from_wire(response(vec![answer("ok"), answer(&long)]).into()).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("answers[1].data", SchemaErrorKind::TooLarge));
    }

    #[test]
    fn query_type_must_fit_u16() {
        let w = wire::DnsActivity { query_type: 65_536, ..response(vec![]).into() };
        let e = DnsActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("query.type", SchemaErrorKind::Malformed));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p atlas-schema --lib classes`
Expected: FAIL to compile, e.g. `cannot find type RegistryKeyActivity`.

- [ ] **Step 4: Write the implementations above each test module**

`crates/atlas-schema/src/classes/registry.rs`:
```rust
//! Registry Key Activity (OCSF 201001) and Registry Value Activity (OCSF 201002),
//! spec sections 5.6–5.7.

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
    pub action: RegistryValueAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryValueAction {
    Set { value_type: RegValueType, data: Vec<u8>, data_truncated: bool },
    Delete,
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

impl From<RegistryKeyActivity> for wire::RegistryKeyActivity {
    fn from(v: RegistryKeyActivity) -> Self {
        let activity = match v.action {
            RegistryKeyAction::Create => WK::Create(wire::RegistryKeyCreate {}),
            RegistryKeyAction::Delete => WK::Delete(wire::RegistryKeyDelete {}),
            RegistryKeyAction::Rename { prev_path } => WK::Rename(wire::RegistryKeyRename { prev_path }),
        };
        Self { actor: Some(v.actor.into()), path: v.path, activity: Some(activity) }
    }
}

impl From<RegistryValueActivity> for wire::RegistryValueActivity {
    fn from(v: RegistryValueActivity) -> Self {
        let activity = match v.action {
            RegistryValueAction::Set { value_type, data, data_truncated } => {
                WV::Set(wire::RegistryValueSet { r#type: value_type as u32, data, data_truncated })
            }
            RegistryValueAction::Delete => WV::Delete(wire::RegistryValueDelete {}),
        };
        Self { actor: Some(v.actor.into()), key_path: v.key_path, name: v.name, activity: Some(activity) }
    }
}

impl RegistryKeyActivity {
    pub(crate) fn from_wire(w: wire::RegistryKeyActivity) -> Result<Self> {
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            path: bounded(w.path, PATH_MAX, "", "reg_key.path")?,
            action: match require(w.activity, "", "activity")? {
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
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            key_path: bounded(w.key_path, PATH_MAX, "", "reg_value.path")?,
            name: bounded(w.name, PATH_MAX, "", "reg_value.name")?,
            action: match require(w.activity, "", "activity")? {
                WV::Set(s) => {
                    let Some(value_type) = RegValueType::from_raw(s.r#type) else {
                        return err("", "reg_value.type", SchemaErrorKind::UnknownEnum);
                    };
                    if s.data.len() > REG_DATA_MAX {
                        return err("", "reg_value.data", SchemaErrorKind::TooLarge);
                    }
                    RegistryValueAction::Set { value_type, data: s.data, data_truncated: s.data_truncated }
                }
                WV::Delete(_) => RegistryValueAction::Delete,
            },
        })
    }
}
```

`crates/atlas-schema/src/classes/dns.rs`:
```rust
//! DNS Activity (OCSF 4003), spec section 5.8.

use atlas_proto::v1 as wire;
use atlas_proto::v1::dns_activity::Activity as W;

use crate::convert::{Result, bounded, err, require, u16_field};
use crate::error::SchemaErrorKind;
use crate::limits::{DNS_ANSWER_DATA_MAX, DNS_ANSWERS_MAX, DNS_HOSTNAME_MAX};
use crate::objects::ProcessRef;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsActivity {
    pub actor: ProcessRef,
    /// `query.hostname`.
    pub hostname: String,
    /// `query.type`: numeric RR type (28 = AAAA).
    pub query_type: u16,
    pub action: DnsAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsAction {
    Response {
        /// DNS response code, when the sensor could map the platform status.
        rcode: Option<u16>,
        /// Raw platform status (Windows: Win32/DNS status from event 3008).
        platform_status: Option<u32>,
        answers: Vec<DnsAnswer>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsAnswer {
    /// Numeric RR type.
    pub rr_type: u16,
    pub data: String,
}

impl From<DnsActivity> for wire::DnsActivity {
    fn from(v: DnsActivity) -> Self {
        let activity = match v.action {
            DnsAction::Response { rcode, platform_status, answers } => W::Response(wire::DnsResponse {
                rcode: rcode.map(u32::from),
                platform_status,
                answers: answers
                    .into_iter()
                    .map(|a| wire::DnsAnswer { r#type: u32::from(a.rr_type), data: a.data })
                    .collect(),
            }),
        };
        Self {
            actor: Some(v.actor.into()),
            hostname: v.hostname,
            query_type: u32::from(v.query_type),
            activity: Some(activity),
        }
    }
}

impl DnsActivity {
    pub(crate) fn from_wire(w: wire::DnsActivity) -> Result<Self> {
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            hostname: bounded(w.hostname, DNS_HOSTNAME_MAX, "", "query.hostname")?,
            query_type: u16_field(w.query_type, "", "query.type")?,
            action: match require(w.activity, "", "activity")? {
                W::Response(r) => {
                    if r.answers.len() > DNS_ANSWERS_MAX {
                        return err("", "answers", SchemaErrorKind::TooLarge);
                    }
                    let rcode = match r.rcode {
                        Some(c) => Some(u16_field(c, "", "rcode")?),
                        None => None,
                    };
                    let answers = r
                        .answers
                        .into_iter()
                        .enumerate()
                        .map(|(i, a)| {
                            let path = format!("answers[{i}]");
                            Ok(DnsAnswer {
                                rr_type: u16_field(a.r#type, &path, "type")?,
                                data: bounded(a.data, DNS_ANSWER_DATA_MAX, &path, "data")?,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    DnsAction::Response { rcode, platform_status: r.platform_status, answers }
                }
            },
        })
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p atlas-schema --lib`
Expected: PASS, 35 tests.

- [ ] **Step 6: Commit**

```powershell
git add crates/atlas-schema
git commit -m "feat(atlas-schema): add Registry Key/Value and DNS activity classes" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01TqnCZsFoaabfHYyb1ieVfc"
```

---

### Task 7: Event envelope, codec, OCSF ids, and the validation suite

**Files:**
- Create: `crates/atlas-schema/src/event.rs`, `crates/atlas-schema/src/ocsf.rs`, `crates/atlas-schema/src/codec.rs`
- Modify: `crates/atlas-schema/src/lib.rs` (final form)
- Test: `crates/atlas-schema/tests/common/mod.rs`, `tests/roundtrip.rs`, `tests/validation.rs`, `tests/ocsf_ids.rs`

**Interfaces:**
- Consumes: every class from Tasks 4–6, the ids from Task 2, and the helpers from Task 3.
- Produces (the public API later sub-projects use):
  - `Event { meta: EventMeta, device: Device, kind: EventKind }`
  - `EventMeta { event_id: EventId, time: i64, sensor: Sensor }`
  - `Sensor::{Etw, Driver}`
  - `Device { uid: DeviceUid, boot_id: BootId }`
  - `EventKind::{Process, Module, Network, File, RegistryKey, RegistryValue, Dns}`
  - `impl From<Event> for wire::Event` and `impl TryFrom<wire::Event> for Event` (Error = `SchemaError`)
  - `encode_event(Event) -> Vec<u8>` and `decode_event(&[u8]) -> Result<Event, SchemaError>`
  - `EventKind::ocsf_ids() -> OcsfIds { category_uid, class_uid, activity_id }` and `OcsfIds::type_uid() -> u64`
  - `atlas_schema::wire`, which re-exports `atlas_proto::v1`
  - Test data: `tests/common/mod.rs::samples() -> Vec<(&'static str, Event)>`, 17 named samples such as `"process_launch"`

- [ ] **Step 1: Write the shared test data**

`crates/atlas-schema/tests/common/mod.rs` (Task 8 adds the proptest strategies to this file):
```rust
//! Shared test data: one named sample per class/activity, and proptest
//! strategies that generate arbitrary *valid* domain events.

#![allow(dead_code)] // each test binary uses a different subset

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use atlas_schema::classes::dns::{DnsAction, DnsActivity, DnsAnswer};
use atlas_schema::classes::file::{FileAction, FileSystemActivity};
use atlas_schema::classes::module::{ModuleAction, ModuleActivity};
use atlas_schema::classes::network::{NetworkAction, NetworkActivity, NetworkDirection, NetworkProtocol};
use atlas_schema::classes::process::ProcessActivity;
use atlas_schema::classes::registry::{
    RegValueType, RegistryKeyAction, RegistryKeyActivity, RegistryValueAction, RegistryValueActivity,
};
use atlas_schema::*;

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
        action,
    }))
}

fn reg_value_event(action: RegistryValueAction) -> Event {
    event(EventKind::RegistryValue(RegistryValueActivity {
        actor: actor(),
        key_path: "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run".into(),
        name: "Updater".into(),
        action,
    }))
}

/// One valid event per (class, activity), named `<class>_<activity>`.
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
        ("registry_key_create", reg_key_event(RegistryKeyAction::Create)),
        ("registry_key_delete", reg_key_event(RegistryKeyAction::Delete)),
        (
            "registry_key_rename",
            reg_key_event(RegistryKeyAction::Rename { prev_path: "HKLM\\SOFTWARE\\Atlas\\Old".into() }),
        ),
        (
            "registry_value_set",
            reg_value_event(RegistryValueAction::Set {
                value_type: RegValueType::Sz,
                data: "C:\\Users\\Public\\u.exe\0".encode_utf16().flat_map(u16::to_le_bytes).collect(),
                data_truncated: false,
            }),
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
    ]
}
```

- [ ] **Step 2: Write the failing integration tests**

`crates/atlas-schema/tests/roundtrip.rs` (Task 8 adds a property test):
```rust
//! Domain → wire → domain must be lossless for every valid event (spec 8.1).

mod common;

use atlas_schema::{decode_event, encode_event};

#[test]
fn every_sample_round_trips() {
    for (name, event) in common::samples() {
        let decoded = decode_event(&encode_event(event.clone())).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(decoded, event, "{name}");
    }
}
```

`crates/atlas-schema/tests/ocsf_ids.rs`:
```rust
//! Derived OCSF ids must equal OCSF 1.9.0 (spec 5.0).

mod common;

/// (sample, category_uid, class_uid, activity_id) copied from spec 5.0.
const EXPECTED: &[(&str, u32, u32, u32)] = &[
    ("process_launch", 1, 1007, 1),
    ("process_terminate", 1, 1007, 2),
    ("module_load", 1, 1005, 1),
    ("network_open", 4, 4001, 1),
    ("network_close", 4, 4001, 2),
    ("file_create", 1, 1001, 1),
    ("file_read", 1, 1001, 2),
    ("file_update", 1, 1001, 3),
    ("file_delete", 1, 1001, 4),
    ("file_rename", 1, 1001, 5),
    ("file_set_attributes", 1, 1001, 6),
    ("registry_key_create", 1, 201001, 1),
    ("registry_key_delete", 1, 201001, 4),
    ("registry_key_rename", 1, 201001, 5),
    ("registry_value_set", 1, 201002, 2),
    ("registry_value_delete", 1, 201002, 4),
    ("dns_response", 4, 4003, 2),
];

#[test]
fn derived_ids_match_ocsf_1_9_0() {
    let samples = common::samples();
    assert_eq!(samples.len(), EXPECTED.len(), "every sample needs an expected row");
    for (name, event) in samples {
        let &(_, category, class, activity) =
            EXPECTED.iter().find(|row| row.0 == name).unwrap_or_else(|| panic!("no expected ids for {name}"));
        let ids = event.kind.ocsf_ids();
        assert_eq!((ids.category_uid, ids.class_uid, ids.activity_id), (category, class, activity), "{name}");
        assert_eq!(ids.type_uid(), u64::from(class) * 100 + u64::from(activity), "{name}");
    }
}

#[test]
fn type_uid_examples() {
    let ids = |name: &str| common::samples().into_iter().find(|(n, _)| *n == name).unwrap().1.kind.ocsf_ids();
    assert_eq!(ids("process_launch").type_uid(), 100_701);
    assert_eq!(ids("registry_value_set").type_uid(), 20_100_202);
}
```

`crates/atlas-schema/tests/validation.rs`. This is one row per rule in §6.1, plus the at-limit cases and the Review Focus items 1–5:
```rust
//! Every validation rule in spec 6.1 has a negative case here, asserting the
//! exact field path and error kind. Boundary cases (exactly at a limit) must pass.

mod common;

use atlas_schema::limits::*;
use atlas_schema::wire::{self, event::Kind};
use atlas_schema::{Event, SchemaError, SchemaErrorKind as K, decode_event};
use prost::Message;

fn wire_sample(name: &str) -> wire::Event {
    let (_, event) = common::samples().into_iter().find(|(n, _)| *n == name).expect("sample exists");
    wire::Event::from(event)
}

fn check(w: wire::Event) -> Result<Event, SchemaError> {
    Event::try_from(w)
}

// ---- accessors into the wire tree (panic if the sample has another shape) ----

fn launch(w: &mut wire::Event) -> &mut wire::ProcessLaunch {
    use wire::process_activity::Activity;
    match w.kind.as_mut() {
        Some(Kind::Process(wire::ProcessActivity { activity: Some(Activity::Launch(l)) })) => l,
        _ => panic!("not a launch"),
    }
}

fn launch_process(w: &mut wire::Event) -> &mut wire::Process {
    launch(w).process.as_mut().expect("process")
}

fn launch_file(w: &mut wire::Event) -> &mut wire::File {
    launch_process(w).file.as_mut().expect("file")
}

fn network(w: &mut wire::Event) -> &mut wire::NetworkActivity {
    match w.kind.as_mut() {
        Some(Kind::Network(n)) => n,
        _ => panic!("not network"),
    }
}

fn file_activity(w: &mut wire::Event) -> &mut wire::FileSystemActivity {
    match w.kind.as_mut() {
        Some(Kind::File(f)) => f,
        _ => panic!("not file"),
    }
}

fn reg_key(w: &mut wire::Event) -> &mut wire::RegistryKeyActivity {
    match w.kind.as_mut() {
        Some(Kind::RegistryKey(k)) => k,
        _ => panic!("not registry key"),
    }
}

fn reg_value(w: &mut wire::Event) -> &mut wire::RegistryValueActivity {
    match w.kind.as_mut() {
        Some(Kind::RegistryValue(v)) => v,
        _ => panic!("not registry value"),
    }
}

fn reg_value_set(w: &mut wire::Event) -> &mut wire::RegistryValueSet {
    use wire::registry_value_activity::Activity;
    match reg_value(w).activity.as_mut() {
        Some(Activity::Set(s)) => s,
        _ => panic!("not a set"),
    }
}

fn dns_activity(w: &mut wire::Event) -> &mut wire::DnsActivity {
    match w.kind.as_mut() {
        Some(Kind::Dns(d)) => d,
        _ => panic!("not dns"),
    }
}

fn dns_response(w: &mut wire::Event) -> &mut wire::DnsResponse {
    use wire::dns_activity::Activity;
    match dns_activity(w).activity.as_mut() {
        Some(Activity::Response(r)) => r,
        None => panic!("no response"),
    }
}

fn long(n: usize) -> String {
    "a".repeat(n)
}

// ---- the rule table ----

type Mutation = fn(&mut wire::Event);

const CASES: &[(&str, Mutation, &str, K)] = &[
    // envelope
    ("process_launch", |w| w.event_id.truncate(15), "event_id", K::Malformed),
    ("process_launch", |w| w.event_id[6] = 0x40, "event_id", K::Malformed), // version 4, not 7
    ("process_launch", |w| w.sensor = 0, "sensor", K::Missing),
    ("process_launch", |w| w.sensor = 99, "sensor", K::UnknownEnum),
    ("process_launch", |w| w.device = None, "device", K::Missing),
    ("process_launch", |w| w.device.as_mut().unwrap().uid.truncate(15), "device.uid", K::Malformed),
    ("process_launch", |w| w.device.as_mut().unwrap().boot_id.clear(), "device.boot_id", K::Malformed),
    ("process_launch", |w| w.kind = None, "kind", K::Missing),
    // process
    (
        "process_launch",
        |w| match w.kind.as_mut() {
            Some(Kind::Process(p)) => p.activity = None,
            _ => unreachable!(),
        },
        "activity",
        K::Missing,
    ),
    ("process_launch", |w| launch(w).actor = None, "actor.process", K::Missing),
    ("process_launch", |w| launch(w).actor.as_mut().unwrap().file = None, "actor.process.file", K::Missing),
    ("process_launch", |w| launch(w).process = None, "process", K::Missing),
    ("process_launch", |w| launch_process(w).file = None, "process.file", K::Missing),
    ("process_launch", |w| launch_file(w).path = long(PATH_MAX + 1), "process.file.path", K::TooLarge),
    ("process_launch", |w| launch_file(w).name = long(PATH_MAX + 1), "process.file.name", K::TooLarge),
    ("process_launch", |w| launch_process(w).cmd_line = long(CMD_LINE_MAX + 1), "process.cmd_line", K::TooLarge),
    ("process_launch", |w| launch_process(w).uid.push(0), "process.uid", K::Malformed),
    ("process_launch", |w| launch_process(w).integrity = Some(0), "process.integrity", K::Missing),
    ("process_launch", |w| launch_process(w).integrity = Some(42), "process.integrity", K::UnknownEnum),
    (
        "process_launch",
        |w| launch_process(w).parent_process.as_mut().unwrap().uid.truncate(3),
        "process.parent_process.uid",
        K::Malformed,
    ),
    (
        "process_launch",
        |w| launch_file(w).hashes.as_mut().unwrap().sha256.as_mut().unwrap().truncate(31),
        "process.file.hashes.sha256",
        K::Malformed,
    ),
    (
        "process_launch",
        |w| launch_file(w).signature.as_mut().unwrap().status = 0,
        "process.file.signature.status",
        K::Missing,
    ),
    (
        "process_terminate",
        |w| match w.kind.as_mut() {
            Some(Kind::Process(wire::ProcessActivity {
                activity: Some(wire::process_activity::Activity::Terminate(t)),
            })) => t.process = None,
            _ => unreachable!(),
        },
        "process",
        K::Missing,
    ),
    // module
    (
        "module_load",
        |w| match w.kind.as_mut() {
            Some(Kind::Module(wire::ModuleActivity {
                activity: Some(wire::module_activity::Activity::Load(l)),
                ..
            })) => l.file = None,
            _ => unreachable!(),
        },
        "module.file",
        K::Missing,
    ),
    (
        "module_load",
        |w| match w.kind.as_mut() {
            Some(Kind::Module(m)) => m.actor = None,
            _ => unreachable!(),
        },
        "actor.process",
        K::Missing,
    ),
    // network
    ("network_open", |w| network(w).src_endpoint = None, "src_endpoint", K::Missing),
    (
        "network_open",
        |w| network(w).dst_endpoint.as_mut().unwrap().ip = vec![1, 2, 3, 4, 5],
        "dst_endpoint.ip",
        K::Malformed,
    ),
    ("network_open", |w| network(w).src_endpoint.as_mut().unwrap().port = 70_000, "src_endpoint.port", K::Malformed),
    ("network_open", |w| network(w).protocol = 0, "protocol", K::Missing),
    ("network_open", |w| network(w).direction = 9, "direction", K::UnknownEnum),
    ("network_open", |w| network(w).activity = None, "activity", K::Missing),
    // file
    ("file_create", |w| file_activity(w).file = None, "file", K::Missing),
    (
        "file_rename",
        |w| match file_activity(w).activity.as_mut() {
            Some(wire::file_system_activity::Activity::Rename(r)) => r.file_result = None,
            _ => unreachable!(),
        },
        "file_result",
        K::Missing,
    ),
    (
        "file_rename",
        |w| match file_activity(w).activity.as_mut() {
            Some(wire::file_system_activity::Activity::Rename(r)) => {
                r.file_result.as_mut().unwrap().path = long(PATH_MAX + 1)
            }
            _ => unreachable!(),
        },
        "file_result.path",
        K::TooLarge,
    ),
    // registry key
    ("registry_key_create", |w| reg_key(w).path = long(PATH_MAX + 1), "reg_key.path", K::TooLarge),
    (
        "registry_key_rename",
        |w| match reg_key(w).activity.as_mut() {
            Some(wire::registry_key_activity::Activity::Rename(r)) => r.prev_path = long(PATH_MAX + 1),
            _ => unreachable!(),
        },
        "prev_reg_key.path",
        K::TooLarge,
    ),
    // registry value
    ("registry_value_set", |w| reg_value(w).key_path = long(PATH_MAX + 1), "reg_value.path", K::TooLarge),
    ("registry_value_set", |w| reg_value(w).name = long(PATH_MAX + 1), "reg_value.name", K::TooLarge),
    ("registry_value_set", |w| reg_value_set(w).r#type = 12, "reg_value.type", K::UnknownEnum),
    ("registry_value_set", |w| reg_value_set(w).data = vec![0; REG_DATA_MAX + 1], "reg_value.data", K::TooLarge),
    // dns
    ("dns_response", |w| dns_activity(w).hostname = long(DNS_HOSTNAME_MAX + 1), "query.hostname", K::TooLarge),
    ("dns_response", |w| dns_activity(w).query_type = 65_536, "query.type", K::Malformed),
    ("dns_response", |w| dns_response(w).rcode = Some(70_000), "rcode", K::Malformed),
    (
        "dns_response",
        |w| {
            let a = dns_response(w).answers[0].clone();
            dns_response(w).answers = vec![a; DNS_ANSWERS_MAX + 1];
        },
        "answers",
        K::TooLarge,
    ),
    (
        "dns_response",
        |w| {
            let r = dns_response(w);
            let a = r.answers[0].clone();
            r.answers = vec![a.clone(), a.clone(), wire::DnsAnswer { data: long(DNS_ANSWER_DATA_MAX + 1), ..a }];
        },
        "answers[2].data",
        K::TooLarge,
    ),
    ("dns_response", |w| dns_response(w).answers[0].r#type = 70_000, "answers[0].type", K::Malformed),
];

#[test]
fn every_rule_rejects_with_exact_path_and_kind() {
    for (i, (sample, mutate, path, kind)) in CASES.iter().enumerate() {
        let mut w = wire_sample(sample);
        mutate(&mut w);
        let err = check(w).expect_err(&format!("case {i} ({path}) should be rejected"));
        assert_eq!((err.field_path.as_str(), err.kind), (*path, *kind), "case {i}");
    }
}

// ---- boundaries: exactly at the limit is accepted ----

const AT_LIMIT: &[(&str, Mutation)] = &[
    ("process_launch", |w| launch_file(w).path = long(PATH_MAX)),
    ("process_launch", |w| launch_process(w).cmd_line = long(CMD_LINE_MAX)),
    ("registry_value_set", |w| reg_value_set(w).data = vec![0; REG_DATA_MAX]),
    ("dns_response", |w| dns_activity(w).hostname = long(DNS_HOSTNAME_MAX)),
    ("dns_response", |w| {
        let a = dns_response(w).answers[0].clone();
        dns_response(w).answers = vec![a; DNS_ANSWERS_MAX];
    }),
    ("network_open", |w| network(w).src_endpoint.as_mut().unwrap().port = 65_535),
];

#[test]
fn values_exactly_at_limits_are_accepted() {
    for (i, (sample, mutate)) in AT_LIMIT.iter().enumerate() {
        let mut w = wire_sample(sample);
        mutate(&mut w);
        check(w).unwrap_or_else(|e| panic!("case {i}: {e}"));
    }
}

// ---- optional fields may be absent ----

#[test]
fn optional_fields_may_be_absent() {
    let mut w = wire_sample("process_launch");
    let p = launch_process(&mut w);
    p.user = None;
    p.integrity = None;
    p.parent_process = None;
    let f = p.file.as_mut().unwrap();
    f.hashes = None;
    f.signature = None;
    check(w).expect("optional fields are optional");
}

#[test]
fn empty_strings_are_allowed() {
    // proto3 cannot tell "absent" from "empty"; e.g. the System process has no image path.
    let mut w = wire_sample("process_launch");
    launch_file(&mut w).path.clear();
    launch_process(&mut w).cmd_line.clear();
    check(w).expect("empty strings are valid");
}

// ---- byte-level checks in decode_event ----

#[test]
fn oversized_input_is_rejected_before_decoding() {
    let err = decode_event(&vec![0u8; EVENT_MAX + 1]).unwrap_err();
    assert_eq!((err.field_path.as_str(), err.kind), ("event", K::TooLarge));
}

#[test]
fn undecodable_bytes_are_malformed() {
    let err = decode_event(&[0xff, 0xff, 0xff]).unwrap_err();
    assert_eq!((err.field_path.as_str(), err.kind), ("event", K::Malformed));
}

#[test]
fn invalid_utf8_in_a_string_is_malformed() {
    let mut bytes = wire_sample("registry_key_create").encode_to_vec();
    let at = bytes.windows(4).position(|w| w == b"HKLM").expect("path bytes present");
    bytes[at] = 0xff;
    let err = decode_event(&bytes).unwrap_err();
    assert_eq!((err.field_path.as_str(), err.kind), ("event", K::Malformed));
}

#[test]
fn class_from_a_newer_schema_is_rejected_as_missing_kind() {
    // A newer agent might send oneof field 17 (a class this build does not know).
    // prost keeps it as an unknown field, so `kind` is absent.
    let mut w = wire_sample("process_launch");
    w.kind = None;
    let mut bytes = w.encode_to_vec();
    bytes.extend_from_slice(&[0x8a, 0x01, 0x00]); // field 17, wire type 2, length 0
    let err = decode_event(&bytes).unwrap_err();
    assert_eq!((err.field_path.as_str(), err.kind), ("kind", K::Missing));
}

#[test]
fn activity_from_a_newer_schema_is_rejected_as_missing_activity() {
    // A newer agent might send ProcessActivity oneof field 3 (e.g. Inject).
    let mut w = wire_sample("process_launch");
    w.kind = None;
    let mut bytes = w.encode_to_vec();
    let process_activity = [0x1a, 0x00]; // field 3, wire type 2, length 0
    bytes.extend_from_slice(&[0x52, process_activity.len() as u8]); // Event field 10 (process)
    bytes.extend_from_slice(&process_activity);
    let err = decode_event(&bytes).unwrap_err();
    assert_eq!((err.field_path.as_str(), err.kind), ("activity", K::Missing));
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p atlas-schema --tests`
Expected: FAIL to compile, with `unresolved imports atlas_schema::Event, decode_event, encode_event, wire, Device, …`.

- [ ] **Step 4: Write the envelope, OCSF derivation, and codec**

`crates/atlas-schema/src/event.rs`:
```rust
//! The event envelope (spec sections 3.1, 4.1, 4.2).

use atlas_proto::v1 as wire;
use atlas_proto::v1::event::Kind as W;
use uuid::Uuid;

use crate::classes::dns::DnsActivity;
use crate::classes::file::FileSystemActivity;
use crate::classes::module::ModuleActivity;
use crate::classes::network::NetworkActivity;
use crate::classes::process::ProcessActivity;
use crate::classes::registry::{RegistryKeyActivity, RegistryValueActivity};
use crate::convert::{Result, err, fixed, require, wire_enum};
use crate::error::{SchemaError, SchemaErrorKind};
use crate::ids::{BootId, DeviceUid, EventId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub meta: EventMeta,
    pub device: Device,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventMeta {
    pub event_id: EventId,
    /// When the event occurred: nanoseconds since the Unix epoch, UTC.
    pub time: i64,
    pub sensor: Sensor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensor {
    Etw,
    Driver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    /// Must be checked against the agent's authenticated identity by the server.
    pub uid: DeviceUid,
    pub boot_id: BootId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Process(ProcessActivity),
    Module(ModuleActivity),
    Network(NetworkActivity),
    File(FileSystemActivity),
    RegistryKey(RegistryKeyActivity),
    RegistryValue(RegistryValueActivity),
    Dns(DnsActivity),
}

impl From<Event> for wire::Event {
    fn from(v: Event) -> Self {
        let sensor = match v.meta.sensor {
            Sensor::Etw => wire::Sensor::Etw,
            Sensor::Driver => wire::Sensor::Driver,
        };
        let kind = match v.kind {
            EventKind::Process(a) => W::Process(a.into()),
            EventKind::Module(a) => W::Module(a.into()),
            EventKind::Network(a) => W::Network(a.into()),
            EventKind::File(a) => W::File(a.into()),
            EventKind::RegistryKey(a) => W::RegistryKey(a.into()),
            EventKind::RegistryValue(a) => W::RegistryValue(a.into()),
            EventKind::Dns(a) => W::Dns(a.into()),
        };
        Self {
            event_id: v.meta.event_id.as_bytes().to_vec(),
            time: v.meta.time,
            sensor: sensor as i32,
            device: Some(wire::Device {
                uid: v.device.uid.as_bytes().to_vec(),
                boot_id: v.device.boot_id.as_bytes().to_vec(),
            }),
            kind: Some(kind),
        }
    }
}

/// The validation gate for untrusted wire data (spec section 6).
impl TryFrom<wire::Event> for Event {
    type Error = SchemaError;

    fn try_from(w: wire::Event) -> Result<Self> {
        let Some(event_id) = EventId::from_uuid(Uuid::from_bytes(fixed::<16>(w.event_id, "", "event_id")?)) else {
            return err("", "event_id", SchemaErrorKind::Malformed);
        };
        let sensor = wire_enum(
            w.sensor,
            |s: wire::Sensor| match s {
                wire::Sensor::Unspecified => None,
                wire::Sensor::Etw => Some(Sensor::Etw),
                wire::Sensor::Driver => Some(Sensor::Driver),
            },
            "",
            "sensor",
        )?;
        let device = require(w.device, "", "device")?;
        let device = Device {
            uid: DeviceUid::from_bytes(fixed::<16>(device.uid, "device", "uid")?),
            boot_id: BootId::from_bytes(fixed::<16>(device.boot_id, "device", "boot_id")?),
        };
        let kind = match require(w.kind, "", "kind")? {
            W::Process(a) => EventKind::Process(ProcessActivity::from_wire(a)?),
            W::Module(a) => EventKind::Module(ModuleActivity::from_wire(a)?),
            W::Network(a) => EventKind::Network(NetworkActivity::from_wire(a)?),
            W::File(a) => EventKind::File(FileSystemActivity::from_wire(a)?),
            W::RegistryKey(a) => EventKind::RegistryKey(RegistryKeyActivity::from_wire(a)?),
            W::RegistryValue(a) => EventKind::RegistryValue(RegistryValueActivity::from_wire(a)?),
            W::Dns(a) => EventKind::Dns(DnsActivity::from_wire(a)?),
        };
        Ok(Self { meta: EventMeta { event_id, time: w.time, sensor }, device, kind })
    }
}
```

`crates/atlas-schema/src/ocsf.rs`:
```rust
//! OCSF 1.9.0 numeric ids, derived from the domain types (spec section 5.0).
//! They are never stored, so they cannot disagree with the event.

use crate::classes::dns::DnsAction;
use crate::classes::file::FileAction;
use crate::classes::module::ModuleAction;
use crate::classes::network::NetworkAction;
use crate::classes::process::ProcessActivity;
use crate::classes::registry::{RegistryKeyAction, RegistryValueAction};
use crate::event::EventKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcsfIds {
    pub category_uid: u32,
    pub class_uid: u32,
    pub activity_id: u32,
}

impl OcsfIds {
    /// `class_uid * 100 + activity_id`, as OCSF requires.
    pub const fn type_uid(&self) -> u64 {
        self.class_uid as u64 * 100 + self.activity_id as u64
    }
}

const SYSTEM: u32 = 1;
const NETWORK: u32 = 4;

impl EventKind {
    pub fn ocsf_ids(&self) -> OcsfIds {
        let (category_uid, class_uid, activity_id) = match self {
            EventKind::Process(a) => (
                SYSTEM,
                1007,
                match a {
                    ProcessActivity::Launch { .. } => 1,
                    ProcessActivity::Terminate { .. } => 2,
                },
            ),
            EventKind::Module(a) => (
                SYSTEM,
                1005,
                match a.action {
                    ModuleAction::Load { .. } => 1,
                },
            ),
            EventKind::Network(a) => (
                NETWORK,
                4001,
                match a.action {
                    NetworkAction::Open => 1,
                    NetworkAction::Close { .. } => 2,
                },
            ),
            EventKind::File(a) => (
                SYSTEM,
                1001,
                match a.action {
                    FileAction::Create => 1,
                    FileAction::Read => 2,
                    FileAction::Update => 3,
                    FileAction::Delete => 4,
                    FileAction::Rename { .. } => 5,
                    FileAction::SetAttributes => 6,
                },
            ),
            EventKind::RegistryKey(a) => (
                SYSTEM,
                201001,
                match a.action {
                    RegistryKeyAction::Create => 1,
                    RegistryKeyAction::Delete => 4,
                    RegistryKeyAction::Rename { .. } => 5,
                },
            ),
            EventKind::RegistryValue(a) => (
                SYSTEM,
                201002,
                match a.action {
                    RegistryValueAction::Set { .. } => 2,
                    RegistryValueAction::Delete => 4,
                },
            ),
            EventKind::Dns(a) => (
                NETWORK,
                4003,
                match a.action {
                    DnsAction::Response { .. } => 2,
                },
            ),
        };
        OcsfIds { category_uid, class_uid, activity_id }
    }
}
```

`crates/atlas-schema/src/codec.rs`:
```rust
//! Byte-level encode/decode: the entry points agents and the server use.

use atlas_proto::v1 as wire;
use prost::Message;

use crate::error::{SchemaError, SchemaErrorKind};
use crate::event::Event;
use crate::limits::EVENT_MAX;

/// Encodes a domain event to protobuf bytes. Infallible.
pub fn encode_event(event: Event) -> Vec<u8> {
    wire::Event::from(event).encode_to_vec()
}

/// Decodes and validates untrusted bytes. The size limit is checked before
/// any protobuf parsing.
pub fn decode_event(bytes: &[u8]) -> Result<Event, SchemaError> {
    if bytes.len() > EVENT_MAX {
        return Err(SchemaError::new("event", SchemaErrorKind::TooLarge));
    }
    let wire = wire::Event::decode(bytes).map_err(|_| SchemaError::new("event", SchemaErrorKind::Malformed))?;
    Event::try_from(wire)
}
```

Replace `crates/atlas-schema/src/lib.rs` with its final form:
```rust
//! Atlas event schema: the typed domain model every Atlas component uses.
//!
//! - Domain → wire is infallible (`From`); see [`encode_event`].
//! - Wire → domain validates untrusted input (`TryFrom`); see [`decode_event`].
//!
//! Design: `docs/specs/2026-09-24-event-schema-design.md`.

pub mod classes;
mod codec;
mod convert;
mod error;
mod event;
mod ids;
pub mod limits;
mod objects;
mod ocsf;

pub use codec::{decode_event, encode_event};
pub use error::{SchemaError, SchemaErrorKind};
pub use event::{Device, Event, EventKind, EventMeta, Sensor};
pub use ids::{BootId, DeviceUid, EventId, ProcessUid, process_uid};
pub use objects::{File, Hashes, Integrity, NetworkEndpoint, Process, ProcessRef, Signature, SignatureStatus, User};
pub use ocsf::OcsfIds;

/// Generated wire types, re-exported for transport code.
pub use atlas_proto::v1 as wire;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p atlas-schema`
Expected: PASS, with no warnings:
- 35 lib tests
- `roundtrip`: 1 test
- `ocsf_ids`: 2 tests
- `validation`: 9 tests, including the table of 43 rejection cases in `every_rule_rejects_with_exact_path_and_kind`

- [ ] **Step 6: Commit**

```powershell
git add crates/atlas-schema
git commit -m "feat(atlas-schema): add event envelope, codec, OCSF ids, validation suite" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01TqnCZsFoaabfHYyb1ieVfc"
```

---

### Task 8: Property tests: lossless round-trip and hostile input

**Files:**
- Modify: `crates/atlas-schema/tests/common/mod.rs` (add the strategies)
- Modify: `crates/atlas-schema/tests/roundtrip.rs` (add the property test)
- Create: `crates/atlas-schema/tests/hostile_input.rs`

**Interfaces:**
- Consumes: `samples()` and the public API from Task 7.
- Produces:
  - `tests/common/mod.rs::arb_event() -> BoxedStrategy<Event>` (plus `arb_kind`, `arb_file`, `arb_process_ref`, `arb_process`, `arb_endpoint`, `arb_user`, `arb_event_id`, `arb_integrity`, `arb_string`)
  - The benchmark in Task 10 reuses this file.

These tests describe properties the Task 7 code should already have, so they are expected to **pass on first run**. If a property fails, that's a real bug:
- Use superpowers:systematic-debugging.
- Fix the conversion code.
- Add the minimized proptest input as a named regression case to `tests/validation.rs` or to the class's inline tests.

- [ ] **Step 1: Add the strategies to `tests/common/mod.rs`**

Add `use proptest::prelude::*;` to the imports, then append:

```rust
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

fn arb_reg_value_type() -> impl Strategy<Value = RegValueType> {
    (0u32..=11).prop_map(|raw| RegValueType::from_raw(raw).expect("0..=11 are valid"))
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
    ];
    let file = (arb_process_ref(), arb_file(), file_action)
        .prop_map(|(actor, file, action)| EventKind::File(FileSystemActivity { actor, file, action }));

    let key_action = prop_oneof![
        Just(RegistryKeyAction::Create),
        Just(RegistryKeyAction::Delete),
        arb_string(40).prop_map(|prev_path| RegistryKeyAction::Rename { prev_path }),
    ];
    let reg_key = (arb_process_ref(), arb_string(40), key_action)
        .prop_map(|(actor, path, action)| EventKind::RegistryKey(RegistryKeyActivity { actor, path, action }));

    let value_action = prop_oneof![
        (arb_reg_value_type(), prop::collection::vec(any::<u8>(), 0..64), any::<bool>()).prop_map(
            |(value_type, data, data_truncated)| RegistryValueAction::Set { value_type, data, data_truncated }
        ),
        Just(RegistryValueAction::Delete),
    ];
    let reg_value = (arb_process_ref(), arb_string(40), arb_string(20), value_action).prop_map(
        |(actor, key_path, name, action)| {
            EventKind::RegistryValue(RegistryValueActivity { actor, key_path, name, action })
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

    prop_oneof![
        process.boxed(),
        module.boxed(),
        network.boxed(),
        file.boxed(),
        reg_key.boxed(),
        reg_value.boxed(),
        dns.boxed()
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

The strategies return `BoxedStrategy` on purpose. Unboxed nested strategy types overflow the default Windows test-thread stack in debug builds (`STATUS_STACK_OVERFLOW`).

- [ ] **Step 2: Add the round-trip property**

Replace `crates/atlas-schema/tests/roundtrip.rs` with:
```rust
//! Domain → wire → domain must be lossless for every valid event (spec 8.1).

mod common;

use atlas_schema::{decode_event, encode_event};
use proptest::prelude::*;

#[test]
fn every_sample_round_trips() {
    for (name, event) in common::samples() {
        let decoded = decode_event(&encode_event(event.clone())).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(decoded, event, "{name}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn arbitrary_valid_events_round_trip(event in common::arb_event()) {
        let decoded = decode_event(&encode_event(event.clone())).expect("valid event must decode");
        prop_assert_eq!(decoded, event);
    }
}
```

- [ ] **Step 3: Add the hostile-input properties**

`crates/atlas-schema/tests/hostile_input.rs`:
```rust
//! Stable-Rust fuzzing: arbitrary and corrupted bytes must never panic.
//! (The cargo-fuzz target in `fuzz/` does the same with coverage guidance.)

mod common;

use atlas_schema::{decode_event, encode_event};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5000))]

    #[test]
    fn random_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let _ = decode_event(&bytes);
    }

    #[test]
    fn corrupted_valid_events_never_panic(
        event in common::arb_event(),
        flips in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8),
    ) {
        let mut bytes = encode_event(event);
        for (at, value) in flips {
            let i = at.index(bytes.len());
            bytes[i] = value;
        }
        let _ = decode_event(&bytes);
    }

    #[test]
    fn truncated_valid_events_never_panic(event in common::arb_event(), cut in any::<prop::sample::Index>()) {
        let bytes = encode_event(event);
        let _ = decode_event(&bytes[..cut.index(bytes.len())]);
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p atlas-schema --test roundtrip --test hostile_input`
Expected: PASS. `roundtrip` runs 2 tests (2,000 generated events). `hostile_input` runs 3 tests (5,000 cases each), in about a second.

- [ ] **Step 5: Commit**

```powershell
git add crates/atlas-schema/tests
git commit -m "test(atlas-schema): add round-trip and hostile-input property tests" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01TqnCZsFoaabfHYyb1ieVfc"
```

---

### Task 9: Golden protobuf-JSON fixtures

**Files:**
- Create: `crates/atlas-schema/tests/golden.rs`
- Create: `crates/atlas-schema/tests/fixtures/*.json` (17 files, generated then reviewed)

**Interfaces:**
- Consumes: `samples()` (Task 7), `atlas_proto::FILE_DESCRIPTOR_SET` (Task 1), `encode_event`/`decode_event`.
- Produces: `tests/fixtures/<sample>.json`, which Task 11's reference doc points readers to.

- [ ] **Step 1: Write the golden test**

`crates/atlas-schema/tests/golden.rs`:
```rust
//! Golden fixtures: one protobuf-JSON file per class/activity in
//! `tests/fixtures/`. They document what events look like on the wire and
//! pin the encoding. Regenerate with `ATLAS_UPDATE_FIXTURES=1 cargo test -p atlas-schema --test golden`
//! and review the diff before committing.

mod common;

use std::path::PathBuf;

use atlas_schema::{decode_event, encode_event};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};

fn event_descriptor() -> MessageDescriptor {
    DescriptorPool::decode(atlas_proto::FILE_DESCRIPTOR_SET)
        .expect("descriptor set decodes")
        .get_message_by_name("atlas.events.v1.Event")
        .expect("Event message exists")
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(format!("{name}.json"))
}

fn to_json(bytes: &[u8]) -> String {
    let msg = DynamicMessage::decode(event_descriptor(), bytes).expect("decodes");
    let mut json = serde_json::to_string_pretty(&msg).expect("serializes");
    json.push('\n');
    json
}

fn from_json(json: &str) -> Vec<u8> {
    let mut de = serde_json::Deserializer::from_str(json);
    let msg = DynamicMessage::deserialize(event_descriptor(), &mut de).expect("fixture is valid protobuf-JSON");
    de.end().expect("no trailing data");
    msg.encode_to_vec()
}

#[test]
fn fixtures_match_samples() {
    let update = std::env::var_os("ATLAS_UPDATE_FIXTURES").is_some();
    for (name, event) in common::samples() {
        let path = fixture_path(name);
        if update {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, to_json(&encode_event(event.clone()))).unwrap();
        }
        let json = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing {}; run with ATLAS_UPDATE_FIXTURES=1", path.display()));
        let decoded = decode_event(&from_json(&json)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(decoded, event, "{name}: fixture and sample disagree");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p atlas-schema --test golden`
Expected: FAIL with `missing …\tests/fixtures\process_launch.json; run with ATLAS_UPDATE_FIXTURES=1`.

- [ ] **Step 3: Generate the fixtures**

```powershell
$env:ATLAS_UPDATE_FIXTURES = "1"; cargo test -p atlas-schema --test golden; Remove-Item Env:ATLAS_UPDATE_FIXTURES
```
Expected: PASS, and 17 `.json` files now exist in `crates/atlas-schema/tests/fixtures/`.

- [ ] **Step 4: Review the fixtures by hand**

Open `process_launch.json` and `registry_value_set.json` and check:
- The top-level keys are `eventId`, `time` (a **string**, since protobuf-JSON writes int64 as strings), `sensor: "SENSOR_ETW"`, `device`, and exactly one class key.
- `process_launch.json` has `process.launch.actor.file.path` = `C:\\Program Files\\Microsoft Office\\root\\Office16\\WINWORD.EXE`, `cmdLine` = `powershell.exe -enc SQBFAFgA`, `integrity` = `INTEGRITY_MEDIUM`, and `parentProcess` present.
- `registry_value_set.json` has `set.type` = `1` (REG_SZ) and base64 `data`.

If anything looks wrong, fix the sample in `tests/common/mod.rs` (not the JSON) and regenerate.

- [ ] **Step 5: Run it again without the flag**

Run: `cargo test -p atlas-schema --test golden`
Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add crates/atlas-schema/tests/golden.rs crates/atlas-schema/tests/fixtures
git commit -m "test(atlas-schema): add golden protobuf-JSON fixtures for every activity" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01TqnCZsFoaabfHYyb1ieVfc"
```

---

### Task 10: Benchmark baseline and fuzz target

**Files:**
- Modify: `crates/atlas-schema/Cargo.toml` (add `[[bench]]`)
- Create: `crates/atlas-schema/benches/codec.rs`
- Create: `crates/atlas-schema/fuzz/Cargo.toml`, `crates/atlas-schema/fuzz/fuzz_targets/decode_event.rs`

**Interfaces:**
- Consumes: `samples()` (via `#[path]`), `encode_event`, `decode_event`, `wire::Event`.
- Produces: the recorded baseline numbers (used in Task 11's reference doc) and the fuzz run result.

- [ ] **Step 1: Add the bench target**

Append to `crates/atlas-schema/Cargo.toml`:
```toml

[[bench]]
name = "codec"
harness = false
```

`crates/atlas-schema/benches/codec.rs`:
```rust
//! Baseline throughput for encode, decode+validate, and raw protobuf decode
//! over one sample of every class/activity. Not a gate (spec 8.1).

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;

use atlas_schema::{decode_event, encode_event, wire};
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use prost::Message;

fn codec(c: &mut Criterion) {
    let events: Vec<_> = common::samples().into_iter().map(|(_, e)| e).collect();
    let encoded: Vec<Vec<u8>> = events.iter().cloned().map(encode_event).collect();

    let mut group = c.benchmark_group("codec");
    group.throughput(Throughput::Elements(events.len() as u64));

    group.bench_function("encode", |b| {
        b.iter_batched(
            || events.clone(),
            |events| {
                for e in events {
                    black_box(encode_event(e));
                }
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("decode_and_validate", |b| {
        b.iter(|| {
            for bytes in &encoded {
                black_box(decode_event(black_box(bytes)).expect("valid"));
            }
        })
    });

    group.bench_function("protobuf_decode_only", |b| {
        b.iter(|| {
            for bytes in &encoded {
                black_box(wire::Event::decode(black_box(bytes.as_slice())).expect("valid"));
            }
        })
    });

    group.finish();
}

criterion_group!(benches, codec);
criterion_main!(benches);
```

- [ ] **Step 2: Run the benchmark and record the baseline**

Run: `cargo bench -p atlas-schema --bench codec -- --warm-up-time 1 --measurement-time 3`
Expected: three results, `codec/encode`, `codec/decode_and_validate` and `codec/protobuf_decode_only`, each with `time:` and `thrpt:` lines. On the dev PC at plan time, `protobuf_decode_only` was about 22 µs for the 17 samples (about 770 K events/s). Write down the middle `thrpt` value for each; Task 11 records them.

- [ ] **Step 3: Create the fuzz crate**

`crates/atlas-schema/fuzz/Cargo.toml`:
```toml
[package]
name = "atlas-schema-fuzz"
version = "0.0.0"
edition = "2024"
publish = false

[package.metadata]
cargo-fuzz = true

[dependencies]
atlas-schema = { path = ".." }
libfuzzer-sys = "0.4"

[[bin]]
name = "decode_event"
path = "fuzz_targets/decode_event.rs"
test = false
doc = false
bench = false

# Standalone workspace: cargo-fuzz needs nightly + sanitizers, so keep it out
# of the main workspace build.
[workspace]
members = ["."]
```

`crates/atlas-schema/fuzz/fuzz_targets/decode_event.rs`:
```rust
#![no_main]

use libfuzzer_sys::fuzz_target;

// Untrusted bytes → decode + validate must never panic, hang, or blow up memory.
fuzz_target!(|data: &[u8]| {
    let _ = atlas_schema::decode_event(data);
});
```

Check that it compiles on stable: `cargo check --manifest-path crates/atlas-schema/fuzz/Cargo.toml`. Expected: finishes with no errors.

- [ ] **Step 4: Run the fuzzer for 10 minutes (nightly, in Docker)**

This needs Docker Desktop running. If it isn't, ask the user to start it; don't start it yourself.

```powershell
docker run --rm -v "${PWD}:/src" -w /src/crates/atlas-schema rustlang/rust:nightly bash -c "cargo install cargo-fuzz --locked -q && cargo fuzz run decode_event -- -max_total_time=600"
```
Expected: libFuzzer runs for about 600 s and ends with a `Done N runs in 600 second(s)` line, with no `crash-`, `timeout-` or `oom-` artifact.

If it finds a crash:
- Reproduce it with `cargo fuzz run decode_event fuzz/artifacts/decode_event/<file>`.
- Fix it (superpowers:systematic-debugging).
- Add the input as a regression test in `tests/validation.rs`, then re-run.

If Docker is unavailable, stop and tell the user. The fuzz run then moves into sub-project 0b's CI, and the reference doc records it as pending.

- [ ] **Step 5: Commit**

```powershell
git add crates/atlas-schema/Cargo.toml crates/atlas-schema/benches crates/atlas-schema/fuzz/Cargo.toml crates/atlas-schema/fuzz/Cargo.lock crates/atlas-schema/fuzz/fuzz_targets Cargo.lock
git commit -m "test(atlas-schema): add codec benchmark and decode_event fuzz target" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01TqnCZsFoaabfHYyb1ieVfc"
```

---

### Task 11: Schema reference doc, spec clarifications, roadmap, final verification

**Files:**
- Create: `docs/schema-reference.md`
- Modify: `docs/specs/2026-09-24-event-schema-design.md` (status + clarifications)
- Modify: `docs/architecture-overview.md` (roadmap + decision log)

**Interfaces:**
- Consumes: the fixtures (Task 9), the benchmark numbers (Task 10) and the fuzz result (Task 10).
- Produces: definition-of-done items 3 and 4 (§8.2).

- [ ] **Step 1: Write `docs/schema-reference.md`**

```markdown
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
```

Then append a performance section using the numbers you recorded in Task 10:
```markdown
## Performance baseline

Recorded <date> on the dev PC (release-mode `cargo bench`, 17 mixed samples; see `crates/atlas-schema/benches/codec.rs`).
Not a gate — budgets belong to sub-project 8.

| Benchmark | Throughput |
|---|---|
| `encode` | <value> events/s |
| `decode_and_validate` | <value> events/s |
| `protobuf_decode_only` | <value> events/s |

Fuzzing: `decode_event` ran clean for 10 minutes on <date> (or: pending — runs in 0b CI).
```
Replace each `<…>` with the actual recorded value or date. Don't leave any angle-bracket text in the file.

- [ ] **Step 2: Apply the spec clarifications**

In `docs/specs/2026-09-24-event-schema-design.md`:
1. Change the status line to `**Status:** Implemented (2026-09-24). Reference: docs/schema-reference.md`.
2. In §5.1, change the `parent_process` bullet to: `` `parent_process` (`ProcessRef`, optional: absent when the OS reports no parent, e.g. early-boot processes; the *claimed* parent — see §5.2) ``.
3. In §6.1, after the **Enums** bullet, add: `- **Newer-schema data:** a class or activity this build does not know (an unknown oneof field) is rejected as `Missing` on `kind` / `activity`.`
4. In §8.1, change the **Golden fixtures** bullet to: `- **Golden fixtures:** one protobuf-JSON file per class/activity in `crates/atlas-schema/tests/fixtures/`, generated from the named samples with `ATLAS_UPDATE_FIXTURES=1` and reviewed by hand before commit; decoded and validated in tests; double as documentation.`
5. In §3.1, after the `atlas-proto` bullet list, add: `- Wire layout: `meta` fields are flattened into `Event` (fields 1–3); per-activity fields live in oneof sub-messages mirroring the domain enums.`

- [ ] **Step 3: Update the roadmap and decision log**

In `docs/architecture-overview.md`:
- In the roadmap table, change row `0a`'s status from `Spec in review` to `Done`.
- Change row `0b`'s status to `Next up`.
- Append to the Decision Log table:
  ```
  | 2026-09-24 | 0a implemented: `atlas-proto` + `atlas-schema`; unknown classes/activities from newer agents are rejected as Missing; `parent_process` optional. |
  ```

- [ ] **Step 4: Run the full verification**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Expected:
- fmt prints nothing
- clippy has no warnings
- all test binaries pass: 35 lib tests, plus `smoke` 2, `roundtrip` 2, `validation` 9, `ocsf_ids` 2, `hostile_input` 3 and `golden` 1

Then check that no placeholders are left: `Select-String -Path docs/schema-reference.md -Pattern '<value>|<date>'` must print nothing.

- [ ] **Step 5: Commit**

```powershell
git add docs
git commit -m "docs: add schema reference, record 0a clarifications and completion" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" -m "Claude-Session: https://claude.ai/code/session_01TqnCZsFoaabfHYyb1ieVfc"
```
