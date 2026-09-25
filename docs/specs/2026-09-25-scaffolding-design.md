# Sub-project 0b — Scaffolding Design (CI + Test VM)

**Status:** Approved (2026-09-25); implementation plan: [2026-09-25-scaffolding-plan](../plans/2026-09-25-scaffolding-plan.md). Brainstorm handoff: [scaffolding-brainstorm-notes](2026-09-25-scaffolding-brainstorm-notes.md).
**Depends on:** 0a (event schema: the crates CI builds, the protos `buf` checks, the fuzz target).
**Depended on by:** sub-project 1 (runs and verifies the ETW sensor in the VM), sub-project 6 (driver work happens only in the VM), and every later sub-project (CI).

---

## 1. Purpose & Scope

Give the project two pieces of infrastructure every later sub-project relies on:

1. **CI** on GitHub Actions that keeps `main` building, linted, tested, audited and fuzzed.
2. **The Hyper-V test VM `edr-test`**, built from scripts plus a runbook, where the agent, attack simulations and (later) the kernel driver run. The driver never runs on the host.

**In scope**
- `.github/workflows/ci.yml`, `.github/workflows/fuzz.yml`, `.github/dependabot.yml`.
- `buf.yaml` for the 0a protos; `buf lint` and `buf breaking` in CI (carried from 0a spec §7).
- The first clean 10-minute `cargo fuzz` run of `decode_event` (carried from 0a DoD §8.2.2).
- PowerShell VM scripts in `infra/vm/`, their Pester tests and PSScriptAnalyzer config.
- Runbook `docs/runbooks/edr-test-vm.md`, including a manual acceptance checklist.

**Out of scope** (owned elsewhere)
- Docker Compose for ClickHouse and Postgres → sub-project 2 (decision B1).
- ETW verification items (`ProcessSequenceNumber` vs `PsGetProcessStartKey`, a stable `boot_time` source, the DNS-Client 3008 requester PID) → sub-project 1, run *in* this VM.
- Driver signing and packaging → sub-project 6.
- Turning on branch protection: the user's action. The runbook documents the command; Claude does not change repo settings.

## 2. Key Decisions (from brainstorm)

| # | Decision | Rationale |
|---|---|---|
| B1 | Compose moves out of 0b and into sub-project 2 | No consumer until sub-project 2; a Compose file now would guess at versions and settings sub-project 2 rewrites. |
| B2 | Repo is **public** (since 2026-09-24) | GitHub-hosted runners are free and unmetered for public repos, so nightly fuzzing and Windows jobs on every push cost nothing. |
| B3 | License **AGPL-3.0-only** (`ae85ad9`) | Open for lab and personal use; modified network deployments must share source; the sole copyright holder keeps the dual-licensing option for goal D. |
| B4 | VM built by **scripts plus a runbook** | Rebuilds are reproducible without the fragility of an autounattend answer file; the Windows install itself stays manual. |
| B5 | VM network **isolated by default, online on demand** | An internal switch is always attached; a NAT NIC is added only for updates and downloads. Attack simulations always run isolated. |
| B6 | Windows 11 **Enterprise Evaluation** ISO (90 days) | Free, no activation; the scripted rebuild every 90 days doubles as a test that the scripts still work. |
| B7 | In the VM only: Secure Boot, Memory Integrity (HVCI) and Defender real-time/cloud protection **off** | Required for test-signed drivers and unimpeded attack simulation. Never applied to the host. |

## 3. CI

### 3.1 `ci.yml` — every push and pull request

