# Sub-project 0b: Scaffolding (Brainstorm Notes)

**Status:** Superseded by the spec [2026-09-25-scaffolding-design.md](2026-09-25-scaffolding-design.md) (design §3 approved 2026-09-25). Kept as the brainstorm record. **This is not the spec.** It is the handoff between sessions.
The next session resumes the brainstorm at **"Next steps"** below and then writes the real spec
(`docs/specs/<date>-scaffolding-design.md`) from these notes.

**Process position** (per CLAUDE.md: brainstorm → spec → plan → build):
classified **architectural**; clarifying questions done; design Sections 1–2 approved; Section 3 not yet presented.

---

## Scope (decided)

- **0b = CI + the Hyper-V test VM "edr-test".**
- **Docker Compose (ClickHouse, Postgres) moved to sub-project 2**, where its first consumer lives. A Compose file now
  would guess at versions and settings that sub-project 2 will rewrite.
- **Carried into 0b from 0a:**
  - `buf breaking` in CI (0a spec §7).
  - The deferred **10-minute `cargo fuzz` run of `decode_event`** (0a DoD §8.2.2).
- **Carried into sub-project 1** (not 0b), to be verified *using* the VM:
  - Whether ETW ProcessStart `ProcessSequenceNumber` equals `PsGetProcessStartKey`.
  - A stable `boot_time` source.
  - The requester PID in the header of DNS-Client event 3008.

## Decisions made in this brainstorm

| # | Decision | Why |
|---|---|---|
| B1 | Compose moves out of 0b and into sub-project 2 | No consumer until sub-project 2; avoids speculative config. |
| B2 | Repo is **public** (the user flipped it on 2026-09-24) | GitHub-hosted runners are free and unmetered for public repos, so nightly fuzzing and Windows jobs on every push cost nothing. |
| B3 | License **AGPL-3.0-only** (committed as `ae85ad9`) | Open for lab and personal use. Modified network deployments must share their source. The sole copyright holder keeps the dual-licensing option for goal D (product). |
| B4 | VM built by **scripts plus a runbook**, not a fully unattended build and not a runbook alone | Rebuilds are reproducible without the fragility of an autounattend answer file; the Windows install stays a manual step. |
| B5 | VM network is **isolated by default and online on demand** | An internal host-only switch is always attached; a NAT adapter is added only for updates and downloads. Attack simulations always run isolated. |
| B6 | Windows 11 **Enterprise Evaluation** ISO (90 days) | Free, no activation needed; the scripted rebuild every 90 days doubles as a test that the scripts still work. |

## Section 1: CI (approved)

`.github/workflows/ci.yml` runs on every push and PR:

| Job | Runner | Steps |
|---|---|---|
| `rust-linux` | ubuntu-latest | `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`; `cargo check --manifest-path crates/atlas-schema/fuzz/Cargo.toml` |
| `rust-windows` | windows-latest | clippy and tests on MSVC |
| `proto` | ubuntu-latest | `buf lint` and `buf breaking` against `main`; `buf.yaml` in `crates/atlas-proto/proto/` |
| `powershell` | windows-latest | PSScriptAnalyzer and Pester for the VM scripts |
| `audit` | ubuntu-latest | `cargo audit` (RustSec advisory database) |

Build caching uses `Swatinem/rust-cache`.

`.github/workflows/fuzz.yml`:
- Nightly toolchain, `cargo fuzz run decode_event`.
- **10 min** on a nightly schedule and on manual dispatch. This closes 0a DoD §8.2.2.
- **2 min** on PRs touching `crates/atlas-schema/**` or `crates/atlas-proto/**`.
- The corpus is kept between runs with `actions/cache`.
- On a crash, the job fails and the crashing input is uploaded as an artifact.

Supply chain:
- Actions are pinned to major-version tags.
- Dependabot opens weekly PRs for `cargo` and `github-actions`.
- There are no secrets in CI, so fork PRs are safe.

Branch protection on `main` (requiring the CI jobs) is **the user's action**. The runbook gives the `gh api` command. Claude does not change repo settings.

## Section 2: Test VM "edr-test" (approved)

All scripts live in `infra/vm/`. Host scripts run elevated and support `-WhatIf`.

