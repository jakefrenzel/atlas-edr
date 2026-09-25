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
| 0a | **Foundations: event schema** | `atlas-proto` + `atlas-schema` crates, OCSF-modeled, 7 event classes ([spec](specs/2026-09-24-event-schema-design.md)) | Done (10-min fuzz run pending in 0b CI) |
| 0b | **Foundations: scaffolding** | Hyper-V test VM setup, CI (incl. `buf breaking` and the 0a `cargo fuzz` `decode_event` 10-min run) | In build ([spec](specs/2026-09-25-scaffolding-design.md), [plan](plans/2026-09-25-scaffolding-plan.md), [runbook](runbooks/edr-test-vm.md)) |
| 1 | **Agent: ETW sensor** | Process / image-load / network / file / registry telemetry → normalized events; on-disk offline buffer | Next up (with 0) |
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