| Job | Runner | Steps |
|---|---|---|
| `rust-linux` | `ubuntu-latest` | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`; `cargo check --manifest-path crates/atlas-schema/fuzz/Cargo.toml` (the fuzz crate is its own workspace, so the main commands don't cover it) |
| `rust-windows` | `windows-latest` | `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` (MSVC toolchain) |
| `proto` | `ubuntu-latest` | `bufbuild/buf-action` with `input: crates/atlas-proto/proto`: `buf lint` on every run; `buf breaking` against the PR base branch on pull requests. PR comments are off (`pr_comment: false`). |
| `powershell` | `windows-latest` | PSScriptAnalyzer over `infra/vm/` with `infra/vm/PSScriptAnalyzerSettings.psd1`; Pester 5 over `infra/vm/tests/` |
| `audit` | `ubuntu-latest` | `cargo audit` (RustSec advisory database) |

- Rust toolchain: the workspace's `rust-version` (1.97) via `dtolnay/rust-toolchain`; build caching via `Swatinem/rust-cache`.
- Any warning from `clippy`, `fmt`, PSScriptAnalyzer or `buf lint` fails the job.

### 3.2 `fuzz.yml`

- Runner `ubuntu-latest`, nightly Rust toolchain, `cargo install cargo-fuzz` (cached).
- Target: `cargo fuzz run decode_event` in `crates/atlas-schema`.
- **Duration** (`-max_total_time`):
  - **600 s** on a nightly `schedule` and on `workflow_dispatch`. The first clean scheduled run closes 0a DoD §8.2.2.
  - **120 s** on PRs touching `crates/atlas-schema/**` or `crates/atlas-proto/**`.
- The corpus (`crates/atlas-schema/fuzz/corpus/`, gitignored) persists between runs via `actions/cache`, keyed so each run restores the latest corpus and saves an updated one.
- On a crash the job fails and `crates/atlas-schema/fuzz/artifacts/` is uploaded with `actions/upload-artifact`.

### 3.3 `buf.yaml`

Lives at `crates/atlas-proto/proto/buf.yaml` (buf config v2):
- `lint.use: [STANDARD]`. **Verified:** the eight existing 0a protos pass STANDARD unmodified (buf 1.73.0, 2026-09-25), so no rule exceptions are needed. v1 protos are never edited just to satisfy lint.
- `breaking.use: [FILE]`, the strictest category, which fits a wire contract persisted in agent disk buffers.

### 3.4 Supply chain

- Third-party actions are pinned to major-version tags.
- `dependabot.yml`: weekly updates for `cargo` (root workspace and `crates/atlas-schema/fuzz`) and `github-actions`.
- CI uses no secrets, so PRs from forks are safe to run.
- Every workflow declares `permissions: contents: read` at the top level and nothing more. Caching and artifact upload don't need extra token scopes.

## 4. Test VM `edr-test`

### 4.1 Layout and conventions

```
infra/vm/
  EdrTestVm.psm1                 # constants + shared helpers; all logic Pester can test
  New-EdrTestVm.ps1
  Complete-EdrTestVmInstall.ps1
  Set-EdrTestNetwork.ps1
  Copy-ToEdrTestVm.ps1
  Reset-EdrTestVm.ps1
  guest/Initialize-EdrTestGuest.ps1
  tests/*.Tests.ps1
  PSScriptAnalyzerSettings.psd1
```

- **Constants live only in `EdrTestVm.psm1`:** VM name `edr-test`; switch `edr-internal`; host IP `192.168.77.1/24`; guest IP `192.168.77.10/24`; KDNET port `50000`; guest drop folder `C:\atlas\`; checkpoint name `baseline`.
- Scripts are thin wrappers over module functions.
- Every state-changing function uses `SupportsShouldProcess`, so all host scripts accept `-WhatIf` and `-Confirm`.
- Host scripts need an elevated session and stop with a clear message when not elevated.
- Every host script refuses to act on any VM not named `edr-test` (one guard function in the module).
- **NIC identification:** both VM NICs have Hyper-V **device naming** turned on (`Set-VMNetworkAdapter -DeviceNaming On`) with adapter names `edr-internal` and `edr-online`. The guest script finds its NIC by the `Hyper-V Network Adapter Name` advanced property, not by interface index or alias, so it survives the KDNET adapter swap described in §4.4.

### 4.2 Host scripts

**`New-EdrTestVm.ps1 -IsoPath <eval ISO>`**
- Creates the VM: Gen 2, 4 vCPU, 8 GB static RAM, 80 GB dynamic VHDX, eval ISO attached as first boot device.
- Security: Secure Boot **on** with the `MicrosoftWindows` template, and a vTPM (`Set-VMKeyProtector -NewLocalKeyProtector`, `Enable-VMTPM`). Both are required for Windows 11 setup to pass its hardware check.
- `CheckpointType Standard`, so checkpoints include memory and restore fast.
- Enables the Guest Service Interface integration service (needed for `Copy-VMFile`).
- Creates internal switch `edr-internal` if missing; host vEthernet gets `192.168.77.1/24`.
- Host firewall rule: allow inbound UDP 50000 **only on the `edr-internal` interface** (KDNET).
- Attaches NIC `edr-internal` with device naming on.
- Idempotent: an existing switch, IP, firewall rule or VM is reused, not recreated. If the VM exists with different settings, the script reports the difference and stops rather than changing it.

**`Complete-EdrTestVmInstall.ps1`** — run after the manual Windows install, with the VM off:
- Turns Secure Boot **off**: `bcdedit` refuses to change `testsigning` and debug settings while Secure Boot is on.
- Ejects the ISO and sets the VHDX as the first boot device.

**`Set-EdrTestNetwork.ps1 -Mode Isolated|Online`**
- `Online`: adds NIC `edr-online` on the `Default Switch` (NAT), device naming on.
- `Isolated`: removes NIC `edr-online`.
- NIC `edr-internal` is never touched. Hot add and remove work on a running Gen 2 VM.

**`Copy-ToEdrTestVm.ps1 <paths>`** — wraps `Copy-VMFile -FileSource Host -CreateFullPath` into `C:\atlas\`. Goes over VMBus, so it works while isolated.

**`Reset-EdrTestVm.ps1 [-Checkpoint baseline]`** — restores the named checkpoint (default `baseline`) and starts the VM.

### 4.3 Guest script `Initialize-EdrTestGuest.ps1`

Copied in with `Copy-ToEdrTestVm`, run once as administrator, **in Online mode**. Steps, in order:

1. **Preconditions.** Stop with a clear message if any fails:
   - not running as administrator;
   - Secure Boot still on (`Confirm-SecureBootUEFI`), because `Complete-EdrTestVmInstall` hasn't run;
   - Defender Tamper Protection still on (`Get-MpComputerStatus`). The runbook's one manual step is turning it off in the Windows Security app;
   - no `edr-online` NIC, because winget needs internet.
2. **Static IP** `192.168.77.10/24` on the NIC named `edr-internal`, with no gateway, so the internal NIC never becomes a default route.
3. **Test signing:** `bcdedit /set testsigning on`.
4. **KDNET:**
   - `bcdedit /dbgsettings net hostip:192.168.77.1 port:50000` (bcdedit generates and prints the key);
   - `bcdedit /debug on`.
   - The script prints the key and the matching `windbg -k net:port=50000,key=<key>` line once.
5. **Memory Integrity (HVCI) off** via the `DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity` registry value `Enabled = 0`.
6. **Defender:** cloud-delivered protection off, automatic sample submission set to never send, real-time protection off (`Set-MpPreference`).
7. **Tools:** `winget install --exact --id Microsoft.WinDbg` and `--id Microsoft.Sysinternals.Suite`, with `--accept-source-agreements --accept-package-agreements`. Package IDs verified on 2026-09-25.
8. Prints what the runbook says to do next: reboot, `Set-EdrTestNetwork Isolated`, then take the checkpoint.

A guest cannot checkpoint itself, so the final runbook step is `Checkpoint-VM -Name baseline` on the host.

### 4.4 KDNET on Hyper-V (verified)

- In a Hyper-V VM, KDNET detects virtualization and talks over **VMBus to the host**, which exposes the debug endpoint. After reboot, the guest shows a "Microsoft Kernel Debug Network Adapter" in place of the synthetic NIC, which is why §4.1 finds NICs by device name.
- **`busparams` is not used.** Gen 2 synthetic NICs are VMBus devices, not PCI, so there is no bus/device/function to give.
- Microsoft's guide uses an external switch. This design uses the internal switch instead, because the debugger runs on the host and only needs the host at `192.168.77.1` to be reachable.
- **Known risk:** KDNET binds to one NIC at boot, and with no `busparams` we can't choose which. The `baseline` checkpoint is taken in Isolated mode, so after a restore or reboot there is only `edr-internal` and the binding is unambiguous. The acceptance checklist (§5.3) also tests a reboot in Online mode. If KDNET binds to `edr-online` there, the runbook states the rule "kernel-debug only in Isolated mode" and no design change is needed.

### 4.5 Runbook `docs/runbooks/edr-test-vm.md`

- **One-time setup:** download the eval ISO → `New-EdrTestVm` → manual Windows install (local account) → `Complete-EdrTestVmInstall` → turn off Tamper Protection in Windows Security → `Set-EdrTestNetwork Online` → `Copy-ToEdrTestVm` the guest script, run it → reboot → `Set-EdrTestNetwork Isolated` → `Checkpoint-VM -Name baseline`.
- **90-day rebuild:** remove the VM and its VHDX, then repeat one-time setup. The switch and firewall rule are reused.
- **Daily loop:** `Reset-EdrTestVm` → `Copy-ToEdrTestVm` the build → test → `Reset-EdrTestVm`.
- **Kernel debugging:** start WinDbg on the host with the saved `-k net:` line, then reboot the guest.
- **Branch protection:** the `gh api` command to require the §3.1 jobs on `main` (the user runs it).
- **Acceptance checklist** (§5.3).

## 5. Testing & Definition of Done

### 5.1 Pester (runs in CI, `windows-latest`)

- All Hyper-V, NetTCPIP, NetSecurity, Defender and `bcdedit` calls are mocked.
- The `windows-latest` runner has no Hyper-V module, and Pester can only mock commands that exist. The tests therefore define stub functions for any missing command before mocking it.
- Covered:
  - the VM-name guard refuses anything but `edr-test`;
  - idempotency: an existing switch, IP, rule or VM is reused, and a mismatched VM stops the script;
  - `-WhatIf` performs no state-changing call;
  - `Set-EdrTestNetwork` adds and removes only `edr-online`;
  - each guest precondition failure stops with its message and makes no changes;
  - NIC lookup by device name;
  - parsing the KDNET key from `bcdedit` output.

### 5.2 PSScriptAnalyzer

Default rule set plus `PSUseShouldProcessForStateChangingFunctions`, `PSUseCompatibleSyntax` (PowerShell 7 and Windows PowerShell 5.1: the guest script must run on a fresh Windows install, which only has 5.1). Warnings fail CI.

### 5.3 Manual acceptance checklist (in the runbook)

1. The VM boots to the desktop.
2. `bcdedit` in the guest shows `testsigning Yes` and `debug Yes`.
3. WinDbg on the host attaches over KDNET after a guest reboot and breaks in.
4. Isolated mode: pinging `1.1.1.1` from the guest fails; pinging `192.168.77.1` succeeds.
5. Online mode: the guest reaches the internet.
6. Online mode, after a guest reboot: note whether KDNET still attaches (§4.4). Either result passes; the runbook records it.
7. `Copy-ToEdrTestVm` delivers a file while isolated.
8. `Reset-EdrTestVm` restores `baseline`: a file created after the checkpoint is gone.

### 5.4 Definition of done

1. All five `ci.yml` jobs are green on `main`, and Dependabot is enabled.
2. The first scheduled nightly fuzz run finishes 600 s clean. The roadmap drops "fuzz run pending" from 0a.
3. The VM is built by following the runbook from a fresh eval ISO. Any step that differs from the runbook is fixed in the runbook.
4. The §5.3 acceptance checklist passes.
5. The decision log and roadmap are updated.

## 6. Verification Notes (2026-09-25)

| Item | Result | Effect on design |
|---|---|---|
| Does Windows 11 setup need Secure Boot *enabled* in a Gen 2 VM? | Yes. Setup's hardware check fails ("This PC can't run Windows 11") when Secure Boot or the vTPM is off. | Keep Secure Boot on for the install; `Complete-EdrTestVmInstall` turns it off afterwards. |
| KDNET on a Hyper-V synthetic NIC | Supported; KDNET talks over VMBus to the host. `busparams` doesn't apply to VMBus NICs. Secure Boot must be off to change debug settings. | §4.4; NICs found by device name; known risk with two NICs. |
| winget IDs | `Microsoft.WinDbg`, `Microsoft.Sysinternals.Suite` | §4.3 step 7. |
| `buf lint` STANDARD on 0a protos | Passes unmodified (8 files, buf 1.73.0). | No rule exceptions in `buf.yaml`. |

Sources: [Microsoft Learn — KDNET for a Hyper-V VM](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/setting-up-network-debugging-of-a-virtual-machine-host); [Microsoft Learn — KDNET manual setup](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/setting-up-a-network-debugging-connection); [TechTarget — Windows 11 on Hyper-V](https://techtarget.com/searchvirtualdesktop/tip/What-to-do-when-a-PC-cant-run-Windows-11-on-Hyper-V); [Rafael Rivera — synthetic kernel debugging for Hyper-V](https://withinrafael.com/2015/02/01/how-to-set-up-synthetic-kernel-debugging-for-hyper-v-virtual-machines/).

## 7. Clarifications made during planning (2026-09-25)

- The guest imports `EdrTestVm.psm1`, so `Copy-ToEdrTestVm` copies both the module and `guest/Initialize-EdrTestGuest.ps1` into `C:\atlas\`.
- Additional guest precondition: `winget.exe` must be present (it can be missing on a fresh install until App Installer updates). All unmet preconditions are reported at once.
- `New-EdrTestVm` refuses a leftover `edr-test.vhdx` when the VM doesn't exist, and disables automatic checkpoints.
- `Copy-ToEdrTestVm` copies files only, validates every path before copying any, and requires the VM to be Running.
- `Complete-EdrTestVmInstall` requires the VM to be Off and is safe to re-run.
- The guest's internal NIC gets DHCP disabled and a static address with no gateway.
- The VM-name guard is case-insensitive, matching Hyper-V.
- Rust toolchains are installed with `rustup` directly: `dtolnay/rust-toolchain` has no major-version tags (§3.4).
- `cargo audit` also checks `crates/atlas-schema/fuzz/Cargo.lock`.
- Pinned tool versions: Pester 5.9.1, PSScriptAnalyzer 1.25.0; actions `checkout@v7`, `cache@v6`, `upload-artifact@v7`, `rust-cache@v2`, `install-action@v2`, `buf-action@v1`.
