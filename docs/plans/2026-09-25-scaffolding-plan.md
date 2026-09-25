# Sub-project 0b — Scaffolding (CI + Test VM) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add GitHub Actions CI (Rust on Linux and Windows, `buf`, PowerShell, `cargo audit`), nightly fuzzing of `decode_event`, and scripts plus a runbook that build and operate the Hyper-V test VM `edr-test`.

**Architecture:**
- **CI.** Two workflow files. `ci.yml` has five independent jobs and runs on every push and PR. `fuzz.yml` runs nightly, on manual dispatch, and on schema/proto PRs.
- **VM tooling.** All logic lives in one PowerShell module, `infra/vm/EdrTestVm.psm1`: constants, guards, host commands and guest setup.
  - The six scripts are 3-line wrappers that import the module and splat their arguments to the matching function.
  - Every external command (Hyper-V, networking, Defender, `bcdedit`, `winget`) is called through a name that Pester can mock.
  - The tests replace every such command with a stub that throws "Unmocked call", so no test can touch the real host.

**Tech Stack:**
- GitHub Actions: `actions/checkout@v7`, `actions/cache@v6`, `actions/upload-artifact@v7`, `Swatinem/rust-cache@v2`, `taiki-e/install-action@v2`, `bufbuild/buf-action@v1`.
- buf 1.73.0 (config v2); cargo-fuzz (nightly); cargo-audit.
- PowerShell 7 (host) and Windows PowerShell 5.1 (guest); Pester 5.9.1; PSScriptAnalyzer 1.25.0; Hyper-V PowerShell module.

**Spec:** `docs/specs/2026-09-25-scaffolding-design.md`. Read it before starting. Section numbers below (§) refer to it.

**Verification note:** Everything in this plan was run in a scratch copy on 2026-09-25:
- **PowerShell:** the module, the scripts and all tests were rebuilt cumulatively, task by task. Every stage passes Pester with zero PSScriptAnalyzer findings: 13 tests after Task 3, 21 after Task 4, 61 after Task 5, 77 after Task 6. The final suite also passes under Windows PowerShell 5.1.
- **Workflows:** `actionlint` 1.7.12 is clean on both workflows. `buf lint` and `buf breaking` (against `main`) pass with the new `buf.yaml`.
- **Rust:** `cargo fmt --check`, `clippy -D warnings` and `cargo test --workspace` pass on current `main`.
- **Not run locally** (first verified in CI): `cargo audit`, and `cargo fuzz` (needs Linux nightly).

## Global Constraints

- Work on branch `feat-0b-scaffolding` (a worktree per superpowers:using-git-worktrees). Never commit to `main` directly.
- **Never run the host or guest scripts for real, not even with `-WhatIf`.** They need elevation and touch Hyper-V, networking and the boot configuration. The VM build (Task 8) is done by the user following the runbook. Driver code is only ever loaded in the VM, never on the host (CLAUDE.md).
- Every file under `infra/vm/` is **ASCII-only**. Windows PowerShell 5.1 reads BOM-less files as ANSI, and a test enforces this.
- `EdrTestVm.psm1` must parse under **Windows PowerShell 5.1**: no `??`, `?:`, `&&`, `||`, or `-Parallel`. `PSUseCompatibleSyntax` and a test enforce this.
- Constants live only in `$script:Config` in `EdrTestVm.psm1`: `edr-test`, `edr-internal`, `edr-online`, `Default Switch`, `192.168.77.1`, `192.168.77.10`, `/24`, `50000`, `Atlas-EdrTest-KDNET`, `C:\atlas`, `baseline`, 4 vCPU, 8 GB, 80 GB, `Microsoft.WinDbg`, `Microsoft.Sysinternals.Suite`.
- Every state-changing function uses `[CmdletBinding(SupportsShouldProcess)]` and wraps each change in `$PSCmdlet.ShouldProcess(...)`. Helpers that don't do this must use a verb outside PSScriptAnalyzer's state-changing list (e.g. `Write-`, `Invoke-`), and their caller gates them.
- Tool versions: Pester **5.9.1**, PSScriptAnalyzer **1.25.0**, buf **1.73.0**, actionlint **1.7.12**, Rust **1.97** (stable jobs), nightly for fuzzing only.
- **One-time local tool install** (Task 3 step 1 does it):
  ```powershell
  Install-Module Pester -RequiredVersion 5.9.1 -Scope CurrentUser -SkipPublisherCheck -Force
  Install-Module PSScriptAnalyzer -RequiredVersion 1.25.0 -Scope CurrentUser -Force
  ```
- **Run the VM tests** always in a *fresh* `pwsh` process, because the stubs shadow real cmdlets for the life of the session:
  ```powershell
  pwsh -NoProfile -Command "Import-Module Pester -RequiredVersion 5.9.1; Invoke-Pester infra/vm/tests -Output Detailed"
  ```
- **Run PSScriptAnalyzer:** it must print nothing.
  ```powershell
  pwsh -NoProfile -Command "Invoke-ScriptAnalyzer -Path infra/vm -Recurse -Settings infra/vm/PSScriptAnalyzerSettings.psd1"
  ```
- Every commit message ends with:
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  ```

## Review Focus

The spec implies these five conditions, but no happy-path test would exercise them. Each has a pinned test in the task that owns the code:

1. **The 90-day rebuild leaves `edr-test.vhdx` behind** after the VM is removed. `New-EdrTestVm` must refuse with a clear message rather than fail inside `New-VM` or reuse a stale disk. Pinned in Task 4: `refuses to create the VM over a leftover VHDX from a previous build`.
2. **Re-running the guest script after a reboot** (a normal thing to do when something failed halfway) must not rotate the KDNET key the user already saved. Pinned in Task 6: `keeps an existing matching KDNET configuration so the key does not change`.
3. **One mistyped or folder path among several** passed to `Copy-ToEdrTestVm` must copy nothing, not a partial set. Pinned in Task 5: `copies nothing when any path is a folder or missing`.
4. **A test that forgets to mock a Hyper-V call** must fail loudly, not reconfigure the developer's real host. Pinned in Task 3: `make an unmocked Hyper-V call fail loudly instead of reaching the real host`.
5. **The guest runs Windows PowerShell 5.1**, where a stray UTF-8 character or PowerShell-7-only syntax breaks parsing. Pinned in Task 3: `<File> is ASCII-only` and `the module imports under powershell.exe`.

## Deliberate clarifications of the spec (written into the spec in Task 7)

- **Copying the guest script:** the guest imports the module, so `Copy-ToEdrTestVm` copies **both** `EdrTestVm.psm1` and `guest/Initialize-EdrTestGuest.ps1` into `C:\atlas\`. The guest script finds the module next to itself (in the VM) or one folder up (in the repo).
- **Extra guest precondition:** `winget.exe` must be present. On a fresh install it can be missing until App Installer updates. The check reports every unmet precondition at once and changes nothing.
- **`New-EdrTestVm` also:**
  - refuses a leftover `edr-test.vhdx` when the VM doesn't exist;
  - disables automatic checkpoints, so Hyper-V doesn't create one on every start and clutter the baseline.
- **`Copy-ToEdrTestVm`:**
  - copies files only;
  - validates every path before copying any;
  - requires the VM to be Running, which `Copy-VMFile` needs anyway.
- **`Complete-EdrTestVmInstall`** requires the VM to be Off and is safe to re-run.
- **Guest static IP:** DHCP is turned off on the internal NIC, and the address has no gateway.
- **VM-name guard:** case-insensitive, like Hyper-V's own VM names.
- **Rust toolchains** are installed with `rustup` directly. `dtolnay/rust-toolchain` publishes no major-version tags, which conflicts with §3.4's pinning rule.
- **`cargo audit`** also checks the fuzz crate's separate `Cargo.lock`.

---

### Task 1: CI workflow (Rust, proto, audit), `buf.yaml`, Dependabot

**Files:**
- Create: `.github/workflows/ci.yml`
- Create: `.github/dependabot.yml`
- Create: `crates/atlas-proto/proto/buf.yaml`

**Interfaces:**
- Consumes: the existing workspace (`cargo` commands) and `crates/atlas-schema/fuzz/Cargo.toml`.
- Produces: CI job ids `rust-linux`, `rust-windows`, `proto`, `audit`. These are the status-check contexts named in the runbook's branch-protection command (Task 7). Task 5 adds a fifth job, `powershell`.

- [ ] **Step 1: Write `crates/atlas-proto/proto/buf.yaml`**

```yaml
# buf config for the atlas.events.v1 wire contract. Spec: docs/specs/2026-09-25-scaffolding-design.md section 3.3.
# The 0a protos pass STANDARD unmodified; never edit a v1 proto just to satisfy lint.
version: v2
lint:
  use:
    - STANDARD
breaking:
  # Strictest category: these messages are also persisted in agent disk buffers.
  use:
    - FILE
```

- [ ] **Step 2: Check lint and breaking locally**

Run:
```powershell
npx -y @bufbuild/buf@1.73.0 lint crates/atlas-proto/proto
npx -y @bufbuild/buf@1.73.0 breaking crates/atlas-proto/proto --against '.git#branch=main,subdir=crates/atlas-proto/proto'
```
Expected: no output from either command, exit code 0. If `lint` reports anything, **stop**: the spec says the 0a protos pass STANDARD unmodified, and v1 protos must not be edited for lint.

- [ ] **Step 3: Write `.github/workflows/ci.yml`**

```yaml
# CI for every push and pull request. Spec: docs/specs/2026-09-25-scaffolding-design.md section 3.1.
name: ci

on:
  push:
  pull_request:

permissions:
  contents: read

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: ${{ github.ref != 'refs/heads/main' }}

env:
  CARGO_TERM_COLOR: always
  RUST_TOOLCHAIN: "1.97"

jobs:
  rust-linux:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust ${{ env.RUST_TOOLCHAIN }}
        run: |
          rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal --component rustfmt,clippy
          rustup default "$RUST_TOOLCHAIN"
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: |
            . -> target
            crates/atlas-schema/fuzz -> target
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
      # The fuzz crate is its own workspace, so the commands above don't build it.
      - run: cargo check --manifest-path crates/atlas-schema/fuzz/Cargo.toml

  rust-windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust ${{ env.RUST_TOOLCHAIN }}
        shell: bash
        run: |
          rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal --component clippy
          rustup default "$RUST_TOOLCHAIN"
      - uses: Swatinem/rust-cache@v2
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace

  proto:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      # Lint on every run; breaking-change check against the PR base on pull requests (the action's default).
      - uses: bufbuild/buf-action@v1
        with:
          input: crates/atlas-proto/proto
          lint: true
          format: false
          push: false
          archive: false
          pr_comment: false

  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-audit
      - run: cargo audit
      - run: cargo audit --file crates/atlas-schema/fuzz/Cargo.lock
```

- [ ] **Step 4: Write `.github/dependabot.yml`**

```yaml
# Spec: docs/specs/2026-09-25-scaffolding-design.md section 3.4.
version: 2
updates:
  - package-ecosystem: cargo
    directories:
      - "/"
      - "/crates/atlas-schema/fuzz"
    schedule:
      interval: weekly
  - package-ecosystem: github-actions
    directory: "/"
    schedule:
      interval: weekly
```

- [ ] **Step 5: Lint the workflow with actionlint**

Run:
```powershell
gh release download v1.7.12 --repo rhysd/actionlint -p 'actionlint_1.7.12_windows_amd64.zip' -D "$env:TEMP\actionlint" --clobber
Expand-Archive "$env:TEMP\actionlint\actionlint_1.7.12_windows_amd64.zip" -DestinationPath "$env:TEMP\actionlint" -Force
& "$env:TEMP\actionlint\actionlint.exe" -no-color .github/workflows/ci.yml
```
Expected: no output, exit code 0.

- [ ] **Step 6: Run the Rust job's commands locally**

Run:
```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --manifest-path crates/atlas-schema/fuzz/Cargo.toml
```
Expected: all succeed (verified on `main` at plan time for the first three).

- [ ] **Step 7: Commit and push; watch CI**

```powershell
git add .github/workflows/ci.yml .github/dependabot.yml crates/atlas-proto/proto/buf.yaml
git commit -m "ci: add Rust, buf and cargo-audit jobs; Dependabot

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -u origin feat-0b-scaffolding
Start-Sleep 15  # let GitHub register the run
gh run watch (gh run list --branch feat-0b-scaffolding --workflow ci --limit 1 --json databaseId --jq '.[0].databaseId') --exit-status
```
Expected: `rust-linux`, `rust-windows`, `proto` and `audit` all succeed.
- If `audit` fails on an advisory, **stop and report it to the user** with the advisory id and affected crate. Do not add ignores on your own.
- For any other failure, fix the cause, not the check.

---

### Task 2: Fuzz workflow

**Files:**
- Create: `.github/workflows/fuzz.yml`

**Interfaces:**
- Consumes: `crates/atlas-schema/fuzz` (target `decode_event`); `.gitignore` already ignores `**/fuzz/corpus` and `**/fuzz/artifacts`.
- Produces: workflow `fuzz`, job `decode_event`. Task 8 dispatches it on `main`.

- [ ] **Step 1: Write `.github/workflows/fuzz.yml`**

```yaml
# Fuzzes atlas_schema::decode_event. Spec: docs/specs/2026-09-25-scaffolding-design.md section 3.2.
name: fuzz

on:
  schedule:
    - cron: "17 3 * * *" # nightly, 03:17 UTC
  workflow_dispatch:
  pull_request:
    paths:
      - "crates/atlas-schema/**"
      - "crates/atlas-proto/**"

permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always

jobs:
  decode_event:
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v7
      - name: Install nightly Rust
        run: |
          rustup toolchain install nightly --profile minimal
          rustup default nightly
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-fuzz
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: crates/atlas-schema/fuzz -> target
      # Each run saves a new corpus entry; restore-keys picks up the most recent one.
      - name: Restore corpus
        uses: actions/cache/restore@v6
        with:
          path: crates/atlas-schema/fuzz/corpus
          key: fuzz-corpus-decode_event-${{ github.run_id }}
          restore-keys: fuzz-corpus-decode_event-
      - name: Fuzz
        working-directory: crates/atlas-schema
        env:
          # 10 minutes nightly and on manual runs (closes 0a DoD 8.2.2); 2 minutes on pull requests.
          SECONDS_TO_RUN: ${{ github.event_name == 'pull_request' && '120' || '600' }}
        run: cargo fuzz run decode_event -- -max_total_time="$SECONDS_TO_RUN"
      - name: Save corpus
        if: always()
        uses: actions/cache/save@v6
        with:
          path: crates/atlas-schema/fuzz/corpus
          key: fuzz-corpus-decode_event-${{ github.run_id }}
      - name: Upload crashing inputs
        if: failure()
        uses: actions/upload-artifact@v7
        with:
          name: fuzz-artifacts-decode_event
          path: crates/atlas-schema/fuzz/artifacts/
          if-no-files-found: ignore
```

- [ ] **Step 2: Lint it**

Run: `& "$env:TEMP\actionlint\actionlint.exe" -no-color .github/workflows/fuzz.yml`
Expected: no output, exit code 0.

- [ ] **Step 3: Commit and push**

```powershell
git add .github/workflows/fuzz.yml
git commit -m "ci: fuzz decode_event nightly (10 min) and on schema PRs (2 min)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push
```
Expected: `ci` runs again and stays green.
- `fuzz` does **not** run yet: this branch's push doesn't match its triggers.
- Its first real run is the PR in Task 8 only if schema files change. Otherwise the first run is the `workflow_dispatch` in Task 8, after merge.

---

### Task 3: VM module core, test stubs, analyzer settings

**Files:**
- Create: `infra/vm/EdrTestVm.psm1`
- Create: `infra/vm/PSScriptAnalyzerSettings.psd1`
- Create: `infra/vm/tests/TestStubs.ps1`
- Test: `infra/vm/tests/Common.Tests.ps1`

**Interfaces:**
- Produces (module-internal, used by Tasks 4–6):
  - `$script:Config` (pscustomobject; the properties are listed in Global Constraints)
  - `Test-EdrElevated` → `[bool]`
  - `Assert-EdrElevated`, which throws `'...elevated...'`
  - `Assert-EdrTestVmName -Name <string>`, which throws `"Refusing to act on VM '<name>'..."`
  - `Get-EdrTestVmRequired` → VM object. It throws `"...does not exist. Run New-EdrTestVm.ps1 first."`
- Produces (exported): `Get-EdrTestVmConfig`.
- Produces (tests):
  - `Register-EdrTestStub` / `Unregister-EdrTestStub`
  - `Get-FakeVm [-Override <hashtable>]`, which returns a VM object matching the spec with any overrides applied.

- [ ] **Step 1: Install the PowerShell test tools (one-time)**

```powershell
Install-Module Pester -RequiredVersion 5.9.1 -Scope CurrentUser -SkipPublisherCheck -Force
Install-Module PSScriptAnalyzer -RequiredVersion 1.25.0 -Scope CurrentUser -Force
```

- [ ] **Step 2: Write the test stubs `infra/vm/tests/TestStubs.ps1`**

It lists **every** external command the finished module calls, so later tasks only mock and never edit this file.

```powershell
# Stand-ins for every external command EdrTestVm.psm1 calls.
#
# Pester can only mock a command that exists, and CI runners have no Hyper-V module. These stubs are also defined on
# machines that DO have Hyper-V: a function outranks a cmdlet of the same name, so a test that forgets a Mock hits
# the stub and fails with "Unmocked call" instead of touching the real host.
#
# Each entry lists the parameters the module passes. Prefix switches with '[switch]'.

$script:EdrTestStubs = [ordered]@{
    # Hyper-V
    'Get-VM'                      = @('Name')
    'Get-VMHost'                  = @()
    'Get-VMSwitch'                = @('Name')
    'New-VMSwitch'                = @('Name', 'SwitchType')
    'New-VM'                      = @('Name', 'Generation', 'MemoryStartupBytes', 'NewVHDPath', 'NewVHDSizeBytes', 'SwitchName')
    'Set-VM'                      = @('Name', 'ProcessorCount', '[switch]StaticMemory', 'CheckpointType', 'AutomaticCheckpointsEnabled')
    'Set-VMFirmware'              = @('VMName', 'EnableSecureBoot', 'SecureBootTemplate', 'FirstBootDevice')
    'Set-VMKeyProtector'          = @('VMName', '[switch]NewLocalKeyProtector')
    'Enable-VMTPM'                = @('VMName')
    'Enable-VMIntegrationService' = @('VMName', 'Name')
    'Get-VMNetworkAdapter'        = @('VMName', 'Name')
    'Add-VMNetworkAdapter'        = @('VMName', 'Name', 'SwitchName', 'DeviceNaming')
    'Remove-VMNetworkAdapter'     = @('VMName', 'Name')
    'Rename-VMNetworkAdapter'     = @('VMName', 'Name', 'NewName')
    'Set-VMNetworkAdapter'        = @('VMName', 'Name', 'DeviceNaming')
    'Add-VMDvdDrive'              = @('VMName', 'Path')
    'Get-VMDvdDrive'              = @('VMName')
    'Remove-VMDvdDrive'           = @('VMDvdDrive')
    'Get-VMHardDiskDrive'         = @('VMName')
    'Copy-VMFile'                 = @('Name', 'SourcePath', 'DestinationPath', 'FileSource', '[switch]CreateFullPath', '[switch]Force')
    'Get-VMCheckpoint'            = @('VMName', 'Name')
    'Restore-VMCheckpoint'        = @('VMName', 'Name')
    'Start-VM'                    = @('Name')
    # Networking and firewall (host and guest)
    'Get-NetIPAddress'            = @('InterfaceAlias', 'AddressFamily', 'IPAddress')
    'New-NetIPAddress'            = @('InterfaceAlias', 'IPAddress', 'PrefixLength')
    'Set-NetIPInterface'          = @('InterfaceAlias', 'AddressFamily', 'Dhcp')
    'Get-NetFirewallRule'         = @('Name')
    'New-NetFirewallRule'         = @('Name', 'DisplayName', 'Direction', 'Protocol', 'LocalPort', 'InterfaceAlias', 'Action', 'Profile')
    'Get-NetAdapterAdvancedProperty' = @('DisplayName')
    # Guest security
    'Confirm-SecureBootUEFI'      = @()
    'Get-MpComputerStatus'        = @()
    'Set-MpPreference'            = @('MAPSReporting', 'SubmitSamplesConsent', 'DisableRealtimeMonitoring')
}

function Register-EdrTestStub {
    foreach ($name in $script:EdrTestStubs.Keys) {
        $params = foreach ($p in $script:EdrTestStubs[$name]) {
            if ($p -like '`[switch`]*') { '[switch]$' + $p.Substring(8) } else { '$' + $p }
        }
        $body = "[CmdletBinding(SupportsShouldProcess)] param($($params -join ', ')) throw 'Unmocked call to $name'"
        Set-Item -Path "function:global:$name" -Value ([scriptblock]::Create($body))
    }
}

function Unregister-EdrTestStub {
    foreach ($name in $script:EdrTestStubs.Keys) {
        Remove-Item -Path "function:global:$name" -ErrorAction SilentlyContinue
    }
}

# A VM object shaped like Get-VM's output, matching the spec unless overridden.
function Get-FakeVm([hashtable]$Override = @{}) {
    $vm = @{
        Name = 'edr-test'; State = 'Off'; Generation = 2; ProcessorCount = 4; MemoryStartup = 8GB
        DynamicMemoryEnabled = $false; CheckpointType = 'Standard'
    }
    foreach ($k in $Override.Keys) { $vm[$k] = $Override[$k] }
    [pscustomobject]$vm
}
```

- [ ] **Step 3: Write the failing tests `infra/vm/tests/Common.Tests.ps1`**

```powershell
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseDeclaredVarsMoreThanAssignments', '', Justification = 'Pester shares BeforeEach variables with It blocks.')]
param()

BeforeAll {
    . (Join-Path $PSScriptRoot 'TestStubs.ps1')
    Register-EdrTestStub
    Import-Module (Join-Path $PSScriptRoot '..\EdrTestVm.psm1') -Force
}

AfterAll {
    Remove-Module EdrTestVm -ErrorAction SilentlyContinue
    Unregister-EdrTestStub
}

Describe 'Get-EdrTestVmConfig' {
    It 'holds the values from spec section 4.1' {
        $c = Get-EdrTestVmConfig
        $c.VmName | Should -Be 'edr-test'
        $c.InternalSwitch | Should -Be 'edr-internal'
        $c.InternalAdapter | Should -Be 'edr-internal'
        $c.OnlineAdapter | Should -Be 'edr-online'
        $c.OnlineSwitch | Should -Be 'Default Switch'
        $c.HostIp | Should -Be '192.168.77.1'
        $c.GuestIp | Should -Be '192.168.77.10'
        $c.PrefixLength | Should -Be 24
        $c.KdnetPort | Should -Be 50000
        $c.GuestDropPath | Should -Be 'C:\atlas'
        $c.BaselineCheckpoint | Should -Be 'baseline'
        $c.ProcessorCount | Should -Be 4
        $c.MemoryBytes | Should -Be 8GB
        $c.VhdSizeBytes | Should -Be 80GB
        $c.WingetPackages | Should -Be @('Microsoft.WinDbg', 'Microsoft.Sysinternals.Suite')
    }
}

Describe 'Assert-EdrTestVmName' {
    It 'accepts edr-test' {
        InModuleScope EdrTestVm { { Assert-EdrTestVmName -Name 'edr-test' } | Should -Not -Throw }
    }
    It 'refuses <Name>' -TestCases @(
        @{ Name = 'prod-dc' }
        @{ Name = 'edr-test-2' }
        @{ Name = '' }
    ) {
        InModuleScope EdrTestVm -Parameters @{ Name = $Name } {
            { Assert-EdrTestVmName -Name $Name } | Should -Throw '*Refusing*'
        }
    }
}

Describe 'Assert-EdrElevated' {
    It 'throws when not elevated' {
        Mock -ModuleName EdrTestVm Test-EdrElevated { $false }
        InModuleScope EdrTestVm { { Assert-EdrElevated } | Should -Throw '*elevated*' }
    }
    It 'passes when elevated' {
        Mock -ModuleName EdrTestVm Test-EdrElevated { $true }
        InModuleScope EdrTestVm { { Assert-EdrElevated } | Should -Not -Throw }
    }
}

Describe 'Test stubs' {
    It 'make an unmocked Hyper-V call fail loudly instead of reaching the real host' {
        InModuleScope EdrTestVm { { Get-VM -Name 'edr-test' } | Should -Throw 'Unmocked call to Get-VM' }
    }
}