**Host scripts:**
- **`New-EdrTestVm.ps1 -IsoPath <eval ISO>`**:
  - Gen 2, 4 vCPU, 8 GB RAM, 80 GB dynamic VHDX, vTPM (key protector + `Enable-VMTPM`), eval ISO attached.
  - `CheckpointType Standard`, so checkpoints include memory and restore fast.
  - Enables the Guest Service Interface integration service, needed for `Copy-VMFile`.
  - Creates the internal switch `edr-internal`.
  - Host vEthernet static IP `192.168.77.1/24`.
  - Host firewall: allow inbound UDP 50000 on that interface only (KDNET).
  - Idempotent, and refuses to modify any VM not named `edr-test`.
- **`Complete-EdrTestVmInstall.ps1`**: after the manual Windows install, turns **Secure Boot off** (Windows refuses `testsigning` while Secure Boot is on) and ejects the ISO.
- **`Set-EdrTestNetwork.ps1 -Mode Isolated|Online`**: removes or adds a second NIC on the Default Switch (NAT). The internal NIC is always present.
- **`Copy-ToEdrTestVm.ps1 <paths>`**: wraps `Copy-VMFile` into guest `C:\atlas\`. Works while isolated.
- **`Reset-EdrTestVm.ps1 [-Checkpoint baseline]`**: restores a checkpoint (default `baseline`).

**Guest script: `Initialize-EdrTestGuest.ps1`** (copied in with `Copy-ToEdrTestVm`, run as admin once):
- Static IP `192.168.77.10/24` on the internal NIC.
- `bcdedit /set testsigning on`.
- KDNET: `bcdedit /dbgsettings net hostip:192.168.77.1 port:50000 key:<generated>` and `bcdedit /debug on`. The key is printed once for WinDbg.
- Memory Integrity (HVCI) off, because it can block or crash test-signed drivers under development.
- Defender:
  - Cloud-delivered protection off, and automatic sample submission off, so attack samples never leave the VM.
  - Real-time protection off, so it doesn't interfere with Atomic Red Team or the driver.
  - Tamper Protection blocks these changes, so the runbook's one manual step is turning Tamper Protection off in the Windows Security app first. The script detects it and stops with a clear message if it is still on.
- winget installs WinDbg and the Sysinternals suite. This needs Online mode; the script checks.
- Takes no checkpoint (a guest can't checkpoint itself). The final runbook step is `Checkpoint-VM -Name baseline` on the host.

**Runbook `docs/runbooks/edr-test-vm.md`:**
- One-time setup: download the eval ISO → `New-EdrTestVm` → manual Windows install → `Complete-EdrTestVmInstall` → disable Tamper Protection → `Set-EdrTestNetwork Online` → copy in and run `Initialize-EdrTestGuest` → `Set-EdrTestNetwork Isolated` → baseline checkpoint.
- The 90-day rebuild.
- The daily loop: restore baseline → copy build → test → restore.
- Connecting WinDbg over KDNET.
- The branch-protection `gh api` command.

The user accepted Defender real-time protection and Memory Integrity being **off in the VM** (lab-only; never on the host).

**To verify while writing the spec:**
1. Does Windows 11 setup require Secure Boot *enabled*, or only *capable*, during install in a Gen 2 VM? If it requires it enabled, keep the plan above (turn it off after install); otherwise it can be off from the start.
2. KDNET support details for the Hyper-V synthetic NIC in a Gen 2 VM (any `busparams` needed?).
3. Exact winget package IDs for WinDbg and the Sysinternals suite.
4. Whether `buf lint` STANDARD rules pass on the existing 0a protos unmodified. If not, list the rule exceptions in `buf.yaml`, because changing v1 protos for lint reasons is not allowed.

## Next steps (resume here)

1. **Present design Section 3**, covering:
   - repo layout (`.github/`, `infra/vm/`, `docs/runbooks/`, `buf.yaml`, `dependabot.yml`)
   - how the scripts are tested (Pester for pure logic plus `-WhatIf` paths; PSScriptAnalyzer; real runs are a manual acceptance checklist in the runbook)
   - 0b's definition of done: CI green on `main`; first nightly fuzz run clean for 10 min; the VM built from the runbook; the acceptance checklist passed (boots, `testsigning` on, WinDbg attaches over KDNET, isolated mode has no internet, online mode does, `Copy-VMFile` works, baseline restore works)

   Then get approval.
2. Run the verification items above, write the spec to `docs/specs/<date>-scaffolding-design.md`, self-review, commit, and have the user review it.
3. `superpowers:writing-plans`, then execution.