Describe 'Windows PowerShell 5.1 compatibility' {
    # The guest runs the module under Windows PowerShell 5.1, which reads BOM-less files as ANSI.
    It '<File> is ASCII-only' -TestCases @(
        Get-ChildItem -Path (Join-Path $PSScriptRoot '..') -Recurse -File -Include '*.ps1', '*.psm1', '*.psd1' |
            ForEach-Object { @{ File = $_.Name; FullName = $_.FullName } }
    ) {
        $bytes = [IO.File]::ReadAllBytes($FullName)
        @($bytes | Where-Object { $_ -gt 0x7F }).Count | Should -Be 0
    }

    It 'the module imports under powershell.exe' -Skip:(-not (Get-Command powershell.exe -ErrorAction SilentlyContinue)) {
        $module = Join-Path $PSScriptRoot '..\EdrTestVm.psm1'
        $out = & powershell.exe -NoProfile -NonInteractive -Command "Import-Module '$module'; (Get-EdrTestVmConfig).VmName"
        $LASTEXITCODE | Should -Be 0
        $out | Should -Be 'edr-test'
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run the VM tests (Global Constraints).
Expected: FAIL. `Import-Module` can't find `EdrTestVm.psm1`, so every test in `Common.Tests.ps1` errors.

- [ ] **Step 5: Write `infra/vm/EdrTestVm.psm1`**

```powershell
# EdrTestVm: build and operate the Hyper-V test VM "edr-test".
# Spec: docs/specs/2026-09-25-scaffolding-design.md section 4.
#
# Must stay parseable by Windows PowerShell 5.1: the guest script imports this module on a fresh Windows install.
# Every external command is called through a name the Pester tests can mock (see tests/TestStubs.ps1).

Set-StrictMode -Version 3.0

$script:Config = [pscustomobject]@{
    VmName             = 'edr-test'
    InternalSwitch     = 'edr-internal'
    InternalAdapter    = 'edr-internal'
    OnlineSwitch       = 'Default Switch'
    OnlineAdapter      = 'edr-online'
    HostIp             = '192.168.77.1'
    GuestIp            = '192.168.77.10'
    PrefixLength       = 24
    KdnetPort          = 50000
    FirewallRuleName   = 'Atlas-EdrTest-KDNET'
    GuestDropPath      = 'C:\atlas'
    BaselineCheckpoint = 'baseline'
    ProcessorCount     = 4
    MemoryBytes        = 8GB
    VhdSizeBytes       = 80GB
    WingetPackages     = @('Microsoft.WinDbg', 'Microsoft.Sysinternals.Suite')
}

function Get-EdrTestVmConfig {
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param()
    $script:Config
}

# ---------------------------------------------------------------------------------------------------------------------
# Shared guards
# ---------------------------------------------------------------------------------------------------------------------

function Test-EdrElevated {
    [CmdletBinding()]
    [OutputType([bool])]
    param()
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    ([Security.Principal.WindowsPrincipal]$identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Assert-EdrElevated {
    [CmdletBinding()]
    param()
    if (-not (Test-EdrElevated)) {
        throw 'This command must run in an elevated PowerShell session (Run as administrator).'
    }
}

function Assert-EdrTestVmName {
    [CmdletBinding()]
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Name)
    if ($Name -ne $script:Config.VmName) {
        throw "Refusing to act on VM '$Name': these scripts only manage '$($script:Config.VmName)'."
    }
}

function Get-EdrTestVmRequired {
    [CmdletBinding()]
    param()
    $vm = Get-VM -Name $script:Config.VmName -ErrorAction SilentlyContinue
    if (-not $vm) {
        throw "VM '$($script:Config.VmName)' does not exist. Run New-EdrTestVm.ps1 first."
    }
    Assert-EdrTestVmName -Name $vm.Name
    $vm
}

Export-ModuleMember -Function @(
    'Get-EdrTestVmConfig'
)
```

- [ ] **Step 6: Write `infra/vm/PSScriptAnalyzerSettings.psd1`**

```powershell
@{
    Severity            = @('Error', 'Warning')
    IncludeDefaultRules = $true
    Rules               = @{
        # The guest script and the module it imports run on a fresh Windows install, which only has Windows
        # PowerShell 5.1. Host-only code must parse there too, because it lives in the same module.
        PSUseCompatibleSyntax = @{
            Enable         = $true
            TargetVersions = @('5.1', '7.0')
        }
    }
}
```

- [ ] **Step 7: Run the tests and the analyzer**

Run the VM tests, then PSScriptAnalyzer (Global Constraints).
Expected:
- Pester: `Tests Passed: 13, Failed: 0`. On a machine without `powershell.exe`, the 5.1 import test is skipped.
- PSScriptAnalyzer: no output.

- [ ] **Step 8: Commit**

```powershell
git add infra/vm
git commit -m "feat(vm): EdrTestVm module core, test stubs and analyzer settings

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `New-EdrTestVm` (host: switch, address, firewall, VM)

**Files:**
- Modify: `infra/vm/EdrTestVm.psm1`: insert a section before `Export-ModuleMember` and extend the export list
- Test: `infra/vm/tests/NewEdrTestVm.Tests.ps1`

**Interfaces:**
- Consumes: `$script:Config`, `Assert-EdrElevated`, `Assert-EdrTestVmName` (Task 3); `Get-FakeVm` (tests).
- Produces:
  - Exported: `New-EdrTestVm -IsoPath <string> [-WhatIf]`.
  - Internal: `Initialize-EdrInternalSwitch`, `Initialize-EdrHostAddress`, `Initialize-EdrKdnetFirewallRule`, `New-EdrTestVmMachine -IsoPath <string>`, and `Compare-EdrTestVm -Vm <object>`, which returns `[string[]]` differences, empty when the VM matches.

- [ ] **Step 1: Write the failing tests `infra/vm/tests/NewEdrTestVm.Tests.ps1`**

```powershell
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseDeclaredVarsMoreThanAssignments', '', Justification = 'Pester shares BeforeEach variables with It blocks.')]
param()

BeforeAll {
    . (Join-Path $PSScriptRoot 'TestStubs.ps1')
    Register-EdrTestStub
    Import-Module (Join-Path $PSScriptRoot '..\EdrTestVm.psm1') -Force
}

AfterAll {
    Remove-Module EdrTestVm -ErrorAction SilentlyContinue
    Unregister-EdrTestStub
}

Describe 'New-EdrTestVm' {
    BeforeEach {
        $iso = Join-Path $TestDrive 'eval.iso'
        Set-Content -LiteralPath $iso -Value 'iso'
        $vhdDir = Join-Path $TestDrive 'vhd'
        New-Item -ItemType Directory -Path $vhdDir -Force | Out-Null
        Remove-Item -Path (Join-Path $vhdDir '*') -Force

        Mock -ModuleName EdrTestVm Test-EdrElevated { $true }
        Mock -ModuleName EdrTestVm Get-VMHost { [pscustomobject]@{ VirtualHardDiskPath = $vhdDir } }
        # Fresh host: nothing exists yet.
        Mock -ModuleName EdrTestVm Get-VMSwitch {}
        Mock -ModuleName EdrTestVm Get-NetIPAddress {}
        Mock -ModuleName EdrTestVm Get-NetFirewallRule {}
        Mock -ModuleName EdrTestVm Get-VM {}
        Mock -ModuleName EdrTestVm Get-VMDvdDrive { [pscustomobject]@{ Path = $iso } }
        foreach ($cmd in 'New-VMSwitch', 'New-NetIPAddress', 'New-NetFirewallRule', 'New-VM', 'Set-VM', 'Set-VMFirmware',
            'Set-VMKeyProtector', 'Enable-VMTPM', 'Enable-VMIntegrationService', 'Rename-VMNetworkAdapter',
            'Set-VMNetworkAdapter', 'Add-VMDvdDrive') {
            Mock -ModuleName EdrTestVm $cmd {}
        }
        $mutating = 'New-VMSwitch', 'New-NetIPAddress', 'New-NetFirewallRule', 'New-VM', 'Set-VM', 'Set-VMFirmware',
        'Set-VMKeyProtector', 'Enable-VMTPM', 'Enable-VMIntegrationService', 'Rename-VMNetworkAdapter',
        'Set-VMNetworkAdapter', 'Add-VMDvdDrive'
    }

    It 'on a fresh host creates the switch, host address, firewall rule and VM per spec section 4.2' {
        New-EdrTestVm -IsoPath $iso

        Should -Invoke -ModuleName EdrTestVm New-VMSwitch -Times 1 -Exactly -ParameterFilter {
            $Name -eq 'edr-internal' -and $SwitchType -eq 'Internal'
        }
        Should -Invoke -ModuleName EdrTestVm New-NetIPAddress -Times 1 -Exactly -ParameterFilter {
            $InterfaceAlias -eq 'vEthernet (edr-internal)' -and $IPAddress -eq '192.168.77.1' -and $PrefixLength -eq 24
        }
        Should -Invoke -ModuleName EdrTestVm New-NetFirewallRule -Times 1 -Exactly -ParameterFilter {
            $Direction -eq 'Inbound' -and $Protocol -eq 'UDP' -and $LocalPort -eq 50000 -and
            $InterfaceAlias -eq 'vEthernet (edr-internal)' -and $Action -eq 'Allow'
        }
        Should -Invoke -ModuleName EdrTestVm New-VM -Times 1 -Exactly -ParameterFilter {
            $Name -eq 'edr-test' -and $Generation -eq 2 -and $MemoryStartupBytes -eq 8GB -and
            $NewVHDSizeBytes -eq 80GB -and $NewVHDPath -eq (Join-Path $vhdDir 'edr-test.vhdx') -and
            $SwitchName -eq 'edr-internal'
        }
        Should -Invoke -ModuleName EdrTestVm Set-VM -Times 1 -Exactly -ParameterFilter {
            $ProcessorCount -eq 4 -and $StaticMemory -and $CheckpointType -eq 'Standard' -and
            $AutomaticCheckpointsEnabled -eq $false
        }
        Should -Invoke -ModuleName EdrTestVm Set-VMFirmware -Times 1 -Exactly -ParameterFilter {
            $EnableSecureBoot -eq 'On' -and $SecureBootTemplate -eq 'MicrosoftWindows'
        }
        Should -Invoke -ModuleName EdrTestVm Set-VMKeyProtector -Times 1 -Exactly -ParameterFilter { $NewLocalKeyProtector }
        Should -Invoke -ModuleName EdrTestVm Enable-VMTPM -Times 1 -Exactly
        Should -Invoke -ModuleName EdrTestVm Enable-VMIntegrationService -Times 1 -Exactly -ParameterFilter {
            $Name -eq 'Guest Service Interface'
        }
        Should -Invoke -ModuleName EdrTestVm Rename-VMNetworkAdapter -Times 1 -Exactly -ParameterFilter {
            $NewName -eq 'edr-internal'
        }
        Should -Invoke -ModuleName EdrTestVm Set-VMNetworkAdapter -Times 1 -Exactly -ParameterFilter {
            $Name -eq 'edr-internal' -and $DeviceNaming -eq 'On'
        }
        Should -Invoke -ModuleName EdrTestVm Add-VMDvdDrive -Times 1 -Exactly -ParameterFilter { $Path -eq $iso }
        Should -Invoke -ModuleName EdrTestVm Set-VMFirmware -Times 1 -Exactly -ParameterFilter { $null -ne $FirstBootDevice }
    }

    It 'reuses everything when it all exists and matches' {
        Mock -ModuleName EdrTestVm Get-VMSwitch { [pscustomobject]@{ Name = 'edr-internal' } }
        Mock -ModuleName EdrTestVm Get-NetIPAddress { [pscustomobject]@{ IPAddress = '192.168.77.1' } }
        Mock -ModuleName EdrTestVm Get-NetFirewallRule { [pscustomobject]@{ Name = 'Atlas-EdrTest-KDNET' } }
        Mock -ModuleName EdrTestVm Get-VM { Get-FakeVm }
        Mock -ModuleName EdrTestVm Get-VMNetworkAdapter { [pscustomobject]@{ Name = 'edr-internal'; SwitchName = 'edr-internal' } }

        New-EdrTestVm -IsoPath $iso

        foreach ($cmd in $mutating) { Should -Invoke -ModuleName EdrTestVm $cmd -Times 0 }
    }

    It 'stops without changes when the existing VM differs, listing every difference' {
        Mock -ModuleName EdrTestVm Get-VM { Get-FakeVm @{ ProcessorCount = 2; CheckpointType = 'Production' } }
        Mock -ModuleName EdrTestVm Get-VMNetworkAdapter { [pscustomobject]@{ Name = 'edr-internal'; SwitchName = 'Default Switch' } }

        $err = { New-EdrTestVm -IsoPath $iso } | Should -Throw -PassThru
        $err.Exception.Message | Should -BeLike '*ProcessorCount: expected 4, found 2*'
        $err.Exception.Message | Should -BeLike '*CheckpointType: expected Standard, found Production*'
        $err.Exception.Message | Should -BeLike "*on switch 'Default Switch'*"
        foreach ($cmd in 'New-VM', 'Set-VM', 'Set-VMFirmware', 'Set-VMNetworkAdapter') {
            Should -Invoke -ModuleName EdrTestVm $cmd -Times 0
        }
    }

    It 'refuses to create the VM over a leftover VHDX from a previous build' {
        Set-Content -LiteralPath (Join-Path $vhdDir 'edr-test.vhdx') -Value 'old'
        { New-EdrTestVm -IsoPath $iso } | Should -Throw '*leftover disk*'
        Should -Invoke -ModuleName EdrTestVm New-VM -Times 0
    }

    It 'fails before any change when the ISO does not exist' {
        { New-EdrTestVm -IsoPath (Join-Path $TestDrive 'missing.iso') } | Should -Throw '*ISO not found*'
        foreach ($cmd in $mutating) { Should -Invoke -ModuleName EdrTestVm $cmd -Times 0 }
    }

    It 'refuses to run unelevated, before any change' {
        Mock -ModuleName EdrTestVm Test-EdrElevated { $false }
        { New-EdrTestVm -IsoPath $iso } | Should -Throw '*elevated*'
        foreach ($cmd in $mutating) { Should -Invoke -ModuleName EdrTestVm $cmd -Times 0 }
    }

    It 'changes nothing with -WhatIf' {
        New-EdrTestVm -IsoPath $iso -WhatIf
        foreach ($cmd in $mutating) { Should -Invoke -ModuleName EdrTestVm $cmd -Times 0 }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run the VM tests.
Expected: the 7 `New-EdrTestVm` tests FAIL with `The term 'New-EdrTestVm' is not recognized`. The 13 Task 3 tests pass, plus the new file's ASCII check.

- [ ] **Step 3: Add the host build section to `EdrTestVm.psm1`**

Insert this block immediately **before** the `Export-ModuleMember` line:

```powershell
# ---------------------------------------------------------------------------------------------------------------------
# Host: New-EdrTestVm
# ---------------------------------------------------------------------------------------------------------------------

function Initialize-EdrInternalSwitch {
    [CmdletBinding(SupportsShouldProcess)]
    param()
    $c = $script:Config
    if (Get-VMSwitch -Name $c.InternalSwitch -ErrorAction SilentlyContinue) {
        Write-Verbose "Switch '$($c.InternalSwitch)' exists; reusing it."
        return
    }
    if ($PSCmdlet.ShouldProcess($c.InternalSwitch, 'Create internal virtual switch')) {
        New-VMSwitch -Name $c.InternalSwitch -SwitchType Internal | Out-Null
    }
}

function Initialize-EdrHostAddress {
    [CmdletBinding(SupportsShouldProcess)]
    param()
    $c = $script:Config
    $alias = "vEthernet ($($c.InternalSwitch))"
    $existing = Get-NetIPAddress -InterfaceAlias $alias -AddressFamily IPv4 -ErrorAction SilentlyContinue |
        Where-Object { $_.IPAddress -eq $c.HostIp }
    if ($existing) {
        Write-Verbose "Host address $($c.HostIp) already on '$alias'."
        return
    }
    if ($PSCmdlet.ShouldProcess($alias, "Assign $($c.HostIp)/$($c.PrefixLength)")) {
        New-NetIPAddress -InterfaceAlias $alias -IPAddress $c.HostIp -PrefixLength $c.PrefixLength | Out-Null
    }
}

function Initialize-EdrKdnetFirewallRule {
    [CmdletBinding(SupportsShouldProcess)]
    param()
    $c = $script:Config
    if (Get-NetFirewallRule -Name $c.FirewallRuleName -ErrorAction SilentlyContinue) {
        Write-Verbose "Firewall rule '$($c.FirewallRuleName)' exists; reusing it."
        return
    }
    $alias = "vEthernet ($($c.InternalSwitch))"
    if ($PSCmdlet.ShouldProcess($c.FirewallRuleName, "Allow inbound UDP $($c.KdnetPort) on '$alias' only")) {
        New-NetFirewallRule -Name $c.FirewallRuleName -DisplayName "Atlas edr-test KDNET (UDP $($c.KdnetPort))" `
            -Direction Inbound -Protocol UDP -LocalPort $c.KdnetPort -InterfaceAlias $alias -Action Allow `
            -Profile Any | Out-Null
    }
}

function Compare-EdrTestVm {
    # Returns one string per setting that differs from the spec; empty when the VM matches.
    [CmdletBinding()]
    [OutputType([string[]])]
    param([Parameter(Mandatory)]$Vm)
    $c = $script:Config
    $expected = [ordered]@{
        Generation           = 2
        ProcessorCount       = $c.ProcessorCount
        MemoryStartup        = $c.MemoryBytes
        DynamicMemoryEnabled = $false
        CheckpointType       = 'Standard'
    }
    $diffs = @()
    foreach ($key in $expected.Keys) {
        $actual = $Vm.$key
        if ("$actual" -ne "$($expected[$key])") {
            $diffs += "${key}: expected $($expected[$key]), found $actual"
        }
    }
    $nic = Get-VMNetworkAdapter -VMName $c.VmName -Name $c.InternalAdapter -ErrorAction SilentlyContinue
    if (-not $nic) {
        $diffs += "Network adapter '$($c.InternalAdapter)': missing"
    } elseif ($nic.SwitchName -ne $c.InternalSwitch) {
        $diffs += "Network adapter '$($c.InternalAdapter)': on switch '$($nic.SwitchName)', expected '$($c.InternalSwitch)'"
    }
    , $diffs
}

function New-EdrTestVmMachine {
    [CmdletBinding(SupportsShouldProcess)]
    param([Parameter(Mandatory)][string]$IsoPath)
    $c = $script:Config
    $vhdPath = Join-Path (Get-VMHost).VirtualHardDiskPath "$($c.VmName).vhdx"
    if (Test-Path -LiteralPath $vhdPath) {
        throw "A leftover disk exists at '$vhdPath' but VM '$($c.VmName)' does not. Delete the file, then re-run."
    }
    if (-not $PSCmdlet.ShouldProcess($c.VmName, 'Create Gen 2 VM (vTPM, Secure Boot, eval ISO attached)')) {
        return
    }
    New-VM -Name $c.VmName -Generation 2 -MemoryStartupBytes $c.MemoryBytes -NewVHDPath $vhdPath `
        -NewVHDSizeBytes $c.VhdSizeBytes -SwitchName $c.InternalSwitch | Out-Null
    Set-VM -Name $c.VmName -ProcessorCount $c.ProcessorCount -StaticMemory -CheckpointType Standard `
        -AutomaticCheckpointsEnabled $false
    Set-VMFirmware -VMName $c.VmName -EnableSecureBoot On -SecureBootTemplate MicrosoftWindows
    Set-VMKeyProtector -VMName $c.VmName -NewLocalKeyProtector
    Enable-VMTPM -VMName $c.VmName
    Enable-VMIntegrationService -VMName $c.VmName -Name 'Guest Service Interface'
    Rename-VMNetworkAdapter -VMName $c.VmName -Name 'Network Adapter' -NewName $c.InternalAdapter
    Set-VMNetworkAdapter -VMName $c.VmName -Name $c.InternalAdapter -DeviceNaming On
    Add-VMDvdDrive -VMName $c.VmName -Path $IsoPath
    Set-VMFirmware -VMName $c.VmName -FirstBootDevice (Get-VMDvdDrive -VMName $c.VmName)
}

function New-EdrTestVm {
    [CmdletBinding(SupportsShouldProcess)]
    param([Parameter(Mandatory)][string]$IsoPath)
    Assert-EdrElevated
    if (-not (Test-Path -LiteralPath $IsoPath -PathType Leaf)) {
        throw "ISO not found: '$IsoPath'."
    }
    $IsoPath = (Resolve-Path -LiteralPath $IsoPath).ProviderPath
    Initialize-EdrInternalSwitch
    Initialize-EdrHostAddress
    Initialize-EdrKdnetFirewallRule

    $c = $script:Config
    $vm = Get-VM -Name $c.VmName -ErrorAction SilentlyContinue
    if ($vm) {
        Assert-EdrTestVmName -Name $vm.Name
        $diffs = Compare-EdrTestVm -Vm $vm
        if ($diffs.Count -gt 0) {
            throw ("VM '$($c.VmName)' exists with different settings; not changing it:`n  " + ($diffs -join "`n  "))
        }
        Write-Verbose "VM '$($c.VmName)' exists and matches; reusing it."
        return
    }
    New-EdrTestVmMachine -IsoPath $IsoPath
}
```

Then replace the `Export-ModuleMember` block with:

```powershell
Export-ModuleMember -Function @(
    'Get-EdrTestVmConfig'
    'New-EdrTestVm'
)
```

- [ ] **Step 4: Run the tests and the analyzer**

Expected:
- Pester: `Tests Passed: 21, Failed: 0`. `What if:` lines are printed by the `-WhatIf` test; that is expected.
- PSScriptAnalyzer: no output.

- [ ] **Step 5: Commit**

```powershell
git add infra/vm
git commit -m "feat(vm): New-EdrTestVm creates switch, host IP, KDNET firewall rule and VM

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Day-to-day host commands, wrapper scripts, CI job

**Files:**
- Modify: `infra/vm/EdrTestVm.psm1`: insert a section before `Export-ModuleMember` and extend the export list
- Create: `infra/vm/New-EdrTestVm.ps1`, `infra/vm/Complete-EdrTestVmInstall.ps1`, `infra/vm/Set-EdrTestNetwork.ps1`, `infra/vm/Copy-ToEdrTestVm.ps1`, `infra/vm/Reset-EdrTestVm.ps1`
- Modify: `.github/workflows/ci.yml`: add the `powershell` job
- Test: `infra/vm/tests/HostOps.Tests.ps1`, `infra/vm/tests/Wrappers.Tests.ps1`

**Interfaces:**
- Consumes: `Assert-EdrElevated`, `Get-EdrTestVmRequired`, `$script:Config` (Task 3); `New-EdrTestVm` (Task 4).
- Produces (exported):
  - `Complete-EdrTestVmInstall [-WhatIf]`
  - `Set-EdrTestNetwork -Mode Isolated|Online [-WhatIf]`
  - `Copy-ToEdrTestVm -Path <string[]> [-WhatIf]`
  - `Reset-EdrTestVm [-Checkpoint <string>] [-WhatIf]`
- Produces (scripts): each script has the same parameters as its function and forwards them, including `-WhatIf`, with `@PSBoundParameters`.

- [ ] **Step 1: Write the failing tests `infra/vm/tests/HostOps.Tests.ps1`**

```powershell
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseDeclaredVarsMoreThanAssignments', '', Justification = 'Pester shares BeforeEach variables with It blocks.')]
param()

BeforeAll {
    . (Join-Path $PSScriptRoot 'TestStubs.ps1')
    Register-EdrTestStub
    Import-Module (Join-Path $PSScriptRoot '..\EdrTestVm.psm1') -Force
}

AfterAll {
    Remove-Module EdrTestVm -ErrorAction SilentlyContinue
    Unregister-EdrTestStub
}

Describe 'Day-to-day host commands' {
    BeforeEach {
        Mock -ModuleName EdrTestVm Test-EdrElevated { $true }
        # Mutating calls: tests below assert none of these happen.
        foreach ($cmd in 'Set-VMFirmware', 'Remove-VMDvdDrive', 'Add-VMNetworkAdapter', 'Remove-VMNetworkAdapter',
            'Copy-VMFile', 'Restore-VMCheckpoint', 'Start-VM', 'New-VM', 'New-VMSwitch') {
            Mock -ModuleName EdrTestVm $cmd {}
        }
    }

    It '<Command> refuses to run unelevated, before touching Hyper-V' -TestCases @(
        @{ Command = 'Complete-EdrTestVmInstall'; Params = @{} }
        @{ Command = 'Set-EdrTestNetwork'; Params = @{ Mode = 'Online' } }
        @{ Command = 'Copy-ToEdrTestVm'; Params = @{ Path = 'C:\x.txt' } }
        @{ Command = 'Reset-EdrTestVm'; Params = @{} }
    ) {
        Mock -ModuleName EdrTestVm Test-EdrElevated { $false }
        Mock -ModuleName EdrTestVm Get-VM {}
        { & $Command @Params } | Should -Throw '*elevated*'
        Should -Invoke -ModuleName EdrTestVm Get-VM -Times 0
    }

    It '<Command> refuses a VM that is not edr-test' -TestCases @(
        @{ Command = 'Complete-EdrTestVmInstall'; Params = @{} }
        @{ Command = 'Set-EdrTestNetwork'; Params = @{ Mode = 'Online' } }
        @{ Command = 'Copy-ToEdrTestVm'; Params = @{ Path = 'C:\x.txt' } }
        @{ Command = 'Reset-EdrTestVm'; Params = @{} }
    ) {
        Mock -ModuleName EdrTestVm Get-VM { [pscustomobject]@{ Name = 'prod-dc'; State = 'Off' } }
        { & $Command @Params } | Should -Throw '*Refusing*'
        foreach ($cmd in 'Set-VMFirmware', 'Add-VMNetworkAdapter', 'Copy-VMFile', 'Restore-VMCheckpoint') {
            Should -Invoke -ModuleName EdrTestVm $cmd -Times 0
        }
    }

    It '<Command> explains when the VM does not exist' -TestCases @(
        @{ Command = 'Complete-EdrTestVmInstall'; Params = @{} }
        @{ Command = 'Set-EdrTestNetwork'; Params = @{ Mode = 'Online' } }
        @{ Command = 'Copy-ToEdrTestVm'; Params = @{ Path = 'C:\x.txt' } }
        @{ Command = 'Reset-EdrTestVm'; Params = @{} }
    ) {
        Mock -ModuleName EdrTestVm Get-VM {}
        { & $Command @Params } | Should -Throw '*does not exist*New-EdrTestVm*'
    }
}

Describe 'Complete-EdrTestVmInstall' {
    BeforeEach {
        Mock -ModuleName EdrTestVm Test-EdrElevated { $true }
        Mock -ModuleName EdrTestVm Get-VM { Get-FakeVm }
        Mock -ModuleName EdrTestVm Get-VMDvdDrive { [pscustomobject]@{ Path = 'C:\eval.iso' } }
        Mock -ModuleName EdrTestVm Get-VMHardDiskDrive { [pscustomobject]@{ Path = 'C:\vhd\edr-test.vhdx' } }
        Mock -ModuleName EdrTestVm Set-VMFirmware {}
        Mock -ModuleName EdrTestVm Remove-VMDvdDrive {}
    }

    It 'turns Secure Boot off, ejects the ISO and boots from the disk' {
        Complete-EdrTestVmInstall
        Should -Invoke -ModuleName EdrTestVm Set-VMFirmware -Times 1 -Exactly -ParameterFilter { $EnableSecureBoot -eq 'Off' }
        Should -Invoke -ModuleName EdrTestVm Remove-VMDvdDrive -Times 1 -Exactly
        Should -Invoke -ModuleName EdrTestVm Set-VMFirmware -Times 1 -Exactly -ParameterFilter {
            $FirstBootDevice.Path -eq 'C:\vhd\edr-test.vhdx'
        }
    }

    It 'is safe to re-run when the ISO is already ejected' {
        Mock -ModuleName EdrTestVm Get-VMDvdDrive {}
        { Complete-EdrTestVmInstall } | Should -Not -Throw
        Should -Invoke -ModuleName EdrTestVm Remove-VMDvdDrive -Times 0
    }

    It 'refuses while the VM is running' {
        Mock -ModuleName EdrTestVm Get-VM { Get-FakeVm @{ State = 'Running' } }
        { Complete-EdrTestVmInstall } | Should -Throw '*Shut it down*'
        Should -Invoke -ModuleName EdrTestVm Set-VMFirmware -Times 0
    }

    It 'changes nothing with -WhatIf' {
        Complete-EdrTestVmInstall -WhatIf
        Should -Invoke -ModuleName EdrTestVm Set-VMFirmware -Times 0
        Should -Invoke -ModuleName EdrTestVm Remove-VMDvdDrive -Times 0
    }
}

Describe 'Set-EdrTestNetwork' {
    BeforeEach {
        Mock -ModuleName EdrTestVm Test-EdrElevated { $true }
        Mock -ModuleName EdrTestVm Get-VM { Get-FakeVm @{ State = 'Running' } }
        Mock -ModuleName EdrTestVm Add-VMNetworkAdapter {}
        Mock -ModuleName EdrTestVm Remove-VMNetworkAdapter {}
    }

    It 'Online adds edr-online on the Default Switch with device naming' {
        Mock -ModuleName EdrTestVm Get-VMNetworkAdapter {}
        Set-EdrTestNetwork -Mode Online
        Should -Invoke -ModuleName EdrTestVm Add-VMNetworkAdapter -Times 1 -Exactly -ParameterFilter {
            $VMName -eq 'edr-test' -and $Name -eq 'edr-online' -and $SwitchName -eq 'Default Switch' -and $DeviceNaming -eq 'On'
        }
    }

    It 'Online is a no-op when already online' {
        Mock -ModuleName EdrTestVm Get-VMNetworkAdapter { [pscustomobject]@{ Name = 'edr-online' } }
        Set-EdrTestNetwork -Mode Online
        Should -Invoke -ModuleName EdrTestVm Add-VMNetworkAdapter -Times 0
    }

    It 'Isolated removes only edr-online, never edr-internal' {
        Mock -ModuleName EdrTestVm Get-VMNetworkAdapter { [pscustomobject]@{ Name = 'edr-online' } }
        Set-EdrTestNetwork -Mode Isolated
        Should -Invoke -ModuleName EdrTestVm Remove-VMNetworkAdapter -Times 1 -Exactly -ParameterFilter { $Name -eq 'edr-online' }
        Should -Invoke -ModuleName EdrTestVm Remove-VMNetworkAdapter -Times 0 -ParameterFilter { $Name -ne 'edr-online' }
    }

    It 'Isolated is a no-op when already isolated' {
        Mock -ModuleName EdrTestVm Get-VMNetworkAdapter {}
        Set-EdrTestNetwork -Mode Isolated
        Should -Invoke -ModuleName EdrTestVm Remove-VMNetworkAdapter -Times 0
    }

    It 'rejects an unknown mode' {
        { Set-EdrTestNetwork -Mode Bridged } | Should -Throw
    }

    It 'changes nothing with -WhatIf' {
        Mock -ModuleName EdrTestVm Get-VMNetworkAdapter {}
        Set-EdrTestNetwork -Mode Online -WhatIf
        Should -Invoke -ModuleName EdrTestVm Add-VMNetworkAdapter -Times 0
    }
}

Describe 'Copy-ToEdrTestVm' {
    BeforeEach {
        Mock -ModuleName EdrTestVm Test-EdrElevated { $true }
        Mock -ModuleName EdrTestVm Get-VM { Get-FakeVm @{ State = 'Running' } }
        Mock -ModuleName EdrTestVm Copy-VMFile {}
        $a = Join-Path $TestDrive 'agent.exe'; Set-Content -LiteralPath $a -Value 'a'
        $b = Join-Path $TestDrive 'rules.yml'; Set-Content -LiteralPath $b -Value 'b'
    }

    It 'copies each file into C:\atlas\ under its own name' {
        Copy-ToEdrTestVm -Path $a, $b
        Should -Invoke -ModuleName EdrTestVm Copy-VMFile -Times 1 -Exactly -ParameterFilter {
            $Name -eq 'edr-test' -and $SourcePath -eq $a -and $DestinationPath -eq 'C:\atlas\agent.exe' -and
            $FileSource -eq 'Host' -and $CreateFullPath -and $Force
        }
        Should -Invoke -ModuleName EdrTestVm Copy-VMFile -Times 1 -Exactly -ParameterFilter {
            $DestinationPath -eq 'C:\atlas\rules.yml'
        }
    }

    It 'copies nothing when any path is a folder or missing' {
        { Copy-ToEdrTestVm -Path $a, $TestDrive } | Should -Throw '*Not a file*'
        { Copy-ToEdrTestVm -Path $a, (Join-Path $TestDrive 'nope.txt') } | Should -Throw '*Not a file*'
        Should -Invoke -ModuleName EdrTestVm Copy-VMFile -Times 0
    }

    It 'refuses when the VM is not running' {
        Mock -ModuleName EdrTestVm Get-VM { Get-FakeVm @{ State = 'Off' } }
        { Copy-ToEdrTestVm -Path $a } | Should -Throw '*needs it Running*'
    }
}

Describe 'Reset-EdrTestVm' {
    BeforeEach {
        Mock -ModuleName EdrTestVm Test-EdrElevated { $true }
        Mock -ModuleName EdrTestVm Get-VM { Get-FakeVm @{ State = 'Running' } }
        Mock -ModuleName EdrTestVm Get-VMCheckpoint { [pscustomobject]@{ Name = $Name } }
        Mock -ModuleName EdrTestVm Restore-VMCheckpoint {}
        Mock -ModuleName EdrTestVm Start-VM {}
    }

    It 'restores baseline by default and starts the VM' {
        Reset-EdrTestVm
        Should -Invoke -ModuleName EdrTestVm Restore-VMCheckpoint -Times 1 -Exactly -ParameterFilter {
            $VMName -eq 'edr-test' -and $Name -eq 'baseline'
        }
        Should -Invoke -ModuleName EdrTestVm Start-VM -Times 1 -Exactly
    }

    It 'restores a named checkpoint' {
        Reset-EdrTestVm -Checkpoint 'pre-driver'
        Should -Invoke -ModuleName EdrTestVm Restore-VMCheckpoint -Times 1 -Exactly -ParameterFilter { $Name -eq 'pre-driver' }
    }

    It 'fails clearly when the checkpoint does not exist' {
        Mock -ModuleName EdrTestVm Get-VMCheckpoint {}
        { Reset-EdrTestVm } | Should -Throw "*no checkpoint named 'baseline'*"
        Should -Invoke -ModuleName EdrTestVm Restore-VMCheckpoint -Times 0
    }
}
```

- [ ] **Step 2: Write the failing tests `infra/vm/tests/Wrappers.Tests.ps1`**

Task 6 adds the guest script's test case to this file.

```powershell
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseDeclaredVarsMoreThanAssignments', '', Justification = 'Pester shares BeforeEach variables with It blocks.')]
param()

BeforeAll {
    . (Join-Path $PSScriptRoot 'TestStubs.ps1')
    Register-EdrTestStub
    Import-Module (Join-Path $PSScriptRoot '..\EdrTestVm.psm1') -Force
}

AfterAll {
    Remove-Module EdrTestVm -ErrorAction SilentlyContinue
    Unregister-EdrTestStub
}

Describe 'Wrapper scripts' {
    It '<Script> forwards its arguments and -WhatIf to <Function>' -TestCases @(
        @{ Script = 'New-EdrTestVm.ps1'; Function = 'New-EdrTestVm'; Params = @{ IsoPath = 'C:\eval.iso' } }
        @{ Script = 'Complete-EdrTestVmInstall.ps1'; Function = 'Complete-EdrTestVmInstall'; Params = @{} }
        @{ Script = 'Set-EdrTestNetwork.ps1'; Function = 'Set-EdrTestNetwork'; Params = @{ Mode = 'Isolated' } }
        @{ Script = 'Copy-ToEdrTestVm.ps1'; Function = 'Copy-ToEdrTestVm'; Params = @{ Path = @('C:\a', 'C:\b') } }
        @{ Script = 'Reset-EdrTestVm.ps1'; Function = 'Reset-EdrTestVm'; Params = @{ Checkpoint = 'pre-driver' } }
    ) {
        Mock Import-Module {}
        Mock $Function {}
        & (Join-Path $PSScriptRoot "..\$Script") @Params -WhatIf
        Should -Invoke $Function -Times 1 -Exactly -ParameterFilter {
            $ok = $PesterBoundParameters.ContainsKey('WhatIf')
            foreach ($k in $Params.Keys) {
                $ok = $ok -and ("$($PesterBoundParameters[$k])" -eq "$($Params[$k])")
            }
            $ok
        }
        Should -Invoke Import-Module -Times 1 -Exactly -ParameterFilter {
            $Name -like '*EdrTestVm.psm1' -and (Test-Path -LiteralPath $Name)
        }
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Expected: the new tests FAIL: the commands are not recognized, and the wrapper scripts don't exist. The 21 earlier tests still pass.

- [ ] **Step 4: Add the day-to-day section to `EdrTestVm.psm1`**

Insert this block immediately **before** the `Export-ModuleMember` line:

```powershell
# ---------------------------------------------------------------------------------------------------------------------
# Host: day-to-day operations
# ---------------------------------------------------------------------------------------------------------------------

function Complete-EdrTestVmInstall {
    [CmdletBinding(SupportsShouldProcess)]
    param()
    Assert-EdrElevated
    $vm = Get-EdrTestVmRequired
    if ("$($vm.State)" -ne 'Off') {
        throw "VM '$($vm.Name)' is $($vm.State). Shut it down from inside Windows first, then re-run."
    }
    if ($PSCmdlet.ShouldProcess($vm.Name, 'Turn Secure Boot off, eject ISO, boot from disk')) {
        Set-VMFirmware -VMName $vm.Name -EnableSecureBoot Off
        foreach ($drive in @(Get-VMDvdDrive -VMName $vm.Name)) {
            Remove-VMDvdDrive -VMDvdDrive $drive
        }
        Set-VMFirmware -VMName $vm.Name -FirstBootDevice (Get-VMHardDiskDrive -VMName $vm.Name | Select-Object -First 1)
    }
}

function Set-EdrTestNetwork {
    [CmdletBinding(SupportsShouldProcess)]
    param([Parameter(Mandatory)][ValidateSet('Isolated', 'Online')][string]$Mode)
    Assert-EdrElevated
    $c = $script:Config
    $vm = Get-EdrTestVmRequired
    $online = Get-VMNetworkAdapter -VMName $vm.Name -Name $c.OnlineAdapter -ErrorAction SilentlyContinue
    if ($Mode -eq 'Online') {
        if ($online) {
            Write-Verbose 'Already online.'
            return
        }
        if ($PSCmdlet.ShouldProcess($vm.Name, "Add NIC '$($c.OnlineAdapter)' on '$($c.OnlineSwitch)'")) {
            Add-VMNetworkAdapter -VMName $vm.Name -Name $c.OnlineAdapter -SwitchName $c.OnlineSwitch -DeviceNaming On
        }
    } else {
        if (-not $online) {
            Write-Verbose 'Already isolated.'
            return
        }
        if ($PSCmdlet.ShouldProcess($vm.Name, "Remove NIC '$($c.OnlineAdapter)'")) {
            Remove-VMNetworkAdapter -VMName $vm.Name -Name $c.OnlineAdapter
        }
    }
}

function Copy-ToEdrTestVm {
    [CmdletBinding(SupportsShouldProcess)]
    param([Parameter(Mandatory)][string[]]$Path)
    Assert-EdrElevated
    $c = $script:Config
    $vm = Get-EdrTestVmRequired
    if ("$($vm.State)" -ne 'Running') {
        throw "VM '$($vm.Name)' is $($vm.State); Copy-VMFile needs it Running."
    }
    # Validate everything before copying anything.
    $files = foreach ($p in $Path) {
        if (-not (Test-Path -LiteralPath $p -PathType Leaf)) {
            throw "Not a file: '$p'. Copy-ToEdrTestVm copies files, not folders."
        }
        (Resolve-Path -LiteralPath $p).ProviderPath
    }
    foreach ($file in $files) {
        $dest = Join-Path $c.GuestDropPath (Split-Path -Leaf $file)
        if ($PSCmdlet.ShouldProcess($vm.Name, "Copy '$file' to '$dest'")) {
            Copy-VMFile -Name $vm.Name -SourcePath $file -DestinationPath $dest -FileSource Host -CreateFullPath -Force
        }
    }
}

function Reset-EdrTestVm {
    [CmdletBinding(SupportsShouldProcess)]
    param([string]$Checkpoint = $script:Config.BaselineCheckpoint)
    Assert-EdrElevated
    $vm = Get-EdrTestVmRequired
    if (-not (Get-VMCheckpoint -VMName $vm.Name -Name $Checkpoint -ErrorAction SilentlyContinue)) {
        throw "VM '$($vm.Name)' has no checkpoint named '$Checkpoint'."
    }
    if ($PSCmdlet.ShouldProcess($vm.Name, "Restore checkpoint '$Checkpoint' and start")) {
        Restore-VMCheckpoint -VMName $vm.Name -Name $Checkpoint -Confirm:$false
        Start-VM -Name $vm.Name
    }
}
```

Then replace the `Export-ModuleMember` block with:

```powershell
Export-ModuleMember -Function @(
    'Get-EdrTestVmConfig'
    'New-EdrTestVm'
    'Complete-EdrTestVmInstall'
    'Set-EdrTestNetwork'
    'Copy-ToEdrTestVm'
    'Reset-EdrTestVm'
)
```

- [ ] **Step 5: Write the five wrapper scripts**

`infra/vm/New-EdrTestVm.ps1`:
```powershell
<#
.SYNOPSIS
Creates the edr-test VM, the edr-internal switch, the host address and the KDNET firewall rule.
.NOTES
Runs on the host, elevated. Supports -WhatIf. Runbook: docs/runbooks/edr-test-vm.md
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSShouldProcess', '', Justification = 'Delegates to the module function, which calls ShouldProcess.')]
[CmdletBinding(SupportsShouldProcess)]
param([Parameter(Mandatory)][string]$IsoPath)

Import-Module (Join-Path $PSScriptRoot 'EdrTestVm.psm1') -Force
New-EdrTestVm @PSBoundParameters
```

`infra/vm/Complete-EdrTestVmInstall.ps1`:
```powershell
<#
.SYNOPSIS
After the manual Windows install: turns Secure Boot off, ejects the ISO, boots from disk.
.NOTES
Runs on the host, elevated. Supports -WhatIf. Runbook: docs/runbooks/edr-test-vm.md
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSShouldProcess', '', Justification = 'Delegates to the module function, which calls ShouldProcess.')]
[CmdletBinding(SupportsShouldProcess)]
param()

Import-Module (Join-Path $PSScriptRoot 'EdrTestVm.psm1') -Force
Complete-EdrTestVmInstall @PSBoundParameters
```

`infra/vm/Set-EdrTestNetwork.ps1`:
```powershell
<#
.SYNOPSIS
Isolated removes the NAT NIC; Online adds it. The internal NIC is never touched.
.NOTES
Runs on the host, elevated. Supports -WhatIf. Runbook: docs/runbooks/edr-test-vm.md
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSShouldProcess', '', Justification = 'Delegates to the module function, which calls ShouldProcess.')]
[CmdletBinding(SupportsShouldProcess)]
param([Parameter(Mandatory)][ValidateSet('Isolated', 'Online')][string]$Mode)

Import-Module (Join-Path $PSScriptRoot 'EdrTestVm.psm1') -Force
Set-EdrTestNetwork @PSBoundParameters
```

`infra/vm/Copy-ToEdrTestVm.ps1`:
```powershell
<#
.SYNOPSIS
Copies files into the guest's C:\atlas\ over VMBus (works while isolated).
.NOTES
Runs on the host, elevated. Supports -WhatIf. Runbook: docs/runbooks/edr-test-vm.md
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSShouldProcess', '', Justification = 'Delegates to the module function, which calls ShouldProcess.')]
[CmdletBinding(SupportsShouldProcess)]
param([Parameter(Mandatory)][string[]]$Path)

Import-Module (Join-Path $PSScriptRoot 'EdrTestVm.psm1') -Force
Copy-ToEdrTestVm @PSBoundParameters
```

`infra/vm/Reset-EdrTestVm.ps1` has no default for `-Checkpoint`: an unbound parameter isn't forwarded, so the module's default (`baseline`) applies.
```powershell
<#
.SYNOPSIS
Restores a checkpoint (default: baseline) and starts the VM.
.NOTES
Runs on the host, elevated. Supports -WhatIf. Runbook: docs/runbooks/edr-test-vm.md
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSShouldProcess', '', Justification = 'Delegates to the module function, which calls ShouldProcess.')]
[CmdletBinding(SupportsShouldProcess)]
param([string]$Checkpoint)

Import-Module (Join-Path $PSScriptRoot 'EdrTestVm.psm1') -Force
Reset-EdrTestVm @PSBoundParameters
```

- [ ] **Step 6: Run the tests and the analyzer**

Expected:
- Pester: `Tests Passed: 61, Failed: 0`.
- PSScriptAnalyzer: no output. Each wrapper suppresses `PSShouldProcess`, because the function it calls does the ShouldProcess checks.

- [ ] **Step 7: Add the `powershell` job to `.github/workflows/ci.yml`**

Insert this job between the `proto` and `audit` jobs:

```yaml
  powershell:
    runs-on: windows-latest
    defaults:
      run:
        shell: pwsh
    steps:
      - uses: actions/checkout@v7
      - name: Install Pester and PSScriptAnalyzer
        run: |
          Set-PSRepository PSGallery -InstallationPolicy Trusted
          Install-Module Pester -RequiredVersion 5.9.1 -Scope CurrentUser -SkipPublisherCheck -Force
          Install-Module PSScriptAnalyzer -RequiredVersion 1.25.0 -Scope CurrentUser -Force
      - name: PSScriptAnalyzer
        run: |
          $findings = Invoke-ScriptAnalyzer -Path infra/vm -Recurse -Settings infra/vm/PSScriptAnalyzerSettings.psd1
          $findings | Format-Table RuleName, Severity, ScriptName, Line, Message -AutoSize -Wrap
          if ($findings) { throw "PSScriptAnalyzer: $(@($findings).Count) finding(s)" }
      - name: Pester
        run: |
          Import-Module Pester -RequiredVersion 5.9.1
          $config = New-PesterConfiguration
          $config.Run.Path = 'infra/vm/tests'
          $config.Run.Exit = $true
          $config.Output.Verbosity = 'Detailed'
          Invoke-Pester -Configuration $config
```

Run: `& "$env:TEMP\actionlint\actionlint.exe" -no-color .github/workflows/ci.yml`
Expected: no output.

- [ ] **Step 8: Commit, push, watch CI**

```powershell
git add infra/vm .github/workflows/ci.yml
git commit -m "feat(vm): host commands and wrapper scripts; run Pester and PSScriptAnalyzer in CI

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push
Start-Sleep 15  # let GitHub register the run
gh run watch (gh run list --branch feat-0b-scaffolding --workflow ci --limit 1 --json databaseId --jq '.[0].databaseId') --exit-status
```
Expected: all five jobs succeed. In the `powershell` job log, Pester reports 61 passed, including the 5.1 import test, because `windows-latest` has `powershell.exe`.

---

### Task 6: Guest setup `Initialize-EdrTestGuest`

**Files:**
- Modify: `infra/vm/EdrTestVm.psm1`: insert a section before `Export-ModuleMember` and extend the export list
- Create: `infra/vm/guest/Initialize-EdrTestGuest.ps1`
- Modify: `infra/vm/tests/Wrappers.Tests.ps1`: add the guest test case
- Test: `infra/vm/tests/Guest.Tests.ps1`

**Interfaces:**
- Consumes: `$script:Config`, `Test-EdrElevated` (Task 3).
- Produces:
  - Exported: `Initialize-EdrTestGuest [-WhatIf]`.
  - Internal:
    - `Invoke-EdrBcdedit -ArgumentList <string[]>` → `[string[]]`
    - `Invoke-EdrWinget -Id <string>`
    - `ConvertFrom-EdrDbgSetting -Text <string[]>` → `[hashtable]`
    - `Find-EdrGuestAdapter -HyperVName <string>` → interface alias or `$null`
    - `Test-EdrGuestPrecondition` → `[string[]]`
    - `Set-EdrGuestAddress`
    - `Set-EdrGuestKdnet` → key `[string]`
    - `Write-EdrRegistryDword -Path -Name -Value`

- [ ] **Step 1: Write the failing tests `infra/vm/tests/Guest.Tests.ps1`**

```powershell
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseDeclaredVarsMoreThanAssignments', '', Justification = 'Pester shares BeforeEach variables with It blocks.')]
param()

BeforeAll {
    . (Join-Path $PSScriptRoot 'TestStubs.ps1')
    Register-EdrTestStub
    Import-Module (Join-Path $PSScriptRoot '..\EdrTestVm.psm1') -Force

    # `bcdedit /dbgsettings` output after KDNET has been configured.
    $script:ConfiguredDbgSettings = @(
        'key                     2steg4fzbj2sz.23418vzkd4ko3.1g34ou07z4pev.1sp3yo9yz874p'
        'debugtype               NET'
        'hostip                  192.168.77.1'
        'port                    50000'
        'dhcp                    Yes'
        'The operation completed successfully.'
    )
    # `bcdedit /dbgsettings` output on a fresh install.
    $script:DefaultDbgSettings = @(
        'debugtype               Local'
        'debugstart              Active'
        'noumex                  Yes'
        'The operation completed successfully.'
    )
}

AfterAll {
    Remove-Module EdrTestVm -ErrorAction SilentlyContinue
    Unregister-EdrTestStub
}

Describe 'ConvertFrom-EdrDbgSetting' {
    It 'parses name/value lines and ignores the status line' {
        InModuleScope EdrTestVm -Parameters @{ Text = $ConfiguredDbgSettings } {
            $s = ConvertFrom-EdrDbgSetting -Text $Text
            $s['key'] | Should -Be '2steg4fzbj2sz.23418vzkd4ko3.1g34ou07z4pev.1sp3yo9yz874p'
            $s['debugtype'] | Should -Be 'NET'
            $s['hostip'] | Should -Be '192.168.77.1'
            $s['port'] | Should -Be '50000'
            $s.ContainsKey('The') | Should -BeFalse
        }
    }
}

Describe 'Find-EdrGuestAdapter' {
    It 'finds the interface by its Hyper-V device name, not its alias' {
        Mock -ModuleName EdrTestVm Get-NetAdapterAdvancedProperty {
            [pscustomobject]@{ Name = 'Ethernet'; DisplayValue = 'edr-online' }
            [pscustomobject]@{ Name = 'Ethernet 2'; DisplayValue = 'edr-internal' }
        }
        InModuleScope EdrTestVm {
            Find-EdrGuestAdapter -HyperVName 'edr-internal' | Should -Be 'Ethernet 2'
            Find-EdrGuestAdapter -HyperVName 'edr-nope' | Should -BeNullOrEmpty
        }
    }
}

Describe 'Initialize-EdrTestGuest' {
    BeforeEach {
        # A guest that is ready: elevated, Secure Boot off, Tamper Protection off, online, winget present.
        Mock -ModuleName EdrTestVm Test-EdrElevated { $true }
        Mock -ModuleName EdrTestVm Confirm-SecureBootUEFI { $false }
        Mock -ModuleName EdrTestVm Get-MpComputerStatus { [pscustomobject]@{ IsTamperProtected = $false } }
        Mock -ModuleName EdrTestVm Get-NetAdapterAdvancedProperty {
            [pscustomobject]@{ Name = 'Ethernet'; DisplayValue = 'edr-internal' }
            [pscustomobject]@{ Name = 'Ethernet 2'; DisplayValue = 'edr-online' }
        }
        Mock -ModuleName EdrTestVm Get-Command { [pscustomobject]@{ Name = 'winget.exe' } } -ParameterFilter { $Name -eq 'winget.exe' }
        Mock -ModuleName EdrTestVm Get-NetIPAddress {}
        Mock -ModuleName EdrTestVm Invoke-EdrBcdedit { $DefaultDbgSettings } -ParameterFilter { "$ArgumentList" -eq '/dbgsettings' }
        Mock -ModuleName EdrTestVm Invoke-EdrBcdedit { 'Key=new.key.value.here' } -ParameterFilter { $ArgumentList[1] -eq 'net' }
        Mock -ModuleName EdrTestVm Invoke-EdrBcdedit { 'The operation completed successfully.' } -ParameterFilter {
            $ArgumentList[0] -in '/set', '/debug'
        }
        foreach ($cmd in 'Set-NetIPInterface', 'New-NetIPAddress', 'Set-MpPreference', 'Invoke-EdrWinget', 'Write-EdrRegistryDword',
            'Write-Information') {
            Mock -ModuleName EdrTestVm $cmd {}
        }
        $changes = 'Set-NetIPInterface', 'New-NetIPAddress', 'Set-MpPreference', 'Invoke-EdrWinget', 'Write-EdrRegistryDword'
    }

    It 'configures the guest per spec section 4.3' {
        Initialize-EdrTestGuest

        Should -Invoke -ModuleName EdrTestVm Set-NetIPInterface -Times 1 -Exactly -ParameterFilter {
            $InterfaceAlias -eq 'Ethernet' -and $Dhcp -eq 'Disabled'
        }
        Should -Invoke -ModuleName EdrTestVm New-NetIPAddress -Times 1 -Exactly -ParameterFilter {
            $InterfaceAlias -eq 'Ethernet' -and $IPAddress -eq '192.168.77.10' -and $PrefixLength -eq 24
        }
        Should -Invoke -ModuleName EdrTestVm Invoke-EdrBcdedit -Times 1 -Exactly -ParameterFilter {
            "$ArgumentList" -eq '/set testsigning on'
        }
        Should -Invoke -ModuleName EdrTestVm Invoke-EdrBcdedit -Times 1 -Exactly -ParameterFilter {
            "$ArgumentList" -eq '/dbgsettings net hostip:192.168.77.1 port:50000'
        }
        Should -Invoke -ModuleName EdrTestVm Invoke-EdrBcdedit -Times 1 -Exactly -ParameterFilter { "$ArgumentList" -eq '/debug on' }
        Should -Invoke -ModuleName EdrTestVm Write-EdrRegistryDword -Times 1 -Exactly -ParameterFilter {
            $Path -like '*DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity' -and $Name -eq 'Enabled' -and $Value -eq 0
        }
        Should -Invoke -ModuleName EdrTestVm Set-MpPreference -Times 1 -Exactly -ParameterFilter {
            $MAPSReporting -eq 'Disabled' -and $SubmitSamplesConsent -eq 'NeverSend' -and $DisableRealtimeMonitoring -eq $true
        }
        Should -Invoke -ModuleName EdrTestVm Invoke-EdrWinget -Times 1 -Exactly -ParameterFilter { $Id -eq 'Microsoft.WinDbg' }
        Should -Invoke -ModuleName EdrTestVm Invoke-EdrWinget -Times 1 -Exactly -ParameterFilter {
            $Id -eq 'Microsoft.Sysinternals.Suite'
        }
        Should -Invoke -ModuleName EdrTestVm Write-Information -ParameterFilter {
            "$MessageData" -like '*windbg -k net:port=50000,key=new.key.value.here*'
        }
    }

    It 'keeps an existing matching KDNET configuration so the key does not change' {
        Mock -ModuleName EdrTestVm Invoke-EdrBcdedit { $ConfiguredDbgSettings } -ParameterFilter { "$ArgumentList" -eq '/dbgsettings' }
        Mock -ModuleName EdrTestVm Get-NetIPAddress { [pscustomobject]@{ IPAddress = '192.168.77.10' } }

        Initialize-EdrTestGuest

        Should -Invoke -ModuleName EdrTestVm Invoke-EdrBcdedit -Times 0 -ParameterFilter { $ArgumentList[1] -eq 'net' }
        Should -Invoke -ModuleName EdrTestVm New-NetIPAddress -Times 0
        Should -Invoke -ModuleName EdrTestVm Write-Information -ParameterFilter {
            "$MessageData" -like '*key=2steg4fzbj2sz.23418vzkd4ko3.1g34ou07z4pev.1sp3yo9yz874p*'
        }
    }

    It 'reconfigures KDNET when the existing settings point elsewhere' {
        $other = $ConfiguredDbgSettings -replace '50000', '50001'
        Mock -ModuleName EdrTestVm Invoke-EdrBcdedit { $other } -ParameterFilter { "$ArgumentList" -eq '/dbgsettings' }
        Initialize-EdrTestGuest
        Should -Invoke -ModuleName EdrTestVm Invoke-EdrBcdedit -Times 1 -Exactly -ParameterFilter { $ArgumentList[1] -eq 'net' }
    }

    It 'stops before any change when <Case>' -TestCases @(
        @{ Case = 'not elevated'; MockCmd = 'Test-EdrElevated'; MockBody = { $false }; Message = '*Not elevated*' }
        @{ Case = 'Secure Boot is on'; MockCmd = 'Confirm-SecureBootUEFI'; MockBody = { $true }; Message = '*Complete-EdrTestVmInstall*' }
        @{ Case = 'Tamper Protection is on'; MockCmd = 'Get-MpComputerStatus'; MockBody = { [pscustomobject]@{ IsTamperProtected = $true } }
            Message = '*Tamper Protection*'
        }
        @{ Case = 'offline'; MockCmd = 'Get-NetAdapterAdvancedProperty'
            MockBody = { [pscustomobject]@{ Name = 'Ethernet'; DisplayValue = 'edr-internal' } }; Message = '*-Mode Online*'
        }
        @{ Case = 'the internal NIC is missing'; MockCmd = 'Get-NetAdapterAdvancedProperty'
            MockBody = { [pscustomobject]@{ Name = 'Ethernet 2'; DisplayValue = 'edr-online' } }; Message = "*No NIC named 'edr-internal'*"
        }
    ) {
        Mock -ModuleName EdrTestVm $MockCmd $MockBody
        { Initialize-EdrTestGuest } | Should -Throw $Message
        foreach ($c in $changes) { Should -Invoke -ModuleName EdrTestVm $c -Times 0 }
        Should -Invoke -ModuleName EdrTestVm Invoke-EdrBcdedit -Times 0
    }

    It 'stops before any change when winget is missing' {
        Mock -ModuleName EdrTestVm Get-Command {} -ParameterFilter { $Name -eq 'winget.exe' }
        { Initialize-EdrTestGuest } | Should -Throw '*App Installer*'
        foreach ($c in $changes) { Should -Invoke -ModuleName EdrTestVm $c -Times 0 }
    }

    It 'reports every unmet precondition at once' {
        Mock -ModuleName EdrTestVm Confirm-SecureBootUEFI { $true }
        Mock -ModuleName EdrTestVm Get-MpComputerStatus { [pscustomobject]@{ IsTamperProtected = $true } }
        $err = { Initialize-EdrTestGuest } | Should -Throw -PassThru
        $err.Exception.Message | Should -BeLike '*Secure Boot*'
        $err.Exception.Message | Should -BeLike '*Tamper Protection*'
    }

    It 'changes nothing with -WhatIf' {
        Initialize-EdrTestGuest -WhatIf
        foreach ($c in $changes) { Should -Invoke -ModuleName EdrTestVm $c -Times 0 }
        Should -Invoke -ModuleName EdrTestVm Invoke-EdrBcdedit -Times 0 -ParameterFilter { "$ArgumentList" -ne '/dbgsettings' }
    }
}
```

- [ ] **Step 2: Add the guest case to `infra/vm/tests/Wrappers.Tests.ps1`**

In the `-TestCases` list, after the `Reset-EdrTestVm.ps1` line, add:

```powershell
        @{ Script = 'guest\Initialize-EdrTestGuest.ps1'; Function = 'Initialize-EdrTestGuest'; Params = @{} }
```

- [ ] **Step 3: Run the tests to verify they fail**

Expected: the new guest tests and the new wrapper case FAIL (`Initialize-EdrTestGuest` is not recognized, and the guest script doesn't exist). The 61 earlier tests still pass.

- [ ] **Step 4: Add the guest section to `EdrTestVm.psm1`**

Insert this block immediately **before** the `Export-ModuleMember` line:

```powershell
# ---------------------------------------------------------------------------------------------------------------------
# Guest: Initialize-EdrTestGuest (runs inside the VM, Windows PowerShell 5.1)
# ---------------------------------------------------------------------------------------------------------------------

function Invoke-EdrBcdedit {
    [CmdletBinding()]
    [OutputType([string[]])]
    param([Parameter(Mandatory)][string[]]$ArgumentList)
    $output = & bcdedit.exe @ArgumentList 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "bcdedit $($ArgumentList -join ' ') failed ($LASTEXITCODE): $output"
    }
    , [string[]]$output
}

function Invoke-EdrWinget {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Id)
    & winget.exe install --exact --id $Id --silent --accept-source-agreements --accept-package-agreements
    # -1978335189 (0x8A15002B): already installed, no applicable upgrade.
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) {
        throw "winget install $Id failed ($LASTEXITCODE)."
    }
}

function ConvertFrom-EdrDbgSetting {
    # Parses `bcdedit /dbgsettings` output ("name   value" lines) into a hashtable.
    [CmdletBinding()]
    [OutputType([hashtable])]
    param([Parameter(Mandatory)][AllowEmptyCollection()][AllowEmptyString()][string[]]$Text)
    $settings = @{}
    foreach ($line in $Text) {
        if ($line -match '^(?<name>[a-z]+)\s+(?<value>\S+)\s*$') {
            $settings[$Matches.name] = $Matches.value
        }
    }
    $settings
}

function Find-EdrGuestAdapter {
    # Returns the guest interface alias whose Hyper-V device name is $HyperVName, or $null.
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory)][string]$HyperVName)
    $match = Get-NetAdapterAdvancedProperty -DisplayName 'Hyper-V Network Adapter Name' -ErrorAction SilentlyContinue |
        Where-Object { $_.DisplayValue -eq $HyperVName } | Select-Object -First 1
    if ($match) { $match.Name } else { $null }
}

function Test-EdrGuestPrecondition {
    # Returns one message per unmet precondition; empty when the guest is ready.
    [CmdletBinding()]
    [OutputType([string[]])]
    param()
    $c = $script:Config
    $problems = @()
    if (-not (Test-EdrElevated)) {
        $problems += 'Not elevated: run from an administrator PowerShell.'
    }
    if (Confirm-SecureBootUEFI) {
        $problems += 'Secure Boot is on: shut down the VM and run Complete-EdrTestVmInstall.ps1 on the host.'
    }
    if ((Get-MpComputerStatus).IsTamperProtected) {
        $problems += 'Tamper Protection is on: turn it off in Windows Security > Virus & threat protection > Manage settings.'
    }
    if (-not (Find-EdrGuestAdapter -HyperVName $c.InternalAdapter)) {
        $problems += "No NIC named '$($c.InternalAdapter)': check the VM's network adapters on the host."
    }
    if (-not (Find-EdrGuestAdapter -HyperVName $c.OnlineAdapter)) {
        $problems += 'Not online: run Set-EdrTestNetwork.ps1 -Mode Online on the host (winget needs internet).'
    }
    if (-not (Get-Command winget.exe -ErrorAction SilentlyContinue)) {
        $problems += 'winget is not available yet: update "App Installer" from the Microsoft Store, then re-run.'
    }
    , $problems
}

function Set-EdrGuestAddress {
    [CmdletBinding(SupportsShouldProcess)]
    param()
    $c = $script:Config
    $alias = Find-EdrGuestAdapter -HyperVName $c.InternalAdapter
    if (Get-NetIPAddress -InterfaceAlias $alias -IPAddress $c.GuestIp -ErrorAction SilentlyContinue) {
        Write-Verbose "Guest address $($c.GuestIp) already on '$alias'."
        return
    }
    if ($PSCmdlet.ShouldProcess($alias, "Static $($c.GuestIp)/$($c.PrefixLength), no gateway")) {
        Set-NetIPInterface -InterfaceAlias $alias -AddressFamily IPv4 -Dhcp Disabled
        New-NetIPAddress -InterfaceAlias $alias -IPAddress $c.GuestIp -PrefixLength $c.PrefixLength | Out-Null
    }
}

function Set-EdrGuestKdnet {
    # Enables KDNET. Keeps an existing matching configuration so the key doesn't change on re-runs.
    # Returns the key.
    [CmdletBinding(SupportsShouldProcess)]
    [OutputType([string])]
    param()
    $c = $script:Config
    $current = ConvertFrom-EdrDbgSetting -Text (Invoke-EdrBcdedit -ArgumentList '/dbgsettings')
    $reuse = $current['debugtype'] -eq 'NET' -and $current['hostip'] -eq $c.HostIp -and
        $current['port'] -eq "$($c.KdnetPort)" -and $current['key']
    if ($reuse) {
        $key = $current['key']
    } elseif ($PSCmdlet.ShouldProcess('boot configuration', "KDNET to $($c.HostIp):$($c.KdnetPort)")) {
        $out = Invoke-EdrBcdedit -ArgumentList '/dbgsettings', 'net', "hostip:$($c.HostIp)", "port:$($c.KdnetPort)"
        $key = ((@($out) -join "`n") | Select-String -Pattern 'Key=(\S+)').Matches[0].Groups[1].Value
    } else {
        return $null
    }
    if ($PSCmdlet.ShouldProcess('boot configuration', 'bcdedit /debug on')) {
        Invoke-EdrBcdedit -ArgumentList '/debug', 'on' | Out-Null
    }
    $key
}

function Write-EdrRegistryDword {
    # Creates the key if needed. No ShouldProcess here: the caller gates it.
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][int]$Value)
    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -Path $Path -Force | Out-Null
    }
    New-ItemProperty -LiteralPath $Path -Name $Name -Value $Value -PropertyType DWord -Force | Out-Null
}

function Initialize-EdrTestGuest {
    [CmdletBinding(SupportsShouldProcess)]
    param()
    $c = $script:Config
    $problems = Test-EdrGuestPrecondition
    if ($problems.Count -gt 0) {
        throw ("The guest is not ready; nothing was changed:`n  " + ($problems -join "`n  "))
    }

    Set-EdrGuestAddress
    if ($PSCmdlet.ShouldProcess('boot configuration', 'bcdedit /set testsigning on')) {
        Invoke-EdrBcdedit -ArgumentList '/set', 'testsigning', 'on' | Out-Null
    }
    $key = Set-EdrGuestKdnet
    $hvci = 'HKLM:\SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity'
    if ($PSCmdlet.ShouldProcess('Memory Integrity (HVCI)', 'Turn off')) {
        Write-EdrRegistryDword -Path $hvci -Name 'Enabled' -Value 0
    }
    if ($PSCmdlet.ShouldProcess('Microsoft Defender', 'Cloud protection, sample submission and real-time protection off')) {
        Set-MpPreference -MAPSReporting Disabled -SubmitSamplesConsent NeverSend -DisableRealtimeMonitoring $true
    }
    foreach ($id in $c.WingetPackages) {
        if ($PSCmdlet.ShouldProcess($id, 'winget install')) {
            Invoke-EdrWinget -Id $id
        }
    }

    $lines = @()
    if ($key) {
        $lines += '', 'KDNET key (save it; the runbook needs it):', "  windbg -k net:port=$($c.KdnetPort),key=$key"
    }
    $lines += '', 'Next: restart this VM, then on the host run', '  Set-EdrTestNetwork.ps1 -Mode Isolated',
    "  Checkpoint-VM -Name $($c.VmName) -SnapshotName $($c.BaselineCheckpoint)"
    foreach ($line in $lines) {
        Write-Information -MessageData $line -InformationAction Continue
    }
}
```

Then replace the `Export-ModuleMember` block with:

```powershell
Export-ModuleMember -Function @(
    'Get-EdrTestVmConfig'
    'New-EdrTestVm'
    'Complete-EdrTestVmInstall'
    'Set-EdrTestNetwork'
    'Copy-ToEdrTestVm'
    'Reset-EdrTestVm'
    'Initialize-EdrTestGuest'
)
```

- [ ] **Step 5: Write `infra/vm/guest/Initialize-EdrTestGuest.ps1`**

```powershell
<#
.SYNOPSIS
One-time guest setup for edr-test: static IP, test signing, KDNET, HVCI and Defender off, WinDbg + Sysinternals.
.NOTES
Runs INSIDE the VM, as administrator, in Online mode, under Windows PowerShell 5.1:
  powershell -ExecutionPolicy Bypass -File C:\atlas\Initialize-EdrTestGuest.ps1
Copy EdrTestVm.psm1 into C:\atlas\ alongside it. Runbook: docs/runbooks/edr-test-vm.md
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSShouldProcess', '', Justification = 'Delegates to the module function, which calls ShouldProcess.')]
[CmdletBinding(SupportsShouldProcess)]
param()

# In the guest the module sits next to this script; in the repo it is one folder up.
$module = @((Join-Path $PSScriptRoot 'EdrTestVm.psm1'), (Join-Path (Split-Path $PSScriptRoot) 'EdrTestVm.psm1')) |
    Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $module) {
    throw "EdrTestVm.psm1 not found next to this script. Copy it into $PSScriptRoot as well."
}
Import-Module $module -Force
Initialize-EdrTestGuest @PSBoundParameters
```

- [ ] **Step 6: Run the tests (PowerShell 7 and 5.1) and the analyzer**

Run the VM tests, then the same suite under Windows PowerShell 5.1, which is what the guest runs:
```powershell
powershell.exe -NoProfile -Command "Import-Module Pester -RequiredVersion 5.9.1; Invoke-Pester infra/vm/tests -Output Detailed"
```
Then run PSScriptAnalyzer.
Expected: `Tests Passed: 77, Failed: 0` under both, and no analyzer output.

- [ ] **Step 7: Commit**

```powershell
git add infra/vm
git commit -m "feat(vm): guest setup (static IP, test signing, KDNET, HVCI/Defender off, tools)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Runbook and documentation

**Files:**
- Create: `docs/runbooks/edr-test-vm.md`
- Modify: `docs/specs/2026-09-25-scaffolding-design.md`: status line, plus a new §7
- Modify: `docs/architecture-overview.md`: roadmap row 0b, decision log

**Interfaces:**
- Consumes: script names and parameters (Tasks 4–6); CI job ids (Tasks 1, 5).

- [ ] **Step 1: Write `docs/runbooks/edr-test-vm.md`**

````markdown
# Runbook: the `edr-test` VM

The Hyper-V VM where the agent, attack simulations and (later) the kernel driver run. **Driver code never runs on the host.**

Design: [0b spec §4](../specs/2026-09-25-scaffolding-design.md). Scripts: `infra/vm/`.

| | |
|---|---|
| VM | `edr-test`: Gen 2, 4 vCPU, 8 GB static RAM, 80 GB dynamic VHDX, vTPM |
| Internal switch | `edr-internal`: host `192.168.77.1/24`, guest `192.168.77.10/24`, always attached |
| Online NIC | `edr-online` on `Default Switch` (NAT), attached only on demand |
| Kernel debugging | KDNET over VMBus to the host, UDP 50000 |
| Guest drop folder | `C:\atlas\` |
| Clean-state checkpoint | `baseline` (taken in Isolated mode) |

In the VM only: Secure Boot, Memory Integrity (HVCI), and Defender real-time and cloud protection are **off**. Never apply these to the host.

All host commands run from the repo root in an **elevated** PowerShell 7 session. Every host script accepts `-WhatIf`.

---

## 1. One-time setup

Prerequisite: Hyper-V is enabled on the host (`Get-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V` shows `Enabled`).

1. **Download the ISO.** Get the Windows 11 Enterprise **Evaluation** ISO (90 days) from the Microsoft Evaluation Center: <https://www.microsoft.com/en-us/evalcenter/download-windows-11-enterprise>. Save it somewhere outside the repo.
2. **Create the VM** (host):
   ```powershell
   .\infra\vm\New-EdrTestVm.ps1 -IsoPath 'D:\ISO\Win11_Enterprise_Eval.iso'
   ```
3. **Install Windows** (manual): `vmconnect localhost edr-test`, start the VM, press a key to boot from the DVD.
   - Choose "I don't have a product key" if asked (the eval needs none).
   - At the Microsoft account screen, pick **Sign-in options → Domain join instead** to create a **local** account.
   - Decline all optional diagnostics and experience settings.
   - Finish setup, sign in once, then **shut down from inside Windows**.
4. **Finish the install** (host, with the VM off). This turns Secure Boot off, ejects the ISO and boots from the disk:
   ```powershell
   .\infra\vm\Complete-EdrTestVmInstall.ps1
   Start-VM edr-test
   ```
5. **Turn off Tamper Protection** (guest, manual): Windows Security → Virus & threat protection → Manage settings → Tamper Protection **Off**. This is the one setting a script cannot change.
6. **Go online** (host):
   ```powershell
   .\infra\vm\Set-EdrTestNetwork.ps1 -Mode Online
   ```
   If `winget` isn't available in the guest yet, open Microsoft Store in the guest and update "App Installer".
7. **Copy the guest script in** (host):
   ```powershell
   .\infra\vm\Copy-ToEdrTestVm.ps1 -Path .\infra\vm\EdrTestVm.psm1, .\infra\vm\guest\Initialize-EdrTestGuest.ps1
   ```
8. **Initialize the guest** (guest, **Administrator** Windows PowerShell):
   ```powershell
   powershell -ExecutionPolicy Bypass -File C:\atlas\Initialize-EdrTestGuest.ps1
   ```
   It checks its preconditions first, lists everything unmet, and changes nothing until all are met.
   **Save the printed `windbg -k net:...` line** in your password manager; it holds the KDNET key. The script is safe to re-run, and a re-run keeps the same key.
9. **Restart the guest** (`Restart-Computer`), then **go isolated** (host):
   ```powershell
   .\infra\vm\Set-EdrTestNetwork.ps1 -Mode Isolated
   ```
10. **Take the baseline** (host):
    ```powershell
    Checkpoint-VM -Name edr-test -SnapshotName baseline
    ```
11. Run the **acceptance checklist** (§6).

## 2. Daily loop

```powershell
.\infra\vm\Reset-EdrTestVm.ps1                                   # restore baseline and start
.\infra\vm\Copy-ToEdrTestVm.ps1 -Path .\target\release\atlas-agent.exe
# ... test inside the VM ...
.\infra\vm\Reset-EdrTestVm.ps1                                   # throw the state away
```

Attack simulations (e.g. Atomic Red Team) run **only in Isolated mode**. If a test needs the internet, go Online, download what you need, go Isolated again, then run the test.

## 3. Kernel debugging (KDNET)

1. On the host: start WinDbg with the saved line (`windbg -k net:port=50000,key=<key>`) and wait for "Waiting to reconnect...".
2. Restart the guest. WinDbg connects during boot; press **Break** (Ctrl+Break) to stop in the debugger.
3. The first time, allow WinDbg through the host firewall if prompted. The `Atlas-EdrTest-KDNET` rule already opens UDP 50000 on `vEthernet (edr-internal)` only.

Debug in **Isolated** mode. KDNET binds to a NIC at boot. With only `edr-internal` present the binding is unambiguous. Acceptance item 6 records whether it also works after a reboot in Online mode.

Enhanced session can time out while the guest is stopped at a breakpoint; use a basic session (View → Enhanced session off) while debugging.

## 4. The 90-day rebuild

The evaluation expires after 90 days. Rebuilding also tests that the scripts still work.

```powershell
Stop-VM edr-test -TurnOff
$vhd = (Get-VMHardDiskDrive -VMName edr-test).Path
Remove-VM edr-test -Force
Remove-Item $vhd
```

Then repeat §1 from step 2. The switch, host address and firewall rule are reused. `New-EdrTestVm` refuses to run while an old `edr-test.vhdx` is still present.

## 5. Branch protection (one-time, user action)

This makes the CI jobs required on `main`. Run it from a PowerShell prompt that is signed in to `gh` as the repo owner:

```powershell
@'
{
  "required_status_checks": {
    "strict": true,
    "checks": [
      { "context": "rust-linux" },
      { "context": "rust-windows" },
      { "context": "proto" },
      { "context": "powershell" },
      { "context": "audit" }
    ]
  },
  "enforce_admins": false,
  "required_pull_request_reviews": null,
  "restrictions": null
}
'@ | gh api -X PUT repos/jakefrenzel/atlas-edr/branches/main/protection --input -
```

Check it with: `gh api repos/jakefrenzel/atlas-edr/branches/main/protection --jq '.required_status_checks.checks[].context'`.

## 6. Acceptance checklist

Run after every build or rebuild. Record the date and results in the table below.

| # | Check | How | Pass when |
|---|---|---|---|
| 1 | Boots | Start the VM | Reaches the desktop |
| 2 | Boot flags | Guest (admin): `bcdedit` | `testsigning Yes` and `debug Yes` under `{current}` |
| 3 | KDNET | §3, in Isolated mode | WinDbg connects during boot and breaks in |
| 4 | Isolated | Guest: `ping -n 2 1.1.1.1`; `ping -n 2 192.168.77.1` | First fails; second succeeds |
| 5 | Online | `Set-EdrTestNetwork.ps1 -Mode Online`; guest: `curl.exe -sI https://www.microsoft.com` | HTTP response headers print |
| 6 | KDNET while Online | Still Online: restart the guest with WinDbg waiting | Record the result; either result passes. If it fails, debug only in Isolated mode (§3) |
| 7 | Copy while isolated | `Set-EdrTestNetwork.ps1 -Mode Isolated`; `Copy-ToEdrTestVm.ps1 -Path .\README.md` | `C:\atlas\README.md` exists in the guest |
| 8 | Reset | Guest: `New-Item C:\atlas\canary.txt`; host: `Reset-EdrTestVm.ps1` | After restore, `C:\atlas\canary.txt` is gone |

### Results

| Date | Build | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | Notes |
|---|---|---|---|---|---|---|---|---|---|---|
| | | | | | | | | | | |
````

- [ ] **Step 2: Check that the runbook matches the code**

Run:
```powershell
Select-String -Path docs/runbooks/edr-test-vm.md -Pattern '\.\\infra\\vm\\([A-Za-z-]+)\.ps1' -AllMatches |
  ForEach-Object { $_.Matches.Groups[1].Value } | Sort-Object -Unique |
  ForEach-Object { if (-not (Test-Path "infra/vm/$_.ps1")) { "MISSING: $_" } }
```
Expected: no output. Every script the runbook names exists, and `Initialize-EdrTestGuest` is only referenced by its guest path `C:\atlas\...`.

- [ ] **Step 3: Update the spec**

In `docs/specs/2026-09-25-scaffolding-design.md`, change the status line to:

```markdown
**Status:** Approved (2026-09-25); implementation plan: [2026-09-25-scaffolding-plan](../plans/2026-09-25-scaffolding-plan.md). Brainstorm handoff: [scaffolding-brainstorm-notes](2026-09-25-scaffolding-brainstorm-notes.md).
```

Append a new section at the end of the spec:

```markdown
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
```

- [ ] **Step 4: Update `docs/architecture-overview.md`**

Replace the 0b roadmap row's status cell with:

```markdown
| In build ([spec](specs/2026-09-25-scaffolding-design.md), [plan](plans/2026-09-25-scaffolding-plan.md), [runbook](runbooks/edr-test-vm.md)) |
```

Append to the Decision Log table:

```markdown
| 2026-09-25 | 0b plan: one `EdrTestVm` PowerShell module holds all VM logic; scripts are thin wrappers; every external command is stubbed in tests so no test can reach the real host; the module stays Windows PowerShell 5.1 compatible and ASCII-only for the guest. |
```

- [ ] **Step 5: Commit**

```powershell
git add docs
git commit -m "docs(0b): edr-test VM runbook; record planning clarifications

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push
```

---

### Task 8: Merge, first fuzz run, VM acceptance (with the user)

This task finishes the spec's §5.4 definition of done. Items marked **(user)** need the user; ask, don't do.

- [ ] **Step 1: Open the PR and confirm CI is green**

```powershell
gh pr create --base main --head feat-0b-scaffolding --title "0b: CI + Hyper-V test VM scaffolding" --body-file -
```
The body should:
- summarize the tasks and link the spec, plan and runbook;
- say that the VM itself is built by the user from the runbook;
- end with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

Then run `gh pr checks --watch`. Expected: all `ci` jobs pass.

- [ ] **Step 2: (user) Merge**

Ask the user to review and merge, or to authorize merging. Use superpowers:finishing-a-development-branch.

- [ ] **Step 3: First 10-minute fuzz run on `main`**

```powershell
gh workflow run fuzz.yml --ref main
Start-Sleep 15
gh run watch (gh run list --workflow fuzz --branch main --limit 1 --json databaseId --jq '.[0].databaseId') --exit-status
```
Expected: `decode_event` runs about 10 minutes and succeeds.
- **On a crash:** download the artifact (`gh run download <id> -n fuzz-artifacts-decode_event`) and hand it to the user. This is a 0a bug and needs its own fix cycle, starting with superpowers:systematic-debugging.
- The DoD item is the first **scheduled** nightly run. Check it the next day with `gh run list --workflow fuzz --event schedule --limit 1`.

- [ ] **Step 4: (user) Build the VM and run the acceptance checklist**

The user follows `docs/runbooks/edr-test-vm.md` §1, runs §6, and fills in the results table. Fix any step that doesn't match reality in the runbook, and in the scripts too if needed (TDD applies). Record the result of acceptance item 6 (KDNET while Online) in the runbook.

- [ ] **Step 5: (user) Branch protection**

Point the user to runbook §5. Claude does not change repo settings.

- [ ] **Step 6: Close out 0b**

Once Steps 3 (scheduled run) and 4 pass, on a new branch, update `docs/architecture-overview.md`:
- **Roadmap:**
  - 0a's status becomes `Done`, with the "(10-min fuzz run pending in 0b CI)" note removed.
  - 0b's status becomes `Done ([spec](...), [runbook](...))`.
- **Decision Log:** add `| <date> | 0b done: CI green on main; first nightly 10-min fuzz run of decode_event clean (closes 0a DoD §8.2.2); edr-test built from the runbook and the acceptance checklist passed (KDNET-while-Online: <result>). |`

Then commit, open a PR and merge it the same way.
