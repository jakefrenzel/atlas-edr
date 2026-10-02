# Sub-project 1a — ETW Sensor Spikes Implementation Plan

> **Status:** Approved 2026-10-02. **For agentic workers:** steps use checkbox (`- [ ]`) syntax for tracking. Every `probe trace` step needs an **elevated** PowerShell 7 on the host (or an Administrator Windows PowerShell in the VM); see decision D2 for who runs them.

**Goal:** Answer the ten verification questions of spec §15.2 (S1–S10) with measurements on this host and the `edr-test` VM, write the answers into spec §15.3, record every fallback taken in the decision log, and leave plan 1b with no open questions.

**Architecture:**
- **One throwaway tool, `probe`** (Rust, `windows` crate), in the git-ignored `spikes/probe/`. Its full source is Appendix A. It is not in the workspace and is never committed.
  - `probe trace` runs the spec's sessions: Session A (manifest providers, QPC clock, 250 ms flush, start-key enable property, event-ID allow-lists) and optionally Session B (system logger, `EVENT_TRACE_FLAG_PROCESS`). It writes either TDH-decoded JSON lines filtered to one process tree, or a stats document (rates per event, the probe's own CPU, session loss).
  - `probe act <scenario>` performs scripted file, registry and DNS operations. Each step prints one JSON line with QPC timestamps before and after it.
  - `probe report` lines events up with the step that caused them and prints one Markdown section per step. Most questions are then answered by reading a table.
  - `probe telemetry`, `boot`, `spawn`, `tcp` and `udp` cover S1, S2, S8, S5 and S9.
- **Raw output stays local** in `spikes/results/` (git-ignored). Only conclusions reach the repo: spec §15.3, the decision log and the roadmap.

**Tech stack:** Rust 1.97 (edition 2024), `windows` 0.62, `clap` 4.6, `serde_json` 1, `sha2` 0.10. PowerShell 7 on the host and Windows PowerShell 5.1 in the VM. Hyper-V cmdlets. `logman` to check and clean up sessions.

**Spec:** `docs/specs/2026-10-01-etw-sensor-design.md` (rev 2, approved). Section numbers below (§) refer to it.

**Verification note (2026-10-02, host build 26200, unelevated):**
- The probe builds (debug and release), `cargo clippy --all-targets` is clean, and its 5 unit tests pass. They include an Authenticode check: `notepad.exe` is valid, the unsigned probe is reported as `unsigned`.
- These ran successfully: `boot`, `telemetry`, `act file`, `act registry` (the 3 HKLM steps need elevation), `act dns`, `spawn`, `tcp` (loopback: 12,345 bytes and 7,777 bytes in each direction) and `udp`.
- An end-to-end `trace --provider dns-client --out … -- probe act dns` plus `report` worked: 66 of 102 events kept for the action's process tree, every field decoded.
- A failed `EnableTraceEx2` used to leave the session running. Fixed: `Session` now stops itself on drop.
- Provider GUIDs and keyword masks below were read from `logman query providers` on this host.
- **Not run yet (needs elevation):** the kernel providers, Session B, `--capture-state` and `--enrich`. Task 1 step 5 is their first run.
- **Already visible, not yet conclusions:**
  - `telemetry` showed `ProcessStartKey == (BootId << 48) | ProcessSequenceNumber` for every process readable unelevated (S1).
  - The DNS dry run showed event 3008 carrying the requester's header PID and start key, on cache hits too; events 3009–3020 come from the DNS Client service with `ClientPID` (S3).
  - The boot-time candidates already disagree: the SystemTimeOfDay boot time, now minus interrupt time, and the ETW log-file header's `BootTime` differ by up to ~25 s (S2).

  Each task re-runs these properly, on the right machine and elevated.

## Global Constraints

- **Branches.** This plan is reviewed on `docs/1a-spikes-plan`. The spikes run on a new branch, `docs/1a-spike-results`, created from `main` after this plan merges. Only `.gitignore`, the spec, the architecture overview and (optionally) the runbook change on it. Never commit to `main`.
- **`spikes/` is git-ignored** and holds `probe/`, `env.ps1` and `results/`. Never `git add -f` anything under it.
- **Privacy.** The repo is public. Results files contain usernames, paths, hostnames, IP addresses and DNS names from the host. §15.3 gets **findings only**: field names, versions, booleans, counts, rates, CPU figures and test-key names. Never a host path under `\Users\<name>`, a SID, an IP address other than the `192.168.77.x` test addresses, or a DNS name other than the test names (`example.com`, `www.microsoft.com`, `*.invalid`). `--all` is used only in the steps that say so.
- **Session hygiene.** Probe sessions are named `Atlas-Spike-A` / `Atlas-Spike-B`, never the agent's `Atlas-Sensor` / `Atlas-Process`. If a probe is killed, run `logman stop Atlas-Spike-A -ets; logman stop Atlas-Spike-B -ets`. Every task ends with `logman query -ets | Select-String Atlas`, which must print nothing.
- **Host changes are temporary.** Clock and time-zone changes happen **only in the VM**. Registry test keys (`HKCU\Software\AtlasSpike`, `HKLM\SOFTWARE\AtlasSpike[32]`, `HKLM\SYSTEM\CurrentControlSet\Control\AtlasSpike`) are deleted by the scenario itself. Each task's last step checks they are gone. File scenarios work under `C:\AtlasSpike\`, which Task 11 deletes.
- **VM console:** give single-line commands; pasting multi-line blocks into the VM console joins the lines. Copy files in with `.\infra\vm\Copy-ToEdrTestVm.ps1` (host, elevated). Reset with `.\infra\vm\Reset-EdrTestVm.ps1` when a task is done with the VM.
- **No kernel code** is involved anywhere in this plan; everything is user-mode ETW consumption (spec §1).
- Every commit message ends with:
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  ```

## Where each spike runs

| Spike | Machine | Why |
|---|---|---|
| S1 | Host, then VM across a reboot | Needs a second boot (BootId change); rebooting the daily machine just for this is avoidable |
| S2 | VM | Clock and time-zone changes, save/restore, reboot |
| S3, S4, S6, S7, S8 | Host | Field behaviour on the build the agent will run on (26200); S8's canary cost needs Defender on |
| S5 | Host ↔ VM | A real NIC path with exact byte counts, plus loopback |
| S9, S10 | Host | §13 budgets are measured on the host |

## Decisions (approved 2026-10-02)

The user approved the recommended option for each (D1 A, D2 A, D3 as proposed, D4 A). The alternatives are kept for the record.

- **D1, tooling: A.**
  - **(A) Recommended:** the Rust probe above. It is the only route to the start-key extended data (S1's third source) and to CPU figures for our own consumer design (S9/S10). It also rehearses the `windows`-crate ETW code that plan 1b writes for real.
  - (B) Built-in tools only (`logman`, `tracerpt`, `Get-WinEvent` on `.etl`). This is cheaper, but it cannot set `EVENT_ENABLE_PROPERTY_PROCESS_START_KEY` or event-ID filters, so S1's third source and S9/S10's numbers would be missing.
- **D2, who runs elevated steps: A.**
  - **(A) Recommended:** you run each step's command block in an elevated `pwsh`; output lands in `spikes/results/`; I read it, analyze and draft §15.3. This keeps the agent unelevated on the daily machine (goal 1).
  - (B) Start Claude Code elevated and let me run the steps. Faster, but it gives the agent administrator rights on the host for the whole session.
- **D3, thresholds the spec leaves open: as proposed** (each recorded in the decision log when applied):
  - S9 gate: UDP stays on by default if the probe's CPU with `--model` and UDP enabled is **≤ 2.5% of one core** (half the §13 heavy-load budget, because the real pipeline does more per event than the model) **and** zero events are lost.
  - S8 process canary is "negligible" if a launch per minute costs **≤ 0.1% of one core** on average (child + spawner + Defender CPU per launch ≤ 60 ms).
  - S10 enrichment misses its deadline "often" if **> 5%** of first launches go over 1 s. The Launch deadline is then raised to the measured p99, rounded up to 250 ms.
  - S10 kernel-side cost is reported, not gated. If a clean `cargo build` is **> 5%** slower with Session A running, it goes to you as a decision before plan 1b.
- **D4, probe source: A.**
  - **(A) Recommended:** keep `spikes/` git-ignored as decided. The source is reproducible from Appendix A.
  - (B) Commit `spikes/probe/` for plan 1b to read.

## Results format

Each spike's results go into spec §15.3 as one row and, where needed, a short paragraph:

| # | Answer | Evidence (machine, build, counts) | Design consequence |
|---|---|---|---|

"Design consequence" names the spec row it settles, e.g. "§5.2 row 2: start key = f(BootId, seq)", or the fallback taken with its decision-log date. Raw evidence stays in `spikes/results/<spike>/`.

---

### Task 1: Set up the probe and environment

**Files:**
- Modify: `.gitignore`
- Create (ignored): `spikes/probe/**` from Appendix A, `spikes/env.ps1`, `spikes/results/`

- [ ] **Step 1: Branch and ignore rule**

```powershell
git switch main; git pull; git switch -c docs/1a-spike-results
Add-Content .gitignore '/spikes/'
git add .gitignore
git commit -m "chore: ignore spikes/ (plan 1a throwaway code and results)`n`nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 2: Create the probe**

Create every file of Appendix A under `spikes/probe/` exactly as listed (`Cargo.toml`, `src/*.rs`). The `[workspace]` table in its `Cargo.toml` is what keeps Cargo from treating it as part of the repo workspace.

- [ ] **Step 3: Build and test (unelevated)**

```powershell
cargo build --release --manifest-path spikes/probe/Cargo.toml
cargo test --release --manifest-path spikes/probe/Cargo.toml
git status --short
```
Expected: the build succeeds, `test result: ok. 5 passed`, and `git status` shows nothing under `spikes/`.

- [ ] **Step 4: Write `spikes/env.ps1`**

Dot-source it (`. .\spikes\env.ps1`) at the start of every elevated session, from the repo root. The keyword masks are the spec §4.2 sets, using the values `logman query providers` printed on 2026-10-02.

```powershell
# Plan 1a spike environment. Dot-source from the repo root.
$Probe = (Resolve-Path .\spikes\probe\target\release\probe.exe).Path
$R = (New-Item -ItemType Directory -Force .\spikes\results).FullName
# Spec section 4.2 provider sets: NAME:KEYWORDS:EVENT_IDS
$KP     = 'kernel-process:0x50:1,2,5'                           # PROCESS 0x10 | IMAGE 0x40
$KF     = 'kernel-file:0x1EA0:12,13,14,16,17,26,27,30'          # FILEIO 0x20 | CREATE 0x80 | WRITE 0x200 | DELETE_PATH 0x400 | RENAME_SETLINK_PATH 0x800 | CREATE_NEW_FILE 0x1000
$KR     = 'kernel-registry:0x5300:1,3,5,6'                      # SetValueKey 0x100 | DeleteValueKey 0x200 | CreateKey 0x1000 | DeleteKey 0x4000
$KN_TCP = 'kernel-network:0x30:12,13,15,28,29,31'               # IPV4 0x10 | IPV6 0x20
$KN_UDP = 'kernel-network:0x30:12,13,15,28,29,31,42,43,58,59'
$DNS    = 'dns-client::3008'                                    # keyword: all, until S3 picks the minimal one
function Assert-NoSpikeSession { if (logman query -ets | Select-String 'Atlas-Spike') { throw 'A spike session is still running: logman stop Atlas-Spike-A -ets' } }
"probe: $Probe"; "results: $R"
```

- [ ] **Step 5: Elevated smoke test (first run of the kernel paths)**

In an **elevated** `pwsh` at the repo root:
```powershell
. .\spikes\env.ps1
& $Probe trace --provider $KP --process-logger --out $R\smoke.jsonl -- $Probe spawn --count 3
& $Probe report --events $R\smoke.jsonl --actions $R\smoke.actions.jsonl | Select-Object -First 5
# The probe writes JSON keys in alphabetical order, so match on parsed fields, not on text.
$ev = Get-Content $R\smoke.jsonl | ConvertFrom-Json
@($ev | Where-Object { $_.provider -eq 'kernel-process' -and $_.id -eq 1 }).Count
@($ev | Where-Object session -eq 'B').Count
Assert-NoSpikeSession
```
Expected: `wrote N of M events`; at least **3** Kernel-Process ProcessStart events (the spawned children); at least **3** Session B events; no `decode_error` on these; no leftover session.
If `StartTraceW` fails with error 5, the shell is not elevated. If Session B fails with "no more system logger slots" (error 1450 / `ERROR_NO_SYSTEM_RESOURCES`), list sessions with `logman query -ets`, report it, and stop.

---

### Task 2: S1 — start key vs `ProcessSequenceNumber`

**Question:** is the start key `ProcessSequenceNumber`, or f(`BootId`, sequence number)? Three sources are compared: `PROCESS_TELEMETRY_ID_INFORMATION.ProcessStartKey`, the extended-data start key on events, and ProcessStart's `ProcessSequenceNumber`.

- [ ] **Step 1: Telemetry for every process (host, elevated)**

```powershell
. .\spikes\env.ps1
New-Item -ItemType Directory -Force $R\s1 | Out-Null
& $Probe telemetry > $R\s1\telemetry-host.jsonl
$t = Get-Content $R\s1\telemetry-host.jsonl | ConvertFrom-Json | Where-Object { -not $_.error }
$t | Group-Object key_eq_bootid_shl48_or_seq, key_eq_seq | Select-Object Count, Name
($t | Select-Object -ExpandProperty boot_id -Unique)
```
Record: how many processes were read and how many failed, the count in each group, and the distinct BootIds (expected: exactly one, equal to `probe boot`'s `registry_prefetch`).

- [ ] **Step 2: ETW sequence number vs extended-data start key (host, elevated)**

```powershell
& $Probe trace --provider $KP --out $R\s1\etw-host.jsonl -- $Probe spawn --count 20
$ev = Get-Content $R\s1\etw-host.jsonl | ConvertFrom-Json
$bootId = [uint64]($t[0].boot_id)
$ev | Where-Object { $_.provider -eq 'kernel-process' -and $_.id -eq 1 } | ForEach-Object {
  $child = [uint32]$_.fields.ProcessID; $seq = [uint64]$_.fields.ProcessSequenceNumber
  $keys = @($ev | Where-Object { $_.pid -eq $child -and $_.start_key } | Select-Object -ExpandProperty start_key -Unique)
  [pscustomobject]@{ pid = $child; seq = ('0x{0:x16}' -f $seq); f = ('0x{0:x16}' -f (($bootId -shl 48) -bor $seq)); keys = ($keys -join ' '); match = ($keys.Count -eq 1 -and $keys[0] -eq ('0x{0:x16}' -f (($bootId -shl 48) -bor $seq))) }
} | Format-Table -AutoSize
```
The child's own events (ImageLoad, ProcessStop) carry its start key as extended data. Record: number of children, number with `match = True`, and the ProcessStart event version (from `$ev[...].version`; v3+ expected).
If `ProcessSequenceNumber` is formatted as hex by TDH (`0x…`), use `[uint64]::Parse($_.Substring(2), 'HexNumber')` instead of the cast.

- [ ] **Step 3: Across a reboot (VM)**

Host, elevated:
```powershell
.\infra\vm\Reset-EdrTestVm.ps1
.\infra\vm\Copy-ToEdrTestVm.ps1 -Path .\spikes\probe\target\release\probe.exe
```
VM, Administrator Windows PowerShell, one line at a time:
```powershell
C:\atlas\probe.exe telemetry > C:\atlas\s1-boot1.jsonl; C:\atlas\probe.exe boot > C:\atlas\s1-boot1-boot.json
Restart-Computer
```
After the restart:
```powershell
C:\atlas\probe.exe telemetry > C:\atlas\s1-boot2.jsonl; C:\atlas\probe.exe boot > C:\atlas\s1-boot2-boot.json
Get-Content C:\atlas\s1-boot1.jsonl, C:\atlas\s1-boot2.jsonl | ConvertFrom-Json | Where-Object { -not $_.error } | Group-Object boot_id, key_eq_bootid_shl48_or_seq | Select-Object Count, Name
```
Bring the four files back to `spikes\results\s1\`. `Copy-VMFile` only copies host → guest, so use the enhanced session's clipboard: select the files in the VM's Explorer, Ctrl+C, then paste on the host. The same applies to every "bring the files back" step below. Record: BootId before and after (expected +1), and whether the formula held in both boots. Then reset the VM (`.\infra\vm\Reset-EdrTestVm.ps1`).

- [ ] **Step 4: Decide**

| Result | Spec consequence |
|---|---|
| Step 1–3 all `key_eq_bootid_shl48_or_seq` and extended data = f | §5.2 row 2: Launch start key = `(BootId << 48) \| ProcessSequenceNumber`, BootId from the same source as `device.boot_id`. Note the 16-bit BootId field in §15.3. |
| `key_eq_seq` everywhere | §5.2 row 1. |
| Neither, or any mismatch | §5.2 row 3: all paths use the 0a fallback formula; decision-log entry. |

Assert-NoSpikeSession.

---

### Task 3: S2 — a stable `boot_time` source (VM)

**Candidates** printed by `probe boot`:
- B1: `SystemTimeOfDayInformation.BootTime`, with its `BootTimeBias` and `SleepTimeBias`.
- B2: now − interrupt time.
- B2u: now − unbiased interrupt time.
- B3: creation time of System.
- B4: creation times of the Registry process and `smss.exe`.

Also these, read by hand:
- B5: Kernel-General event 12 `StartTime` in the System log.
- B6: WMI `LastBootUpTime`.
- B7: the ETW log-file header `BootTime` from any `probe trace --stats`.

**Pass** = identical to the 100 ns across every same-boot sample (time changes, save/restore, process restarts), different after a reboot, and readable by the SYSTEM service without the event log (which an attacker can clear).

- [ ] **Step 1: Find KUSER_SHARED_DATA's BootId offset (host, unelevated is fine)**

```powershell
windbg -c "dt ntdll!_KUSER_SHARED_DATA 0x7ffe0000 BootId; q" notepad.exe
```
Record the offset printed for `BootId` (e.g. `+0x2c4`). Check it on the host: `& $Probe boot --kusd-offset <offset>` must print the same value for `kusd_<offset>` as `registry_prefetch` and `telemetry_self`.

- [ ] **Step 2: Freeze the VM clock's outside influences (host, elevated)**

```powershell
.\infra\vm\Reset-EdrTestVm.ps1
Disable-VMIntegrationService -VMName edr-test -Name 'Time Synchronization'
.\infra\vm\Copy-ToEdrTestVm.ps1 -Path .\spikes\probe\target\release\probe.exe
```
VM (Administrator), one line at a time:
```powershell
Stop-Service w32time; Set-Service w32time -StartupType Disabled
```

- [ ] **Step 3: Same-boot samples (VM)**

Replace `<off>` with the Step 1 offset. One line each, in order:
```powershell
C:\atlas\probe.exe boot --kusd-offset <off> --repeat 3 > C:\atlas\s2-0-baseline.jsonl
Set-Date (Get-Date).AddHours(2); C:\atlas\probe.exe boot --kusd-offset <off> > C:\atlas\s2-1-plus2h.jsonl
Set-Date (Get-Date).AddDays(-1); C:\atlas\probe.exe boot --kusd-offset <off> > C:\atlas\s2-2-minus1d.jsonl
Set-TimeZone -Id 'Tokyo Standard Time'; C:\atlas\probe.exe boot --kusd-offset <off> > C:\atlas\s2-3-tz.jsonl
Get-CimInstance Win32_OperatingSystem | Select-Object LastBootUpTime > C:\atlas\s2-b6.txt; Get-WinEvent -FilterHashtable @{LogName='System'; ProviderName='Microsoft-Windows-Kernel-General'; Id=12} -MaxEvents 1 | ForEach-Object { $_.ToXml() } > C:\atlas\s2-b5.xml
```
Host (elevated) — simulated sleep:
```powershell
Save-VM edr-test; Start-VM edr-test
```
VM, after it resumes:
```powershell
C:\atlas\probe.exe boot --kusd-offset <off> > C:\atlas\s2-4-after-save-restore.jsonl
```

- [ ] **Step 4: Next boot (VM)**

```powershell
Restart-Computer
```
After the restart:
```powershell
C:\atlas\probe.exe boot --kusd-offset <off> > C:\atlas\s2-5-reboot.jsonl
```

- [ ] **Step 5: Restore and compare**

Bring the `s2-*` files to `spikes\results\s2\`. Host (elevated):
```powershell
Enable-VMIntegrationService -VMName edr-test -Name 'Time Synchronization'
.\infra\vm\Reset-EdrTestVm.ps1
```
Build a table: candidate × sample (0–5), marking each cell "=" (same as baseline to 100 ns), "Δ <amount>", or "error". Choose the source that passes. If several pass, prefer one call over a process lookup: B1 before B3/B4. If B5 is the only one that passes, stop and bring it to the user: the event log can be cleared, so it needs a decision. The BootId offset from Step 1 also goes into §15.3, because 0a §4.4 names `KUSER_SHARED_DATA.BootId` as the source.

---

### Task 4: S8 — classic Process Start / DCStart, the Launch join, canary cost (host)

- [ ] **Step 1: Fields, version, layout and join (elevated)**

```powershell
. .\spikes\env.ps1
New-Item -ItemType Directory -Force $R\s8 | Out-Null
& $Probe trace --provider $KP --process-logger --raw --out $R\s8\join.jsonl -- $Probe spawn --count 200 --parallel 8
& $Probe trace --provider $KP --process-logger --raw --out $R\s8\wow64.jsonl -- $Probe spawn --count 5 --exe C:\Windows\SysWOW64\cmd.exe -- /c exit
```
Analysis (pwsh):
```powershell
$ev = Get-Content $R\s8\join.jsonl | ConvertFrom-Json
$classic  = $ev | Where-Object { $_.session -eq 'B' -and $_.opcode -eq 1 }
$manifest = $ev | Where-Object { $_.provider -eq 'kernel-process' -and $_.id -eq 1 }
$freq = 10000000  # replace with qpc_freq from any --stats run if it differs
$rows = foreach ($m in $manifest) {
  $pid_ = [uint32]$m.fields.ProcessID
  $cands = @($classic | Where-Object { [uint32]$_.fields.ProcessId -eq $pid_ -and [math]::Abs($_.ts - $m.ts) -le 0.2 * $freq })
  [pscustomobject]@{ pid = $pid_; candidates = $cands.Count; delta_ms = if ($cands) { ($cands[0].ts - $m.ts) * 1000 / $freq } }
}
$rows | Group-Object candidates | Select-Object Count, Name
$rows.delta_ms | Measure-Object -Minimum -Maximum -Average
$classic[0].version; $classic[0].fields | Format-List
```
Record:
- Classic event version and field names.
- How `UserSID` decodes (TDH string), and its raw bytes. Find the `TOKEN_USER` header and the SID inside the `raw` hex, and note the offsets.
- `CommandLine` present?
- Join: count with exactly 1 candidate (must be all of them), any with 0 or > 1, and the timestamp delta range.
- The same for `wow64.jsonl`: is the layout unchanged for 32-bit processes? (Expected: yes, because the kernel logs with its own pointer size.)

- [ ] **Step 2: Rundown at session start (elevated)**

```powershell
& $Probe trace --process-logger --keep process-classic:0 --out $R\s8\rundown.jsonl --seconds 5 -- $Probe noop
$rd = Get-Content $R\s8\rundown.jsonl | ConvertFrom-Json | Where-Object { $_.opcode -eq 3 }
"$($rd.Count) DCStart vs $((Get-Process).Count) processes"; ($rd | Where-Object { $_.fields.CommandLine }).Count
```
Record the DCStart count against the live process count, and how many carry a command line. (This file contains every running process's command line: it never leaves `spikes/results/`.)

- [ ] **Step 3: Canary-launch cost with Defender on (elevated)**

```powershell
$m0 = (Get-Process MsMpEng).TotalProcessorTime.TotalMilliseconds
& $Probe spawn --count 60 --interval-ms 1000 | Select-Object -Last 1 | Tee-Object $R\s8\canary.json
$m1 = (Get-Process MsMpEng).TotalProcessorTime.TotalMilliseconds
"Defender ms per launch: $(($m1 - $m0) / 60)"
```
Per launch: `child_cpu_ms_mean` + `spawner_cpu_ms / 60` + Defender ms. Apply D3 (≤ 60 ms per launch → add a Kernel-Process canary to §9.2; otherwise keep relying on provider checks). Defender's figure also includes unrelated background scanning; if it is noisy, run the block twice and take the lower result.

Assert-NoSpikeSession.

---

### Task 5: S3 — DNS-Client 3008 attribution and minimal keyword (host)

- [ ] **Step 1: Header PID, 64-bit and 32-bit callers (elevated)**

```powershell
. .\spikes\env.ps1
New-Item -ItemType Directory -Force $R\s3 | Out-Null
ipconfig /flushdns
& $Probe trace --provider 'dns-client' --out $R\s3\dns64.jsonl -- $Probe act dns
& $Probe report --events $R\s3\dns64.jsonl --actions $R\s3\dns64.actions.jsonl > $R\s3\dns64.md
& $Probe trace --provider 'dns-client' --out $R\s3\dns32.jsonl -- C:\Windows\SysWOW64\WindowsPowerShell\v1.0\powershell.exe -NoProfile -Command "Resolve-DnsName example.org | Out-Null"
```
Record, from `dns64.md`:
- For every step, does exactly one 3008 appear, with header `pid` = the action's PID and `start_key` = its start key? Include the cached and NXDOMAIN steps.
- `QueryStatus` for NXDOMAIN (expected 9003).
- The `QueryResults` format for the A, AAAA and CNAME-chain steps (exact text, e.g. `ip;ip;`, and how a CNAME appears).
- From `dns32.jsonl`: is 3008 present, attributed to the 32-bit process, and are its `flags` the 32-bit header (`0x0020`)?

- [ ] **Step 2: Minimal keyword (elevated)**

```powershell
foreach ($kw in '0x8000000000000000','0x100','0x4000000000000000','0x10000000') {
  & $Probe trace --provider "dns-client:$($kw):3008" --out "$R\s3\kw-$kw.jsonl" -- $Probe act dns
  "$kw : $((Select-String -Path "$R\s3\kw-$kw.jsonl" -Pattern '"id":3008').Count) x 3008"
}
```
The smallest mask that still yields six 3008 events (one per step) becomes `$DNS`'s keyword in §4.2. If none of the single keywords yields them, try pairs, starting with the channel keyword `0x8000000000000000`.

- [ ] **Step 3: Decide**

If the header PID matched in every case, §5.4 attributes by header PID and the sibling-event fallback is not needed. Otherwise §5.4 joins on `ClientPID` from 3009–3020, and §16's lower-trust note applies; add a decision-log entry.

Assert-NoSpikeSession.

---

### Task 6: S4 + S7 — Kernel-Registry data capture, key paths, rename (host)

- [ ] **Step 1: Discovery run, all keywords and all event IDs (elevated)**

```powershell
. .\spikes\env.ps1
New-Item -ItemType Directory -Force $R\s7 | Out-Null
& $Probe trace --provider 'kernel-registry' --raw --out $R\s7\all.jsonl -- $Probe act registry
& $Probe report --events $R\s7\all.jsonl --actions $R\s7\all.actions.jsonl > $R\s7\all.md
```
Expected: every step `ok` except the three `_FAILS` steps.

- [ ] **Step 2: Read the answers off `all.md`**

Record, per step:
- **S4:** on the `set_*` steps, are `CapturedDataSize` and `CapturedData` non-zero? Are they truncated for `set_binary_8k`, and at what size? Is `PreviousData` filled on `set_sz_overwrite`? Which value types are captured?
- **S7 paths:**
  - On `create_key_absolute_hkcu`, is event 1's `BaseName`/`RelativeName` a full `\REGISTRY\USER\<SID>\…` path, and is `Disposition` new (1) vs opened (2)?
  - On `create_key_relative_nested`, is `RelativeName` relative to the parent handle (`B\C`) with `BaseObject` set?
  - On delete and value steps, is `KeyName` full or relative?
  - On `current_control_set_create_delete`, is the path `ControlSet001` or `CurrentControlSet`?
  - On `wow64_32bit_view`, does the path contain `WOW6432Node`?
- **S7 rename:** which events, if any, appear during `rename_key`? Look for `SetInformationKey` (keyword 0x40). Does any field carry the new name?
- **Failures:** do the `_FAILS` steps produce events, and with which `Status`?

- [ ] **Step 3: Confirm with the spec filter (elevated)**

```powershell
& $Probe trace --provider $KR --out $R\s7\spec.jsonl -- $Probe act registry
& $Probe report --events $R\s7\spec.jsonl --actions $R\s7\spec.actions.jsonl > $R\s7\spec.md
```
Check that the spec's filter keeps everything §5.1 maps: Key Create, Key Delete, Value Set, Value Delete. If S7 showed that names need OpenKey (2) or a rename event, add that ID and keyword to `$KR`, re-run, and note the change for §4.2.

- [ ] **Step 4: Cleanup check**

```powershell
Test-Path HKCU:\Software\AtlasSpike, HKLM:\SOFTWARE\AtlasSpike, HKLM:\SOFTWARE\WOW6432Node\AtlasSpike32, HKLM:\SYSTEM\CurrentControlSet\Control\AtlasSpike
Assert-NoSpikeSession
```
Expected: four `False`.

---

### Task 7: S6 — Kernel-File semantics (host)

- [ ] **Step 1: Check short-name generation on C: (elevated)**

```powershell
fsutil 8dot3name query C:
```
Record the setting. If 8.3 names are disabled for C:, the `short_name_open` step reports so: record "8.3 names off on this volume" and use the VM for that one step (8.3 names are on by default for the system volume of a fresh install; check there with the same command).

- [ ] **Step 2: Discovery run (elevated)**

Keywords `0x1EF0` = the spec set plus `FILENAME` (0x10) and `OP_END` (0x40), with no ID filter:
```powershell
. .\spikes\env.ps1
New-Item -ItemType Directory -Force $R\s6, C:\AtlasSpike | Out-Null
& $Probe trace --provider 'kernel-file:0x1EF0' --raw --out $R\s6\all.jsonl -- $Probe act file --dir C:\AtlasSpike\s6
& $Probe report --events $R\s6\all.jsonl --actions $R\s6\all.actions.jsonl > $R\s6\all.md
```

- [ ] **Step 3: Read the answers off `all.md`**

Record, per step:
- **Failed operations:**
  - `create_new_exists_FAILS` and `create_new_missing_dir_FAILS`: does 30 `CreateNewFile` fire?
  - `delete_sharing_violation_FAILS` and `delete_readonly_FAILS`: does 26 `DeletePath` fire?
  - `rename_onto_existing_FAILS`: does 27 `RenamePath` fire?
  - What `OperationEnd` (24) status follows each, and does its `Irp` match the pre-operation event's?
- **`RenamePath.FilePath`** on `rename`: the old or the new name? And on `rename_replace_existing`?
- **Overwrites:** on `overwrite_create_always`, `overwrite_truncate_existing` and `open_always_*`, does 30 fire? What do 12 `Create`'s `CreateOptions` (disposition in the high byte) and `CreateAttributes` show?
- **Deletes:** on `delete_on_close_flag`, `delete_disposition_info` and `delete_posix_semantics`, does 26 fire, and at which point (the SetInformation call or the close)? Is there a 17 `SetInformation` with which `InfoClass`?
- **Coalescing inputs:** on `write_three_times_one_handle`, record the order and count of 16 `Write`, 13 `Cleanup` and 14 `Close`, and whether they share `FileObject`.
- **Timestomping:** on `set_basic_info_timestomp`, is there a 17 with `InfoClass` = 4 (`FileBasicInformation`)?
- **Names:** on `short_name_open`, is 12's `FileName` the short or the long form? On `alternate_data_stream`, how do `h.txt:secret` and `h.txt::$DATA` appear?
- **Path form:** confirm `\Device\HarddiskVolumeN\…` paths.

- [ ] **Step 4: Handles opened before the session (rundown) (elevated)**

Window 1:
```powershell
& $Probe act file-hold --dir C:\AtlasSpike\s6-hold --delay-ms 20000
```
Note the PID it prints. Within 20 s, in window 2:
```powershell
& $Probe trace --provider 'kernel-file:0x1EF0' --capture-state --all --raw --seconds 30 --out $R\s6\rundown.jsonl -- $Probe noop
```
Then:
```powershell
$ev = Get-Content $R\s6\rundown.jsonl | ConvertFrom-Json
$w = $ev | Where-Object { $_.provider -eq 'kernel-file' -and $_.id -eq 16 -and $_.pid -eq <HOLDER_PID> } | Select-Object -First 1
$fo = $w.fields.FileObject; "FileObject $fo"
$ev | Where-Object { $_.fields.FileObject -eq $fo -or $_.fields.FileKey -eq $w.fields.FileKey } | Select-Object ts, id, @{n='name';e={$_.fields.FileName}} | Format-Table
```
Record: does any event other than the holder's own Write/Cleanup/Close name `held.txt` for that `FileObject` or `FileKey` — for example a `NameCreate` (10) emitted by the capture-state rundown? `--all` is needed here because the rundown is not logged in the holder's context. The file contains every file event on the host for 30 s; delete it after reading (`Remove-Item $R\s6\rundown.jsonl`).

- [ ] **Step 5: Decide**

Map each answer to spec rows:
- §4.2: whether to add 24 and/or 10.
- §5.1: overwrite handling (Create-disposition parsing or not).
- §5.5: how failures are dropped.
- §7.1: the rundown gap closed or documented.
- §7.2 / §16: 8.3 and ADS handling.

Each fallback taken (OperationEnd join, disposition parsing) gets a decision-log entry. Finish with `Assert-NoSpikeSession`.

---

### Task 8: S5 — TCP Disconnect `size` (host ↔ VM, and loopback)

- [ ] **Step 1: VM listener**

Host (elevated):
```powershell
.\infra\vm\Reset-EdrTestVm.ps1
.\infra\vm\Copy-ToEdrTestVm.ps1 -Path .\spikes\probe\target\release\probe.exe
```
VM (Administrator), one line each:
```powershell
New-NetFirewallRule -DisplayName AtlasSpikeTcp -Direction Inbound -Protocol TCP -LocalPort 5599 -Action Allow
C:\atlas\probe.exe tcp --listen 192.168.77.10:5599 --send 3000000
```

- [ ] **Step 2: Host client under trace (elevated)**

```powershell
. .\spikes\env.ps1
New-Item -ItemType Directory -Force $R\s5 | Out-Null
& $Probe trace --provider $KN_TCP --raw --out $R\s5\vm.jsonl -- $Probe tcp --connect 192.168.77.10:5599 --send 1000000
Get-Content $R\s5\vm.jsonl | ConvertFrom-Json | Select-Object id, version, @{n='fields';e={$_.fields | ConvertTo-Json -Compress}} | Format-Table -Wrap
```
Compare the Disconnect (13 / 29) `size` with 1,000,000 sent and 3,000,000 received. Also note the connect event's `size` and the decoded ports and addresses (`dport` 5599?). Keep the raw hex of one connect event; it pins the byte order for plan 1b's parser.

- [ ] **Step 3: Loopback (elevated, two windows)**

Window 1: `& $Probe tcp --listen 127.0.0.1:5599 --send 3000000`. Window 2:
```powershell
& $Probe trace --provider $KN_TCP --raw --out $R\s5\loop.jsonl -- $Probe tcp --connect 127.0.0.1:5599 --send 1000000
```
Record whether loopback produces the same events (and `size`).

- [ ] **Step 4: Decide and clean up**

If `size` equals a byte total (which direction?), §7.3 fills `bytes_in`/`bytes_out` accordingly. Otherwise they stay absent (the spec's fallback, no decision needed beyond §15.3). Delete the VM rule (`Remove-NetFirewallRule -DisplayName AtlasSpikeTcp`), reset the VM, and run `Assert-NoSpikeSession`.

---

### Task 9: S9 — the UDP gate under QUIC streaming (host)

Close other heavy applications. Use a Chromium-based browser (QUIC on by default) playing a 4K video, started 1 minute before each run and kept playing throughout. Plug in the laptop, and use the same power mode for every run.

- [ ] **Step 1: Calibration (elevated)**

```powershell
. .\spikes\env.ps1
New-Item -ItemType Directory -Force $R\s9 | Out-Null
& $Probe trace --provider $KN_UDP --no-decode --model --stats $R\s9\calib.json --seconds 70 -- $Probe udp --pps 2000 --seconds 60
```
This gives the per-event cost on a known rate: `probe_cpu_s` / total events.

- [ ] **Step 2: Run A, UDP off (10 min, elevated)**

```powershell
& $Probe trace --provider $KP --provider $KF --provider $KR --provider $KN_TCP --provider $DNS --process-logger --no-decode --model --stats $R\s9\a-udp-off.json --seconds 600
```

- [ ] **Step 3: Run B, UDP on (10 min, elevated)**

```powershell
& $Probe trace --provider $KP --provider $KF --provider $KR --provider $KN_UDP --provider $DNS --process-logger --no-decode --model --stats $R\s9\b-udp-on.json --seconds 600
```

- [ ] **Step 4: Decide**

From each stats file record:
- `probe_cpu_pct_of_one_core`;
- `A.per_s_avg` and `per_s_peak`, plus the UDP rows (42/43/58/59) of `by_event`;
- `sessions.*.events_lost` and `realtime_buffers_lost`;
- the probe's working set (`(Get-Process probe).WorkingSet64` sampled once mid-run).

Apply D3: run B within 2.5% of one core and zero loss → `network.udp = true` stays the default. Otherwise `network.udp = false`, with a decision-log entry (E8 gate). If any run lost events, re-run it with `--max-buffers 512`. Record which buffer setting achieved zero loss: it tunes §4.1.

Assert-NoSpikeSession.

---

### Task 10: S10 — file/TCP volume, CPU, kernel-side cost, enrichment (host)

Use the S9 outcome for the network provider: `$KN_UDP` if UDP stayed on, else `$KN_TCP`. Below, `$KN` is that choice (`$KN = $KN_UDP` or `$KN = $KN_TCP`).

- [ ] **Step 1: Build + browsing, without enrichment (elevated, ~15 min)**

Window 1:
```powershell
. .\spikes\env.ps1; $KN = <choice>
New-Item -ItemType Directory -Force $R\s10 | Out-Null
& $Probe trace --provider $KP --provider $KF --provider $KR --provider $KN --provider $DNS --process-logger --no-decode --model --stats $R\s10\build.json --seconds 900
```
Window 2 (unelevated, repo root), started right after:
```powershell
cargo clean; Measure-Command { cargo build --workspace --release } | Select-Object TotalSeconds
```
After the build, browse normally (several sites, a video) until the trace ends.

- [ ] **Step 2: The same with enrichment (elevated, ~15 min)**

Same as Step 1 with `--enrich --stats $R\s10\build-enrich.json`. The difference in `probe_cpu_s` is the enrichment cost. `enrich.first_launches`, `over_deadline`, `total_ms.p99` and `signatures` answer the miss-rate question.

- [ ] **Step 3: Kernel-side cost A/B (unelevated build, elevated trace)**

Three clean builds without a session, then three with Session A running (`trace … --stats $R\s10\ab-on.json --seconds 1800` in an elevated window, stopped with Ctrl+C after the third build):
```powershell
1..3 | ForEach-Object { cargo clean; (Measure-Command { cargo build --workspace --release }).TotalSeconds }
```
Record the means and the difference in %.

- [ ] **Step 4: Decide**

Record:
- events/s per class: average and peak, from `by_event`;
- `probe_cpu_pct_of_one_core` for both runs;
- loss counters;
- buffer growth: total events per hour × an assumed ~250 B per encoded event. Mark it as an estimate for sub-project 2's ClickHouse sizing.

Then apply, in order:
1. **§13 fallbacks.** If heavy-load CPU is over budget (D3 margin as in S9), apply UDP off (if not already), then File Update and SetAttributes off, each with a decision-log entry.
2. **Enrichment.** If `over_deadline / first_launches > 5%`, raise the Launch completion deadline to the p99 rounded up to 250 ms (D3), with a decision-log entry.
3. **Kernel-side cost.** If the A/B build slowdown is > 5%, stop and present it as a decision (D3).

Assert-NoSpikeSession.

---

### Task 11: Write-up, cleanup, PR

**Files:**
- Modify: `docs/specs/2026-10-01-etw-sensor-design.md` (§15.3, Status line, §4.1/§4.2 tuning notes only if a spike changed a value)
- Modify: `docs/architecture-overview.md` (decision log, roadmap row 1)

- [ ] **Step 1: Fill spec §15.3**

Replace *(filled in during plan phase 0)* with the Results-format table: one row per spike with answer, evidence, and design consequence. Add a paragraph for S2 (the candidate table) and for S10 (rates per class), following the privacy rule. Change the Status line to: "Approved (2026-10-01), revision 2; spike results in §15.3 (2026-MM-DD)." Do **not** rewrite other sections. Plan 1b reads §15.3 and applies it. Exception: §4.1 buffer numbers and §4.2 keyword or ID changes, which are edited in place with "(per S9)" / "(per S3)" notes.

- [ ] **Step 2: Decision log and roadmap**

Add one decision-log row per fallback or threshold used (D3 values when applied, UDP default, start-key formula, boot_time source, any added event IDs). Change roadmap row 1's status to: "Planning: plan 1b (build) next ([spec](…), [notes](…))".

- [ ] **Step 3: Clean up**

```powershell
Remove-Item -Recurse -Force C:\AtlasSpike
logman query -ets | Select-String Atlas
git status --short
```
Expected: no Atlas sessions; `git status` shows only the two docs. `spikes/` stays on disk (ignored) until plan 1b is written, then the user may delete it.

- [ ] **Step 4: Commit and open the PR**

```powershell
git add docs/specs/2026-10-01-etw-sensor-design.md docs/architecture-overview.md
git commit -m "docs(1): spike results S1-S10 (spec 15.3), decisions, roadmap`n`nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -u origin docs/1a-spike-results
gh pr create --base main --title "docs(1): plan 1a spike results" --body "Spike results S1-S10 for sub-project 1 (spec section 15.3), fallbacks and thresholds in the decision log, roadmap updated. Next: plan 1b.`n`n🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```
Merging is the user's action.

---

## Appendix A — `spikes/probe` source

Create these files verbatim. Layout: `spikes/probe/Cargo.toml`, `spikes/probe/.cargo/config.toml`, `spikes/probe/src/{main,util,etw,tdh,telemetry,boot,act,enrich,report}.rs`.

### `.cargo/config.toml`

Added during Task 2 (2026-10-02): without it `probe.exe` imports `VCRUNTIME140.dll`, which the `edr-test` VM does not have, and the probe silently fails to start there.

```toml
# Link the C runtime statically so probe.exe runs on machines without the VC++ redistributable.
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
```

### `Cargo.toml`

```toml
# Plan 1a spike probe. Throwaway: lives in git-ignored spikes/, never in the workspace.
[package]
name = "probe"
version = "0.0.0"
edition = "2024"
publish = false

# Stand-alone: keeps Cargo from treating this as a member of the repo's workspace.
[workspace]

[dependencies]
clap = { version = "4.6", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"

[dependencies.windows]
version = "0.62"
features = [
    "Wdk_System_SystemInformation",
    "Wdk_System_Threading",
    "Win32_NetworkManagement_Dns",
    "Win32_Security",
    "Win32_Security_Authorization",
    "Win32_Security_Cryptography",
    "Win32_Security_Cryptography_Catalog",
    "Win32_Security_Cryptography_Sip",
    "Win32_Security_WinTrust",
    "Win32_Storage_FileSystem",
    "Win32_System_Console",
    "Win32_System_Diagnostics_Etw",
    "Win32_System_Diagnostics_ToolHelp",
    "Win32_System_IO",
    "Win32_System_Performance",
    "Win32_System_Registry",
    "Win32_System_SystemInformation",
    "Win32_System_Threading",
    "Win32_System_Time",
    "Win32_System_WindowsProgramming",
]
```

### `src/main.rs`

```rust
//! Plan 1a spike probe (throwaway). Answers spec §15.2 questions S1–S10.
//! Not production code: plan 1b writes `atlas-etw` from scratch, using what this probe learned.

mod act;
mod boot;
mod enrich;
mod etw;
mod report;
mod tdh;
mod telemetry;
mod util;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "probe", about = "Atlas plan 1a spike probe (throwaway)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start ETW sessions, consume events in real time, write JSONL and/or stats.
    Trace(etw::TraceArgs),
    /// Dump PROCESS_TELEMETRY_ID_INFORMATION for processes (S1).
    Telemetry {
        /// Process IDs to query (default: every process).
        #[arg(long)]
        pid: Vec<u32>,
        /// Image names to query, e.g. smss.exe.
        #[arg(long)]
        name: Vec<String>,
    },
    /// Print boot-time candidates and BootId sources (S2).
    Boot {
        #[arg(long, default_value_t = 1)]
        repeat: u32,
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
        /// Offset of BootId inside KUSER_SHARED_DATA (hex, from WinDbg `dt`).
        #[arg(long, value_parser = util::parse_hex_u32)]
        kusd_offset: Option<u32>,
    },
    /// Run a scripted action scenario; prints one JSON line per step (S3, S4, S6, S7).
    Act {
        #[arg(value_enum)]
        scenario: act::Scenario,
        /// Working directory for file scenarios (created; must not exist yet).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// file-hold only: milliseconds to wait before writing to the held handle.
        #[arg(long, default_value_t = 10_000)]
        delay_ms: u64,
    },
    /// Spawn processes (S8 join test, canary-launch cost).
    Spawn(act::SpawnArgs),
    /// Exit immediately (the cheapest possible child process).
    Noop,
    /// Exact-size TCP transfer (S5).
    Tcp(act::TcpArgs),
    /// Steady UDP send rate (S9 calibration).
    Udp(act::UdpArgs),
    /// Correlate a trace with an action log, as Markdown.
    Report {
        #[arg(long)]
        events: PathBuf,
        #[arg(long)]
        actions: PathBuf,
        /// Extra time after each step that still counts as that step.
        #[arg(long, default_value_t = 50)]
        slack_ms: u64,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Trace(args) => etw::run(args),
        Cmd::Telemetry { pid, name } => telemetry::run(&pid, &name),
        Cmd::Boot { repeat, interval_ms, kusd_offset } => boot::run(repeat, interval_ms, kusd_offset),
        Cmd::Act { scenario, dir, delay_ms } => act::run(scenario, dir, delay_ms),
        Cmd::Spawn(args) => act::spawn(args),
        Cmd::Noop => Ok(()),
        Cmd::Tcp(args) => act::tcp(args),
        Cmd::Udp(args) => act::udp(args),
        Cmd::Report { events, actions, slack_ms } => report::run(&events, &actions, slack_ms),
    };
    if let Err(e) = result {
        eprintln!("probe: {e}");
        std::process::exit(1);
    }
}
```

### `src/util.rs`

```rust
//! Small helpers shared by the probe's modules.

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

pub type Result<T> = std::result::Result<T, String>;

/// NUL-terminated UTF-16 copy of `s`.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reads a NUL-terminated UTF-16 string at `p` (null pointer → empty).
///
/// # Safety
/// `p` must be null or point to a NUL-terminated UTF-16 string.
pub unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

/// Reads a NUL-terminated UTF-16 string from `bytes` starting at `off`, bounds-checked.
pub fn utf16z_at(bytes: &[u8], off: usize) -> Option<String> {
    let mut units = Vec::new();
    let mut i = off;
    while i + 1 < bytes.len() {
        let u = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        if u == 0 {
            return Some(String::from_utf16_lossy(&units));
        }
        units.push(u);
        i += 2;
    }
    None
}

pub fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

pub fn i64_le(b: &[u8], off: usize) -> i64 {
    u64_at(b, off).map_or(0, |v| v as i64)
}

pub fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

pub fn qpc_now() -> i64 {
    let mut v = 0;
    unsafe { QueryPerformanceCounter(&mut v).expect("QueryPerformanceCounter") };
    v
}

pub fn qpc_freq() -> i64 {
    let mut v = 0;
    unsafe { QueryPerformanceFrequency(&mut v).expect("QueryPerformanceFrequency") };
    v
}

pub fn ms_to_qpc(ms: u64) -> i64 {
    (ms as i128 * qpc_freq() as i128 / 1000) as i64
}

pub fn qpc_to_ms(ticks: i64) -> f64 {
    ticks as f64 * 1000.0 / qpc_freq() as f64
}

pub fn ft_to_u64(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// FILETIME (100 ns since 1601-01-01 UTC) → "YYYY-MM-DDTHH:MM:SS.fffffffZ".
pub fn filetime_iso(ft: u64) -> String {
    const UNIX_EPOCH_FT: u64 = 116_444_736_000_000_000;
    if ft < UNIX_EPOCH_FT {
        return format!("filetime:{ft}");
    }
    let t = ft - UNIX_EPOCH_FT;
    let (secs, frac) = (t / 10_000_000, t % 10_000_000);
    let (days, sod) = ((secs / 86_400) as i64, secs % 86_400);
    // Howard Hinnant's days-from-civil inverse.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{frac:07}Z", sod / 3600, sod % 3600 / 60, sod % 60)
}

/// This process's user + kernel CPU time, in 100 ns units.
pub fn own_cpu_100ns() -> u64 {
    let (mut c, mut e, mut k, mut u) = Default::default();
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).expect("GetProcessTimes") };
    ft_to_u64(k) + ft_to_u64(u)
}

pub fn parse_hex_u32(s: &str) -> std::result::Result<u32, String> {
    let t = s.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(t, 16).map_err(|e| e.to_string())
}

pub fn parse_hex_u64(s: &str) -> std::result::Result<u64, String> {
    let t = s.trim_start_matches("0x").trim_start_matches("0X");
    u64::from_str_radix(t, 16).map_err(|e| e.to_string())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Nearest-rank percentile of an ascending-sorted slice.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_iso_known_values() {
        assert_eq!(filetime_iso(116_444_736_000_000_000), "1970-01-01T00:00:00.0000000Z");
        // 2026-10-02T12:34:56.5Z
        assert_eq!(filetime_iso(134_354_180_965_000_000), "2026-10-02T12:34:56.5000000Z");
    }

    #[test]
    fn utf16z_reads_and_bounds_checks() {
        let b = [b'a', 0, b'b', 0, 0, 0, b'x', 0];
        assert_eq!(utf16z_at(&b, 0).as_deref(), Some("ab"));
        assert_eq!(utf16z_at(&b, 6), None);
    }

    #[test]
    fn percentile_nearest_rank() {
        let v = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile(&v, 50.0), 2.0);
        assert_eq!(percentile(&v, 100.0), 4.0);
    }
}
```

### `src/etw.rs`

```rust
//! ETW sessions and the real-time consumer.
//!
//! Session A mirrors spec §4.1 (manifest providers, QPC clock, 250 ms flush, start-key enable
//! property, event-ID allow-lists). Session B is the system logger with EVENT_TRACE_FLAG_PROCESS.

use crate::enrich;
use crate::tdh;
use crate::util::{Result, filetime_iso, hex, ms_to_qpc, own_cpu_100ns, parse_hex_u64, qpc_freq, qpc_now, wide};
use clap::Args;
use serde_json::{Value, json};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{ERROR_SUCCESS, ERROR_WMI_INSTANCE_NOT_FOUND, GetLastError};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::Diagnostics::Etw::*;
use windows::core::{BOOL, GUID, PCWSTR, PWSTR};

/// Not in the `windows` crate: evntrace.h `EVENT_TRACE_USE_MS_FLUSH_TIMER`.
const EVENT_TRACE_USE_MS_FLUSH_TIMER: u32 = 0x0000_0010;
const INVALID_PROCESSTRACE_HANDLE: u64 = u64::MAX;
/// The session's own header event ("EventTrace" class); skipped.
const EVENT_TRACE_GUID: GUID = GUID::from_u128(0x68fdd900_4a3e_11d1_84f4_0000f80464e3);
/// Classic kernel "Process" event class (Session B).
pub const PROCESS_CLASSIC_GUID: GUID = GUID::from_u128(0x3d6fa8d0_fe05_11d0_9dda_00c04fd7ba7c);
pub const KERNEL_PROCESS_GUID: GUID = GUID::from_u128(0x22fb2cd6_0e7b_422b_a0c7_2fad1fd0e716);

/// Short names for the providers in spec §4.2. Task 2 checks each GUID with `logman query providers`.
const KNOWN: &[(&str, GUID)] = &[
    ("kernel-process", KERNEL_PROCESS_GUID),
    ("kernel-file", GUID::from_u128(0xedd08927_9cc4_4e65_b970_c2560fb5c289)),
    ("kernel-registry", GUID::from_u128(0x70eb4f03_c1de_4f73_a051_33d13d5413bd)),
    ("kernel-network", GUID::from_u128(0x7dd42a49_5329_4832_8dfd_43d979153a88)),
    ("dns-client", GUID::from_u128(0x1c95126e_7eea_49a9_a3fe_a378b03ddb4d)),
    ("process-classic", PROCESS_CLASSIC_GUID),
];

pub fn provider_name(g: &GUID) -> String {
    KNOWN.iter().find(|(_, k)| k == g).map(|(n, _)| (*n).to_string()).unwrap_or_else(|| format!("{g:?}"))
}

#[derive(Args)]
pub struct TraceArgs {
    /// Session A provider: NAME_OR_GUID[:KEYWORDS_HEX[:ID,ID,...]] (repeatable). No IDs = no filter.
    #[arg(long = "provider")]
    providers: Vec<String>,
    /// Also start Session B: system logger, EVENT_TRACE_FLAG_PROCESS (classic process events).
    #[arg(long)]
    process_logger: bool,
    /// After enabling, ask every Session A provider for a rundown (EVENT_CONTROL_CODE_CAPTURE_STATE).
    #[arg(long)]
    capture_state: bool,
    /// Stop after this many seconds (default: when the trailing command exits, else 30).
    #[arg(long)]
    seconds: Option<u64>,
    /// Keep only events of these processes and their descendants (repeatable).
    #[arg(long)]
    pid: Vec<u32>,
    /// Keep every event. Writes host activity to disk: use only into spikes/results/.
    #[arg(long)]
    all: bool,
    /// Always keep PROVIDER:ID events whatever their PID, e.g. kernel-file:10 (repeatable).
    #[arg(long)]
    keep: Vec<String>,
    /// Write events as JSON lines (TDH-decoded unless --no-decode).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Include each event's payload as hex.
    #[arg(long)]
    raw: bool,
    /// Skip TDH decoding (stats runs; PID filtering then needs --all).
    #[arg(long)]
    no_decode: bool,
    /// Write counters, rates, CPU and session loss as one JSON document.
    #[arg(long)]
    stats: Option<PathBuf>,
    /// Add the per-event pipeline cost model (ordering heap + hash-map lookup).
    #[arg(long)]
    model: bool,
    /// Hash and verify each newly launched image on 2 below-normal workers.
    #[arg(long)]
    enrich: bool,
    #[arg(long, default_value_t = 1000)]
    enrich_deadline_ms: u64,
    #[arg(long, default_value = "Atlas-Spike")]
    session_prefix: String,
    #[arg(long, default_value_t = 64)]
    buffer_kb: u32,
    #[arg(long)]
    min_buffers: Option<u32>,
    #[arg(long)]
    max_buffers: Option<u32>,
    /// Wait after enabling before starting the trailing command.
    #[arg(long, default_value_t = 1500)]
    settle_ms: u64,
    /// Keep consuming after the trailing command exits.
    #[arg(long, default_value_t = 2000)]
    linger_ms: u64,
    /// Command to run once the sessions are live; its stdout is saved as <out>.actions.jsonl.
    #[arg(last = true)]
    run: Vec<String>,
}

struct ProviderSpec {
    guid: GUID,
    keywords: u64,
    ids: Vec<u16>,
}

fn parse_guid(s: &str) -> Result<GUID> {
    if let Some((_, g)) = KNOWN.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)) {
        return Ok(*g);
    }
    let t: String = s.trim_matches(|c| c == '{' || c == '}').chars().filter(|c| *c != '-').collect();
    u128::from_str_radix(&t, 16).map(GUID::from_u128).map_err(|_| format!("unknown provider {s:?}"))
}

fn parse_provider(s: &str) -> Result<ProviderSpec> {
    let mut parts = s.splitn(3, ':');
    let guid = parse_guid(parts.next().unwrap_or_default())?;
    let keywords = match parts.next() {
        Some(k) if !k.is_empty() => parse_hex_u64(k)?,
        _ => u64::MAX,
    };
    let ids = match parts.next() {
        Some(list) if !list.is_empty() => list
            .split(',')
            .map(|i| i.trim().parse::<u16>().map_err(|e| format!("event id {i:?}: {e}")))
            .collect::<Result<_>>()?,
        _ => Vec::new(),
    };
    Ok(ProviderSpec { guid, keywords, ids })
}

fn parse_keep(s: &str) -> Result<(GUID, u16)> {
    let (p, id) = s.rsplit_once(':').ok_or(format!("--keep {s:?}: expected PROVIDER:ID"))?;
    Ok((parse_guid(p)?, id.parse().map_err(|e| format!("--keep {s:?}: {e}"))?))
}

// ---------- sessions ----------

struct Session {
    label: &'static str,
    name: Vec<u16>,
    handle: CONTROLTRACE_HANDLE,
    stopped: std::cell::Cell<bool>,
}

/// EVENT_TRACE_PROPERTIES followed by room for the logger name, 8-byte aligned.
fn props_buffer() -> Vec<u64> {
    vec![0u64; (size_of::<EVENT_TRACE_PROPERTIES>() + 2048).div_ceil(8)]
}

fn props_ptr(buf: &mut [u64]) -> *mut EVENT_TRACE_PROPERTIES {
    let p = buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
    unsafe {
        (*p).Wnode.BufferSize = (buf.len() * 8) as u32;
        (*p).LoggerNameOffset = size_of::<EVENT_TRACE_PROPERTIES>() as u32;
    }
    p
}

fn stop_by_name(name: &[u16]) {
    let mut buf = props_buffer();
    let p = props_ptr(&mut buf);
    let st =
        unsafe { ControlTraceW(CONTROLTRACE_HANDLE { Value: 0 }, PCWSTR(name.as_ptr()), p, EVENT_TRACE_CONTROL_STOP) };
    if st == ERROR_SUCCESS {
        eprintln!("trace: stopped a leftover session {}", String::from_utf16_lossy(&name[..name.len() - 1]));
    } else if st != ERROR_WMI_INSTANCE_NOT_FOUND {
        eprintln!("trace: stopping leftover session failed: {st:?}");
    }
}

struct BufferCfg {
    kb: u32,
    min: u32,
    max: u32,
}

impl Session {
    fn start(label: &'static str, name: &str, system_logger: bool, cfg: &BufferCfg) -> Result<Session> {
        let name_w = wide(name);
        stop_by_name(&name_w);
        let mut buf = props_buffer();
        let p = props_ptr(&mut buf);
        unsafe {
            (*p).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
            (*p).Wnode.ClientContext = 1; // raw QPC timestamps (spec §3.3)
            (*p).Wnode.Guid = GUID::new().map_err(|e| e.to_string())?;
            (*p).BufferSize = cfg.kb;
            (*p).MinimumBuffers = cfg.min;
            (*p).MaximumBuffers = cfg.max;
            (*p).LogFileMode = EVENT_TRACE_REAL_TIME_MODE
                | EVENT_TRACE_USE_MS_FLUSH_TIMER
                | if system_logger { EVENT_TRACE_SYSTEM_LOGGER_MODE } else { 0 };
            (*p).FlushTimer = 250;
            if system_logger {
                (*p).EnableFlags = EVENT_TRACE_FLAG_PROCESS;
            }
        }
        let mut handle = CONTROLTRACE_HANDLE::default();
        let st = unsafe { StartTraceW(&mut handle, PCWSTR(name_w.as_ptr()), p) };
        if st != ERROR_SUCCESS {
            return Err(format!("StartTraceW({name}): {st:?} (run elevated?)"));
        }
        Ok(Session { label, name: name_w, handle, stopped: std::cell::Cell::new(false) })
    }

    fn enable(&self, spec: &ProviderSpec) -> Result<()> {
        // EVENT_FILTER_EVENT_ID: FilterIn (BOOLEAN), Reserved, Count (u16), Events[Count].
        let mut filter: Vec<u8> = vec![1, 0];
        filter.extend((spec.ids.len() as u16).to_le_bytes());
        for id in &spec.ids {
            filter.extend(id.to_le_bytes());
        }
        let mut desc = EVENT_FILTER_DESCRIPTOR {
            Ptr: filter.as_ptr() as u64,
            Size: filter.len() as u32,
            Type: EVENT_FILTER_TYPE_EVENT_ID,
        };
        let params = ENABLE_TRACE_PARAMETERS {
            Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
            EnableProperty: EVENT_ENABLE_PROPERTY_PROCESS_START_KEY,
            EnableFilterDesc: if spec.ids.is_empty() { std::ptr::null_mut() } else { &mut desc },
            FilterDescCount: u32::from(!spec.ids.is_empty()),
            ..Default::default()
        };
        let st = unsafe {
            EnableTraceEx2(
                self.handle,
                &spec.guid,
                EVENT_CONTROL_CODE_ENABLE_PROVIDER.0,
                TRACE_LEVEL_VERBOSE as u8,
                spec.keywords,
                0,
                0,
                Some(&params),
            )
        };
        if st != ERROR_SUCCESS {
            return Err(format!("EnableTraceEx2({}): {st:?}", provider_name(&spec.guid)));
        }
        Ok(())
    }

    fn capture_state(&self, spec: &ProviderSpec) -> Result<()> {
        let st = unsafe {
            EnableTraceEx2(
                self.handle,
                &spec.guid,
                EVENT_CONTROL_CODE_CAPTURE_STATE.0,
                TRACE_LEVEL_VERBOSE as u8,
                spec.keywords,
                0,
                0,
                None,
            )
        };
        if st != ERROR_SUCCESS {
            return Err(format!("capture state ({}): {st:?}", provider_name(&spec.guid)));
        }
        Ok(())
    }

    /// Queries (`stop = false`) or stops the session; returns its counters either way.
    fn control(&self, stop: bool) -> Value {
        let mut buf = props_buffer();
        let p = props_ptr(&mut buf);
        let code = if stop { EVENT_TRACE_CONTROL_STOP } else { EVENT_TRACE_CONTROL_QUERY };
        let st = unsafe { ControlTraceW(self.handle, PCWSTR::null(), p, code) };
        if st != ERROR_SUCCESS {
            return json!({ "error": format!("{st:?}") });
        }
        if stop {
            self.stopped.set(true);
        }
        let r = unsafe { &*p };
        json!({
            "events_lost": r.EventsLost,
            "realtime_buffers_lost": r.RealTimeBuffersLost,
            "buffers_written": r.BuffersWritten,
            "buffers": r.NumberOfBuffers,
            "free_buffers": r.FreeBuffers,
            "buffer_kb": r.BufferSize,
            "min_buffers": r.MinimumBuffers,
            "max_buffers": r.MaximumBuffers,
        })
    }
}

/// Stops the session if the run ends early (an enable error, a panic): ETW sessions outlive
/// the process that started them.
impl Drop for Session {
    fn drop(&mut self) {
        if !self.stopped.get() {
            let _ = self.control(true);
            eprintln!("trace: stopped session {}", String::from_utf16_lossy(&self.name[..self.name.len() - 1]));
        }
    }
}

// ---------- consumer ----------

struct Ev {
    ts: i64,
    pids: Vec<u32>,
    /// (claimed parents, child) for process-start events, to follow descendants.
    start: Option<(Vec<u32>, u32)>,
    /// Child PID for process-stop events.
    stop: Option<u32>,
    key: (GUID, u16),
    json: Value,
}

#[derive(Default)]
struct Stats {
    per_key: HashMap<(u128, u16), BTreeMap<i64, u64>>,
    per_sec: BTreeMap<i64, u64>,
    self_events: u64,
    total: u64,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Held {
    ts: i64,
    seq: u64,
    data: Box<[u8]>,
}

/// Approximates the agent's per-event work after parsing: copy into an ordering heap held
/// 750 ms, release in order, one hash-map lookup per released event.
struct Model {
    heap: BinaryHeap<Reverse<Held>>,
    seq: u64,
    hold: i64,
    map: HashMap<u64, u64>,
    released: u64,
}

impl Model {
    fn on_event(&mut self, ts: i64, pid: u32, data: &[u8]) {
        let copy: Box<[u8]> = data[..data.len().min(128)].into();
        self.heap.push(Reverse(Held { ts, seq: self.seq, data: copy }));
        self.seq += 1;
        let now = qpc_now();
        while let Some(Reverse(h)) = self.heap.peek() {
            if now - h.ts < self.hold {
                break;
            }
            let Reverse(h) = self.heap.pop().expect("peeked");
            let mut key = u64::from(pid);
            for chunk in h.data.chunks(8).take(4) {
                let mut b = [0u8; 8];
                b[..chunk.len()].copy_from_slice(chunk);
                key = key.rotate_left(13) ^ u64::from_le_bytes(b);
            }
            *self.map.entry(key).or_insert(0) += 1;
            if self.map.len() > 100_000 {
                self.map.clear();
            }
            self.released += 1;
        }
    }
}

struct Consumer {
    label: &'static str,
    record: bool,
    decode: bool,
    raw: bool,
    stats_on: bool,
    own_pid: u32,
    freq: i64,
    events: Mutex<Vec<Ev>>,
    stats: Mutex<Stats>,
    model: Option<Mutex<Model>>,
    enrich: Option<enrich::Pool>,
}

fn field_u32(fields: &serde_json::Map<String, Value>, name: &str) -> Option<u32> {
    let s = fields.get(name)?.as_str()?;
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

const PID_FIELDS: &[&str] = &["PID", "ProcessID", "ProcessId", "ClientPID", "ParentProcessID", "ParentId"];

impl Consumer {
    fn handle(&self, rec: &EVENT_RECORD) {
        let h = &rec.EventHeader;
        if h.ProviderId == EVENT_TRACE_GUID {
            return;
        }
        let d = &h.EventDescriptor;
        let data: &[u8] = if rec.UserData.is_null() {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(rec.UserData as *const u8, rec.UserDataLength as usize) }
        };
        let start_key = (0..rec.ExtendedDataCount as usize).find_map(|i| unsafe {
            let item = &*rec.ExtendedData.add(i);
            (u32::from(item.ExtType) == EVENT_HEADER_EXT_TYPE_PROCESS_START_KEY && item.DataSize >= 8)
                .then(|| (item.DataPtr as *const u64).read_unaligned())
        });

        if self.stats_on {
            let mut s = self.stats.lock().expect("stats");
            let sec = h.TimeStamp / self.freq;
            *s.per_key.entry((h.ProviderId.to_u128(), d.Id)).or_default().entry(sec).or_insert(0) += 1;
            *s.per_sec.entry(sec).or_insert(0) += 1;
            s.total += 1;
            if h.ProcessId == self.own_pid {
                s.self_events += 1;
            }
        }
        if let Some(m) = &self.model {
            m.lock().expect("model").on_event(h.TimeStamp, h.ProcessId, data);
        }
        if let Some(pool) = &self.enrich
            && h.ProviderId == KERNEL_PROCESS_GUID
            && d.Id == 1
            && let Ok(dec) = tdh::decode(rec)
            && let Some(Value::String(image)) = dec.fields.get("ImageName")
        {
            pool.submit(image.clone(), h.TimeStamp);
        }
        if !self.record {
            return;
        }

        let mut j = json!({
            "session": self.label,
            "ts": h.TimeStamp,
            "provider": provider_name(&h.ProviderId),
            "id": d.Id,
            "version": d.Version,
            "opcode": d.Opcode,
            "task": d.Task,
            "keyword": format!("{:#x}", d.Keyword),
            "pid": h.ProcessId,
            "tid": h.ThreadId,
            "flags": format!("{:#06x}", h.Flags),
            "start_key": start_key.map(|k| format!("{k:#018x}")),
        });
        let mut pids = vec![h.ProcessId];
        let mut start = None;
        let mut stop = None;
        let mut decoded_ok = false;
        if self.decode {
            match tdh::decode(rec) {
                Ok(dec) => {
                    decoded_ok = dec.error.is_none();
                    pids.extend(PID_FIELDS.iter().filter_map(|f| field_u32(&dec.fields, f)));
                    let classic_start = h.ProviderId == PROCESS_CLASSIC_GUID && (d.Opcode == 1 || d.Opcode == 3);
                    let manifest_start = h.ProviderId == KERNEL_PROCESS_GUID && d.Id == 1;
                    if manifest_start || classic_start {
                        let child = field_u32(&dec.fields, if manifest_start { "ProcessID" } else { "ProcessId" });
                        let parent =
                            field_u32(&dec.fields, if manifest_start { "ParentProcessID" } else { "ParentId" });
                        if let Some(c) = child {
                            let mut parents = vec![h.ProcessId];
                            parents.extend(parent);
                            start = Some((parents, c));
                        }
                    }
                    let manifest_stop = h.ProviderId == KERNEL_PROCESS_GUID && d.Id == 2;
                    let classic_stop = h.ProviderId == PROCESS_CLASSIC_GUID && d.Opcode == 2;
                    if manifest_stop || classic_stop {
                        stop = field_u32(&dec.fields, if manifest_stop { "ProcessID" } else { "ProcessId" });
                    }
                    j["provider_name"] = json!(dec.provider_name);
                    j["task_name"] = json!(dec.task);
                    j["opcode_name"] = json!(dec.opcode);
                    j["fields"] = Value::Object(dec.fields);
                    if let Some(e) = dec.error {
                        j["decode_error"] = json!(e);
                    }
                }
                Err(st) => j["decode_error"] = json!(format!("TdhGetEventInformation: {st}")),
            }
        }
        if self.raw || !decoded_ok {
            j["raw"] = json!(hex(data));
        }
        self.events.lock().expect("events").push(Ev {
            ts: h.TimeStamp,
            pids,
            start,
            stop,
            key: (h.ProviderId, d.Id),
            json: j,
        });
    }
}

unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
    // A panic must not unwind into ETW.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let rec = &*rec;
        if let Some(c) = (rec.UserContext as *const Consumer).as_ref() {
            c.handle(rec);
        }
    }));
}

/// Opens a real-time consumer on `name` and starts its ProcessTrace thread.
fn consume(name: &[u16], ctx: &'static Consumer) -> Result<(std::thread::JoinHandle<()>, Value)> {
    let mut name_buf = name.to_vec();
    let mut lf: EVENT_TRACE_LOGFILEW = unsafe { std::mem::zeroed() };
    lf.LoggerName = PWSTR(name_buf.as_mut_ptr());
    lf.Anonymous1.ProcessTraceMode =
        PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
    lf.Anonymous2.EventRecordCallback = Some(on_event);
    lf.Context = ctx as *const Consumer as *mut _;
    let h = unsafe { OpenTraceW(&mut lf) };
    if h.Value == INVALID_PROCESSTRACE_HANDLE {
        return Err(format!("OpenTraceW: {:?}", unsafe { GetLastError() }));
    }
    let hdr = &lf.LogfileHeader;
    let header = json!({
        "boot_time": hdr.BootTime,
        "boot_time_utc": filetime_iso(hdr.BootTime as u64),
        "perf_freq": hdr.PerfFreq,
        "start_time_utc": filetime_iso(hdr.StartTime as u64),
        "pointer_size": unsafe { hdr.Anonymous2.Anonymous.PointerSize },
    });
    let t = std::thread::spawn(move || unsafe {
        let st = ProcessTrace(&[h], None, None);
        if st != ERROR_SUCCESS {
            eprintln!("trace: ProcessTrace returned {st:?}");
        }
        let _ = CloseTrace(h);
    });
    Ok((t, header))
}

// ---------- Ctrl+C ----------

static STOP: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn on_ctrl(_ctrl: u32) -> BOOL {
    STOP.store(true, Ordering::SeqCst);
    BOOL(1)
}

// ---------- run ----------

pub fn run(a: TraceArgs) -> Result<()> {
    let specs: Vec<ProviderSpec> = a.providers.iter().map(|s| parse_provider(s)).collect::<Result<_>>()?;
    let keep: HashSet<(u128, u16)> =
        a.keep.iter().map(|s| parse_keep(s).map(|(g, id)| (g.to_u128(), id))).collect::<Result<_>>()?;
    if specs.is_empty() && !a.process_logger {
        return Err("nothing to trace: give --provider and/or --process-logger".into());
    }
    let record = a.out.is_some();
    if record && !a.all && a.pid.is_empty() && a.run.is_empty() {
        return Err("--out needs --pid, --all or a trailing command (-- CMD ARGS)".into());
    }
    if record && a.no_decode && !a.all {
        return Err("PID filtering needs decoded fields: drop --no-decode or add --all".into());
    }
    if record && a.stats.is_some() && !a.no_decode {
        eprintln!("trace: warning: TDH decoding inflates the CPU figures in --stats");
    }
    unsafe { SetConsoleCtrlHandler(Some(on_ctrl), true) }.map_err(|e| e.to_string())?;

    let cpus = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
    let cfg = BufferCfg {
        kb: a.buffer_kb,
        min: a.min_buffers.unwrap_or(2 * cpus),
        max: a.max_buffers.unwrap_or((4 * cpus).max(256)),
    };
    let freq = qpc_freq();
    let enrich_pool = a.enrich.then(|| enrich::Pool::new(2, a.enrich_deadline_ms));
    let make = |label: &'static str, enrich: Option<enrich::Pool>| -> &'static Consumer {
        Box::leak(Box::new(Consumer {
            label,
            record,
            decode: record && !a.no_decode,
            raw: a.raw,
            stats_on: a.stats.is_some(),
            own_pid: std::process::id(),
            freq,
            events: Mutex::new(Vec::new()),
            stats: Mutex::new(Stats::default()),
            model: a.model.then(|| {
                Mutex::new(Model {
                    heap: BinaryHeap::new(),
                    seq: 0,
                    hold: ms_to_qpc(750),
                    map: HashMap::new(),
                    released: 0,
                })
            }),
            enrich,
        }))
    };

    let mut sessions = Vec::new();
    let mut consumers = Vec::new();
    let mut threads = Vec::new();
    let mut headers = serde_json::Map::new();
    if !specs.is_empty() {
        let s = Session::start("A", &format!("{}-A", a.session_prefix), false, &cfg)?;
        let c = make("A", enrich_pool);
        let (t, hdr) = consume(&s.name, c)?;
        for spec in &specs {
            s.enable(spec)?;
        }
        headers.insert("A".into(), hdr);
        sessions.push(s);
        consumers.push(c);
        threads.push(t);
    }
    if a.process_logger {
        let small = BufferCfg { kb: 64, min: 4, max: 16 };
        let s = Session::start("B", &format!("{}-B", a.session_prefix), true, &small)?;
        let c = make("B", None);
        let (t, hdr) = consume(&s.name, c)?;
        headers.insert("B".into(), hdr);
        sessions.push(s);
        consumers.push(c);
        threads.push(t);
    }
    if a.capture_state
        && let Some(s) = sessions.iter().find(|s| s.label == "A")
    {
        for spec in &specs {
            s.capture_state(spec)?;
        }
    }
    let cpu0 = own_cpu_100ns();
    let t0 = Instant::now();
    eprintln!("trace: sessions live; {} provider(s); process logger: {}", specs.len(), a.process_logger);

    // Run the child, or wait for the duration / Ctrl+C.
    let mut seed: HashSet<u32> = a.pid.iter().copied().collect();
    let deadline = a.seconds.map(|s| t0 + Duration::from_secs(s));
    let mut child_status = None;
    if !a.run.is_empty() {
        std::thread::sleep(Duration::from_millis(a.settle_ms));
        let mut child = Command::new(&a.run[0])
            .args(&a.run[1..])
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("starting {:?}: {e}", a.run[0]))?;
        seed.insert(child.id());
        eprintln!("trace: started child pid {}", child.id());
        let stdout = child.stdout.take().expect("piped");
        let actions_path = a.out.as_deref().map(|o| o.with_extension("actions.jsonl"));
        let copier = std::thread::spawn(move || {
            let mut sink = actions_path.and_then(|p| std::fs::File::create(p).ok()).map(BufWriter::new);
            for line in BufReader::new(stdout).lines().map_while(std::result::Result::ok) {
                println!("{line}");
                if let Some(f) = sink.as_mut() {
                    let _ = writeln!(f, "{line}");
                }
            }
        });
        loop {
            if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
                child_status = Some(st.to_string());
                break;
            }
            if STOP.load(Ordering::SeqCst) || deadline.is_some_and(|d| Instant::now() >= d) {
                let _ = child.kill();
                child_status = Some("killed".into());
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = copier.join();
        std::thread::sleep(Duration::from_millis(a.linger_ms));
    } else {
        let until = deadline.unwrap_or(t0 + Duration::from_secs(30));
        while Instant::now() < until && !STOP.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    let wall = t0.elapsed().as_secs_f64();
    let cpu = (own_cpu_100ns() - cpu0) as f64 / 1e7;
    let mut session_stats = serde_json::Map::new();
    for s in &sessions {
        session_stats.insert(s.label.into(), s.control(true));
    }
    for t in threads {
        let _ = t.join();
    }
    eprintln!("trace: stopped after {wall:.1} s; child: {child_status:?}");

    if let Some(out) = &a.out {
        write_events(out, &consumers, &seed, a.all, &keep)?;
    }
    if let Some(path) = &a.stats {
        let mut doc = json!({
            "wall_s": wall,
            "probe_cpu_s": cpu,
            "probe_cpu_pct_of_one_core": 100.0 * cpu / wall,
            "logical_cpus": cpus,
            "qpc_freq": freq,
            "buffer_cfg": { "kb": cfg.kb, "min": cfg.min, "max": cfg.max },
            "sessions": session_stats,
            "logfile_headers": headers,
            "providers": a.providers,
        });
        for c in &consumers {
            doc[c.label] = consumer_stats(c, wall);
        }
        if let Some(pool) = consumers.iter().find_map(|c| c.enrich.as_ref()) {
            doc["enrich"] = pool.finish();
        }
        std::fs::write(path, serde_json::to_string_pretty(&doc).expect("json")).map_err(|e| e.to_string())?;
        eprintln!("trace: stats written to {}", path.display());
    }
    Ok(())
}

fn consumer_stats(c: &Consumer, wall: f64) -> Value {
    let s = c.stats.lock().expect("stats");
    let mut rows: Vec<Value> = s
        .per_key
        .iter()
        .map(|((g, id), secs)| {
            let count: u64 = secs.values().sum();
            json!({
                "provider": provider_name(&GUID::from_u128(*g)),
                "id": id,
                "count": count,
                "per_s_avg": count as f64 / wall,
                "per_s_peak": secs.values().max().copied().unwrap_or(0),
            })
        })
        .collect();
    rows.sort_by_key(|r| Reverse(r["count"].as_u64().unwrap_or(0)));
    let mut v = json!({
        "total": s.total,
        "per_s_avg": s.total as f64 / wall,
        "per_s_peak": s.per_sec.values().max().copied().unwrap_or(0),
        "self_events": s.self_events,
        "by_event": rows,
    });
    if let Some(m) = &c.model {
        let m = m.lock().expect("model");
        v["model"] = json!({ "released": m.released, "held_at_end": m.heap.len() });
    }
    v
}

fn write_events(
    out: &Path,
    consumers: &[&'static Consumer],
    seed: &HashSet<u32>,
    all: bool,
    keep: &HashSet<(u128, u16)>,
) -> Result<()> {
    let mut evs: Vec<Ev> =
        consumers.iter().flat_map(|c| std::mem::take(&mut *c.events.lock().expect("events"))).collect();
    evs.sort_by_key(|e| e.ts);
    let total = evs.len();
    let mut tree = seed.clone();
    let mut f = BufWriter::new(std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?);
    let mut kept = 0usize;
    for e in evs {
        if let Some((parents, child)) = &e.start
            && parents.iter().any(|p| tree.contains(p))
        {
            tree.insert(*child);
        }
        let in_tree = e.pids.iter().any(|p| tree.contains(p));
        if all || in_tree || keep.contains(&(e.key.0.to_u128(), e.key.1)) {
            writeln!(f, "{}", e.json).map_err(|e| e.to_string())?;
            kept += 1;
        }
        if let Some(p) = e.stop
            && !seed.contains(&p)
        {
            tree.remove(&p); // the PID may be reused by an unrelated process later
        }
    }
    eprintln!("trace: wrote {kept} of {total} events to {}", out.display());
    Ok(())
}
```

### `src/tdh.rs`

```rust
//! Decodes one event with TDH into named, formatted fields. The probe uses TDH as the
//! oracle (spec §4.3); the agent never will per event.

use crate::util::from_wide_ptr;
use serde_json::{Map, Value, json};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
use windows::Win32::System::Diagnostics::Etw::*;
use windows::core::PWSTR;

pub struct Decoded {
    pub provider_name: String,
    pub task: String,
    pub opcode: String,
    pub fields: Map<String, Value>,
    /// Set when decoding stopped early; fields decoded before the error are kept.
    pub error: Option<String>,
}

pub fn decode(rec: &EVENT_RECORD) -> Result<Decoded, u32> {
    let mut size = 0u32;
    let st = unsafe { TdhGetEventInformation(rec, None, None, &mut size) };
    if st != ERROR_INSUFFICIENT_BUFFER.0 {
        return Err(st);
    }
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let info = buf.as_mut_ptr() as *mut TRACE_EVENT_INFO;
    let st = unsafe { TdhGetEventInformation(rec, None, Some(info), &mut size) };
    if st != ERROR_SUCCESS.0 {
        return Err(st);
    }
    let base = buf.as_ptr() as *const u8;
    let name_at = |off: u32| -> String {
        if off == 0 { String::new() } else { unsafe { from_wide_ptr(base.add(off as usize) as *const u16) } }
    };
    let info_ref = unsafe { &*info };
    let props = unsafe {
        std::slice::from_raw_parts(info_ref.EventPropertyInfoArray.as_ptr(), info_ref.PropertyCount as usize)
    };
    let flags = u32::from(rec.EventHeader.Flags);
    let ptr_size = if flags & EVENT_HEADER_FLAG_32_BIT_HEADER != 0 { 4 } else { 8 };
    let data: &[u8] = if rec.UserData.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(rec.UserData as *const u8, rec.UserDataLength as usize) }
    };

    let mut d = Decoder { info, props, data, offset: 0, ptr_size, ints: vec![None; props.len()], name_at: &name_at };
    let mut fields = Map::new();
    let mut error = None;
    for i in 0..info_ref.TopLevelPropertyCount as usize {
        match d.property(i) {
            Ok((name, v)) => {
                fields.insert(name, v);
            }
            Err(e) => {
                error = Some(e);
                break;
            }
        }
    }
    if error.is_none() && d.offset < data.len() {
        fields.insert("_trailing_bytes".into(), json!(data.len() - d.offset));
    }
    Ok(Decoded {
        provider_name: name_at(info_ref.ProviderNameOffset),
        task: name_at(info_ref.TaskNameOffset),
        opcode: name_at(info_ref.OpcodeNameOffset),
        fields,
        error,
    })
}

struct Decoder<'a> {
    info: *const TRACE_EVENT_INFO,
    props: &'a [EVENT_PROPERTY_INFO],
    data: &'a [u8],
    offset: usize,
    ptr_size: u32,
    /// Integer values of already-decoded scalar properties (for length/count references).
    ints: Vec<Option<u64>>,
    name_at: &'a dyn Fn(u32) -> String,
}

impl Decoder<'_> {
    fn property(&mut self, i: usize) -> Result<(String, Value), String> {
        let p = &self.props[i];
        let name = (self.name_at)(p.NameOffset);
        let f = p.Flags.0;
        let count = if f & PropertyParamCount.0 != 0 {
            let idx = unsafe { p.Anonymous2.countPropertyIndex } as usize;
            self.ints.get(idx).copied().flatten().ok_or(format!("{name}: count reference unresolved"))? as usize
        } else {
            unsafe { p.Anonymous2.count as usize }
        };
        let is_array = f & (PropertyParamCount.0 | PropertyParamFixedCount.0) != 0 || count > 1;
        let length = if f & PropertyParamLength.0 != 0 {
            let idx = unsafe { p.Anonymous3.lengthPropertyIndex } as usize;
            self.ints.get(idx).copied().flatten().ok_or(format!("{name}: length reference unresolved"))? as u16
        } else {
            unsafe { p.Anonymous3.length }
        };
        let n = if is_array { count } else { 1 };
        let mut values = Vec::with_capacity(n);
        for _ in 0..n {
            let v = if f & PropertyStruct.0 != 0 {
                let s = unsafe { p.Anonymous1.structType };
                let mut obj = Map::new();
                for m in s.StructStartIndex as usize..(s.StructStartIndex + s.NumOfStructMembers) as usize {
                    let (mn, mv) = self.property(m)?;
                    obj.insert(mn, mv);
                }
                Value::Object(obj)
            } else {
                let t = unsafe { p.Anonymous1.nonStructType };
                if !is_array {
                    self.ints[i] = self.read_int(t.InType);
                }
                let sized_by_ref = f & PropertyParamLength.0 != 0;
                Value::String(self.format(&name, t.InType, t.OutType, length, sized_by_ref)?)
            };
            values.push(v);
        }
        Ok((name, if is_array { Value::Array(values) } else { values.pop().unwrap_or(Value::Null) }))
    }

    /// Little-endian integer at the current offset, for scalar integer in-types.
    fn read_int(&self, in_type: u16) -> Option<u64> {
        let size = match i32::from(in_type) {
            x if x == TDH_INTYPE_INT8.0 || x == TDH_INTYPE_UINT8.0 => 1,
            x if x == TDH_INTYPE_INT16.0 || x == TDH_INTYPE_UINT16.0 => 2,
            x if x == TDH_INTYPE_INT32.0 || x == TDH_INTYPE_UINT32.0 || x == TDH_INTYPE_HEXINT32.0 => 4,
            x if x == TDH_INTYPE_INT64.0 || x == TDH_INTYPE_UINT64.0 || x == TDH_INTYPE_HEXINT64.0 => 8,
            x if x == TDH_INTYPE_POINTER.0 || x == TDH_INTYPE_SIZET.0 => self.ptr_size as usize,
            _ => return None,
        };
        let b = self.data.get(self.offset..self.offset + size)?;
        let mut v = [0u8; 8];
        v[..size].copy_from_slice(b);
        Some(u64::from_le_bytes(v))
    }

    fn format(
        &mut self,
        name: &str,
        in_type: u16,
        out_type: u16,
        length: u16,
        sized_by_ref: bool,
    ) -> Result<String, String> {
        if sized_by_ref && length == 0 {
            return Ok(String::new()); // e.g. CapturedData with CapturedDataSize = 0
        }
        let rest = &self.data[self.offset.min(self.data.len())..];
        if rest.is_empty() {
            return Err(format!("{name}: no data left (older event version?)"));
        }
        let rest = &rest[..rest.len().min(u16::MAX as usize)];
        let mut out = vec![0u16; 256];
        loop {
            let mut size_bytes = (out.len() * 2) as u32;
            let mut consumed = 0u16;
            let st = unsafe {
                TdhFormatProperty(
                    self.info,
                    None,
                    self.ptr_size,
                    in_type,
                    out_type,
                    length,
                    rest,
                    &mut size_bytes,
                    Some(PWSTR(out.as_mut_ptr())),
                    &mut consumed,
                )
            };
            if st == ERROR_INSUFFICIENT_BUFFER.0 {
                out = vec![0u16; (size_bytes as usize).div_ceil(2)];
                continue;
            }
            if st != ERROR_SUCCESS.0 {
                return Err(format!("{name}: TdhFormatProperty error {st} (in_type {in_type})"));
            }
            self.offset += consumed as usize;
            let len = out.iter().position(|&c| c == 0).unwrap_or(out.len());
            return Ok(String::from_utf16_lossy(&out[..len]));
        }
    }
}
```

### `src/telemetry.rs`

```rust
//! PROCESS_TELEMETRY_ID_INFORMATION (NtQueryInformationProcess class 64), for S1 and S2.
//!
//! Layout from phnt `ntpsapi.h` (undocumented). Every field is read bounds-checked, and the
//! output repeats `HeaderSize` and the PID so a layout mismatch is visible at once.

use crate::util::{Result, filetime_iso, u32_at, u64_at, utf16z_at};
use serde_json::{Value, json};
use windows::Wdk::System::Threading::{NtQueryInformationProcess, PROCESSINFOCLASS};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::PSID;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::core::PWSTR;

const PROCESS_TELEMETRY_ID_INFORMATION: PROCESSINFOCLASS = PROCESSINFOCLASS(64);

#[derive(Debug, Clone)]
pub struct Telemetry {
    pub header_size: u32,
    pub pid: u32,
    pub start_key: u64,
    pub create_time: u64,
    pub create_interrupt_time: u64,
    pub create_unbiased_interrupt_time: u64,
    pub sequence_number: u64,
    pub session_create_time: u64,
    pub session_id: u32,
    pub boot_id: u32,
    pub user_sid: Option<String>,
    pub image_path: Option<String>,
    pub command_line: Option<String>,
}

pub fn query(pid: u32) -> Result<Telemetry> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|e| format!("OpenProcess({pid}): {e}"))?;
    let r = query_handle(h);
    unsafe {
        let _ = CloseHandle(h);
    }
    r
}

fn query_handle(h: HANDLE) -> Result<Telemetry> {
    let mut buf = vec![0u64; 1024]; // 8 KiB; retried with the size the kernel asks for
    let mut ret = 0u32;
    loop {
        let st = unsafe {
            NtQueryInformationProcess(
                h,
                PROCESS_TELEMETRY_ID_INFORMATION,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                &mut ret,
            )
        };
        if st.is_ok() {
            break;
        }
        if ret as usize > buf.len() * 8 {
            buf = vec![0u64; (ret as usize).div_ceil(8)];
            continue;
        }
        return Err(format!("NtQueryInformationProcess: NTSTATUS {:#010x}", st.0));
    }
    let b = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, ret as usize) };
    let field = |off| u64_at(b, off).ok_or("telemetry buffer too short".to_string());
    let field32 = |off| u32_at(b, off).ok_or("telemetry buffer too short".to_string());
    let string_at = |off_field: usize| -> Option<String> {
        let off = u32_at(b, off_field)? as usize;
        if off == 0 { None } else { utf16z_at(b, off) }
    };
    let user_sid = u32_at(b, 72).filter(|&o| o != 0 && (o as usize) < b.len()).and_then(|o| {
        let mut s = PWSTR::null();
        unsafe {
            ConvertSidToStringSidW(PSID(b.as_ptr().add(o as usize) as *mut _), &mut s).ok()?;
            let out = s.to_string().ok();
            let _ = LocalFree(Some(HLOCAL(s.0.cast())));
            out
        }
    });
    Ok(Telemetry {
        header_size: field32(0)?,
        pid: field32(4)?,
        start_key: field(8)?,
        create_time: field(16)?,
        create_interrupt_time: field(24)?,
        create_unbiased_interrupt_time: field(32)?,
        sequence_number: field(40)?,
        session_create_time: field(48)?,
        session_id: field32(56)?,
        boot_id: field32(60)?,
        user_sid,
        image_path: string_at(76),
        command_line: string_at(88),
    })
}

impl Telemetry {
    pub fn to_json(&self) -> Value {
        let k = self.start_key;
        let s = self.sequence_number;
        let boot48 = (u64::from(self.boot_id) << 48) | s;
        json!({
            "pid": self.pid,
            "header_size": self.header_size,
            "start_key": format!("{k:#018x}"),
            "sequence_number": format!("{s:#018x}"),
            "boot_id": self.boot_id,
            "key_hi16": k >> 48,
            "key_lo48": format!("{:#014x}", k & 0xFFFF_FFFF_FFFF),
            "key_eq_seq": k == s,
            "key_eq_bootid_shl48_or_seq": k == boot48,
            "create_time": self.create_time,
            "create_time_utc": filetime_iso(self.create_time),
            "create_interrupt_time": self.create_interrupt_time,
            "create_unbiased_interrupt_time": self.create_unbiased_interrupt_time,
            "session_id": self.session_id,
            "session_create_time_raw": self.session_create_time,
            "user_sid": self.user_sid,
            "image_path": self.image_path,
            "command_line": self.command_line,
        })
    }
}

/// (pid, image name) for every running process.
pub fn processes() -> Result<Vec<(u32, String)>> {
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut ok = unsafe { Process32FirstW(snap, &mut e) }.is_ok();
    while ok {
        let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
        out.push((e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..len])));
        ok = unsafe { Process32NextW(snap, &mut e) }.is_ok();
    }
    unsafe {
        let _ = CloseHandle(snap);
    }
    Ok(out)
}

pub fn run(pids: &[u32], names: &[String]) -> Result<()> {
    let all = processes()?;
    let targets: Vec<(u32, String)> = if pids.is_empty() && names.is_empty() {
        all
    } else {
        all.into_iter().filter(|(p, n)| pids.contains(p) || names.iter().any(|w| w.eq_ignore_ascii_case(n))).collect()
    };
    let (mut ok, mut failed) = (0, 0);
    for (pid, name) in targets {
        match query(pid) {
            Ok(t) => {
                ok += 1;
                let mut v = t.to_json();
                v["name"] = json!(name);
                println!("{v}");
            }
            Err(e) => {
                failed += 1;
                println!("{}", json!({ "pid": pid, "name": name, "error": e }));
            }
        }
    }
    eprintln!("telemetry: {ok} processes read, {failed} failed");
    Ok(())
}
```

### `src/boot.rs`

```rust
//! Boot-time candidates and BootId sources (S2). Each candidate is printed as FILETIME and
//! UTC; the spike compares them across clock changes, sleep, agent restarts and reboots.

use crate::telemetry;
use crate::util::{Result, filetime_iso, ft_to_u64, i64_le, u32_at, wide};
use serde_json::{Map, Value, json};
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemTimeOfDayInformation};
use windows::Win32::Foundation::{CloseHandle, FILETIME};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::System::WindowsProgramming::{QueryInterruptTime, QueryUnbiasedInterruptTime};
use windows::core::PCWSTR;

/// User-mode address of KUSER_SHARED_DATA (fixed on every Windows version).
const KUSER_SHARED_DATA: usize = 0x7FFE_0000;

pub fn run(repeat: u32, interval_ms: u64, kusd_offset: Option<u32>) -> Result<()> {
    for i in 0..repeat {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        }
        println!("{}", sample(kusd_offset));
    }
    Ok(())
}

fn ft(v: u64) -> Value {
    json!({ "filetime": v, "utc": filetime_iso(v) })
}

fn sample(kusd_offset: Option<u32>) -> Value {
    let now = ft_to_u64(unsafe { GetSystemTimePreciseAsFileTime() });
    let interrupt = unsafe { QueryInterruptTime() };
    let mut unbiased = 0u64;
    let _ = unsafe { QueryUnbiasedInterruptTime(&mut unbiased) };
    let mut c = Map::new();
    c.insert("now".into(), ft(now));

    // B1: kernel boot time as reported by SystemTimeOfDayInformation.
    let mut tod = [0u8; 48];
    let mut ret = 0u32;
    let st = unsafe {
        NtQuerySystemInformation(SystemTimeOfDayInformation, tod.as_mut_ptr().cast(), tod.len() as u32, &mut ret)
    };
    if st.is_ok() {
        c.insert("B1_timeofday_boot_time".into(), ft(i64_le(&tod, 0) as u64));
        c.insert("B1_boot_time_bias".into(), json!(i64_le(&tod, 32)));
        c.insert("B1_sleep_time_bias".into(), json!(i64_le(&tod, 40)));
    } else {
        c.insert("B1_error".into(), json!(format!("{:#010x}", st.0)));
    }
    // B2: now minus uptime (interrupt time includes sleep; unbiased excludes it).
    c.insert("B2_now_minus_interrupt".into(), ft(now - interrupt));
    c.insert("B2u_now_minus_unbiased".into(), ft(now - unbiased));
    // B3/B4: creation times of the first kernel and user processes.
    for (label, name) in [("B3_system", "System"), ("B4_registry", "Registry"), ("B4_smss", "smss.exe")] {
        c.insert(label.into(), create_time_of(name).map(ft).unwrap_or_else(|e| json!({ "error": e })));
    }
    // BootId from three sources (spec 0a §4.4 says they are the same counter).
    let mut boot_id = Map::new();
    boot_id.insert("registry_prefetch".into(), json!(registry_boot_id()));
    boot_id.insert("telemetry_self".into(), json!(telemetry::query(std::process::id()).ok().map(|t| t.boot_id)));
    if let Some(off) = kusd_offset {
        // SAFETY: KUSER_SHARED_DATA is mapped read-only into every process; the offset is
        // range-checked against its one-page size.
        let v = (off < 0x1000).then(|| unsafe { ((KUSER_SHARED_DATA + off as usize) as *const u32).read_volatile() });
        boot_id.insert(format!("kusd_{off:#x}"), json!(v));
    }
    json!({ "candidates": c, "boot_id": boot_id })
}

fn create_time_of(name: &str) -> Result<u64> {
    let pid = telemetry::processes()?
        .into_iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(name))
        .map(|(p, _)| p)
        .ok_or(format!("{name} not found"))?;
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map_err(|e| e.to_string())?;
    let (mut created, mut e, mut k, mut u) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    let r = unsafe { GetProcessTimes(h, &mut created, &mut e, &mut k, &mut u) };
    unsafe {
        let _ = CloseHandle(h);
    }
    r.map_err(|e| e.to_string())?;
    Ok(ft_to_u64(created))
}

fn registry_boot_id() -> Option<u32> {
    let key = wide(r"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management\PrefetchParameters");
    let val = wide("BootId");
    let mut data = [0u8; 4];
    let mut size = 4u32;
    let st = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(val.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    st.is_ok().then(|| u32_at(&data, 0)).flatten()
}
```

### `src/act.rs`

```rust
//! Scripted actions. Each step prints one JSON line with QPC timestamps before and after, so
//! `probe report` can line up the events a step caused.

use crate::util::{Result, qpc_now, qpc_to_ms, wide};
use clap::{Args, ValueEnum};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, UdpSocket};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HANDLE, WIN32_ERROR};
use windows::Win32::NetworkManagement::Dns::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::System::WindowsProgramming::QueryProcessCycleTime;
use windows::core::PCWSTR;

#[derive(Clone, Copy, ValueEnum)]
pub enum Scenario {
    /// S6: Kernel-File creates, overwrites, renames, deletes, failures, short names, streams.
    File,
    /// S6: open a handle, wait --delay-ms (start the trace meanwhile), then write and close.
    FileHold,
    /// S4/S7: key and value operations under HKCU and HKLM, incl. RegRenameKey.
    Registry,
    /// S3: DnsQuery_W lookups (fresh, cached, NXDOMAIN, AAAA, CNAME chain).
    Dns,
}

type StepResult = std::result::Result<Value, String>;

struct Steps {
    n: u32,
}

impl Steps {
    fn step(&mut self, name: &str, f: impl FnOnce() -> StepResult) {
        self.n += 1;
        let before = qpc_now();
        let r = f();
        let after = qpc_now();
        let mut line = json!({
            "step": self.n, "name": name, "pid": std::process::id(),
            "qpc_before": before, "qpc_after": after,
        });
        match r {
            Ok(info) => {
                line["ok"] = json!(true);
                line["info"] = info;
            }
            Err(e) => {
                line["ok"] = json!(false);
                line["error"] = json!(e);
            }
        }
        println!("{line}");
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_millis(150)); // keeps step windows apart
    }
}

fn os_err(e: std::io::Error) -> String {
    match e.raw_os_error() {
        Some(c) => format!("win32 {c}: {e}"),
        None => e.to_string(),
    }
}

fn w32(r: WIN32_ERROR) -> StepResult {
    if r.is_ok() { Ok(json!(null)) } else { Err(format!("win32 {}", r.0)) }
}

fn h(f: &File) -> HANDLE {
    HANDLE(f.as_raw_handle())
}

pub fn run(s: Scenario, dir: Option<PathBuf>, delay_ms: u64) -> Result<()> {
    match s {
        Scenario::File => file(&work_dir(dir)?),
        Scenario::FileHold => file_hold(&work_dir(dir)?, delay_ms),
        Scenario::Registry => registry(),
        Scenario::Dns => dns(),
    }
}

fn work_dir(dir: Option<PathBuf>) -> Result<PathBuf> {
    let d = dir.ok_or("--dir is required for file scenarios")?;
    if d.exists() {
        return Err(format!("{} already exists; use a fresh directory", d.display()));
    }
    std::fs::create_dir_all(&d).map_err(os_err)?;
    Ok(d)
}

const DELETE_ACCESS: u32 = 0x0001_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

fn write_new(p: &Path, bytes: &[u8]) -> std::result::Result<(), String> {
    OpenOptions::new().write(true).create_new(true).open(p).and_then(|mut f| f.write_all(bytes)).map_err(os_err)
}

fn file(d: &Path) -> Result<()> {
    let p = |n: &str| d.join(n);
    let mut s = Steps { n: 0 };
    s.step("create_new", || write_new(&p("a.txt"), b"0123456789").map(|_| json!(p("a.txt"))));
    s.step("create_new_exists_FAILS", || write_new(&p("a.txt"), b"x").map(|_| json!(null)));
    s.step("create_new_missing_dir_FAILS", || write_new(&p("missing\\x.txt"), b"x").map(|_| json!(null)));
    s.step("open_existing_read", || {
        let mut b = String::new();
        File::open(p("a.txt")).and_then(|mut f| f.read_to_string(&mut b)).map(|n| json!(n)).map_err(os_err)
    });
    s.step("overwrite_create_always", || {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).open(p("a.txt")).map_err(os_err)?;
        f.write_all(b"overwritten").map(|_| json!(null)).map_err(os_err)
    });
    s.step("overwrite_truncate_existing", || {
        OpenOptions::new().write(true).truncate(true).open(p("a.txt")).map(|_| json!(null)).map_err(os_err)
    });
    s.step("open_always_existing", || {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false) // OPEN_ALWAYS
            .open(p("a.txt"))
            .map(|_| json!(null))
            .map_err(os_err)
    });
    s.step("open_always_new", || {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false) // OPEN_ALWAYS
            .open(p("b.txt"))
            .map(|_| json!(null))
            .map_err(os_err)
    });
    s.step("write_three_times_one_handle", || {
        let mut f = OpenOptions::new().append(true).open(p("a.txt")).map_err(os_err)?;
        for chunk in [b"one", b"two", b"333"] {
            f.write_all(chunk).map_err(os_err)?;
            f.flush().map_err(os_err)?;
        }
        Ok(json!(null))
    });
    s.step("rename", || move_file(&p("a.txt"), &p("a-renamed.txt"), false));
    s.step("rename_replace_existing", || {
        write_new(&p("c.txt"), b"c")?;
        move_file(&p("b.txt"), &p("c.txt"), true)
    });
    s.step("rename_onto_existing_FAILS", || move_file(&p("a-renamed.txt"), &p("c.txt"), false));
    s.step("set_basic_info_timestomp", || {
        let f = OpenOptions::new().write(true).open(p("a-renamed.txt")).map_err(os_err)?;
        let t2001: i64 = 126_227_808_000_000_000; // 2001-01-01T00:00:00Z
        let info = FILE_BASIC_INFO {
            CreationTime: t2001,
            LastAccessTime: t2001,
            LastWriteTime: t2001,
            ChangeTime: 0,
            FileAttributes: 0,
        };
        unsafe {
            SetFileInformationByHandle(
                h(&f),
                FileBasicInfo,
                (&info as *const FILE_BASIC_INFO).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )
        }
        .map(|_| json!(null))
        .map_err(|e| e.to_string())
    });
    s.step("set_attributes_hidden", || set_attrs(&p("a-renamed.txt"), FILE_ATTRIBUTE_HIDDEN));
    s.step("delete", || delete(&p("a-renamed.txt")));
    s.step("delete_sharing_violation_FAILS", || {
        write_new(&p("d.txt"), b"d")?;
        let _held =
            OpenOptions::new().read(true).share_mode(1 /* FILE_SHARE_READ only */).open(p("d.txt")).map_err(os_err)?;
        delete(&p("d.txt"))
    });
    s.step("cleanup_d", || delete(&p("d.txt")));
    s.step("delete_readonly_FAILS", || {
        write_new(&p("e.txt"), b"e")?;
        set_attrs(&p("e.txt"), FILE_ATTRIBUTE_READONLY)?;
        delete(&p("e.txt"))
    });
    s.step("cleanup_e", || {
        set_attrs(&p("e.txt"), FILE_ATTRIBUTE_NORMAL)?;
        delete(&p("e.txt"))
    });
    s.step("delete_on_close_flag", || {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .access_mode(GENERIC_WRITE | DELETE_ACCESS)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE.0)
            .open(p("f.txt"))
            .map_err(os_err)?;
        f.write_all(b"f").map(|_| json!(null)).map_err(os_err)
    });
    s.step("delete_disposition_info", || {
        write_new(&p("g.txt"), b"g")?;
        let f = OpenOptions::new().access_mode(DELETE_ACCESS).open(p("g.txt")).map_err(os_err)?;
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        unsafe {
            SetFileInformationByHandle(
                h(&f),
                FileDispositionInfo,
                (&info as *const FILE_DISPOSITION_INFO).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        }
        .map(|_| json!(null))
        .map_err(|e| e.to_string())
    });
    s.step("delete_posix_semantics", || {
        write_new(&p("g2.txt"), b"g")?;
        let f = OpenOptions::new().access_mode(DELETE_ACCESS).open(p("g2.txt")).map_err(os_err)?;
        let info = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_INFO_EX_FLAGS(
                FILE_DISPOSITION_FLAG_DELETE.0 | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS.0,
            ),
        };
        unsafe {
            SetFileInformationByHandle(
                h(&f),
                FileDispositionInfoEx,
                (&info as *const FILE_DISPOSITION_INFO_EX).cast(),
                size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        }
        .map(|_| json!(null))
        .map_err(|e| e.to_string())
    });
    s.step("short_name_open", || {
        let long = p("long-file-name-for-short-name-test.txt");
        write_new(&long, b"s")?;
        let short = short_path(&long)?;
        File::open(&short).map_err(os_err)?;
        Ok(json!({ "long": long, "short": short }))
    });
    s.step("alternate_data_stream", || {
        write_new(&p("h.txt"), b"h")?;
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(p("h.txt:secret"))
            .and_then(|mut f| f.write_all(b"ads"))
            .map_err(os_err)?;
        File::open(p("h.txt::$DATA")).map(|_| json!(null)).map_err(os_err)
    });
    Ok(())
}

fn move_file(from: &Path, to: &Path, replace: bool) -> StepResult {
    let (f, t) = (wide(&from.to_string_lossy()), wide(&to.to_string_lossy()));
    let flags = if replace { MOVEFILE_REPLACE_EXISTING } else { MOVE_FILE_FLAGS(0) };
    unsafe { MoveFileExW(PCWSTR(f.as_ptr()), PCWSTR(t.as_ptr()), flags) }
        .map(|_| json!(null))
        .map_err(|e| e.to_string())
}

fn delete(p: &Path) -> StepResult {
    let w = wide(&p.to_string_lossy());
    unsafe { DeleteFileW(PCWSTR(w.as_ptr())) }.map(|_| json!(null)).map_err(|e| e.to_string())
}

fn set_attrs(p: &Path, a: FILE_FLAGS_AND_ATTRIBUTES) -> StepResult {
    let w = wide(&p.to_string_lossy());
    unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), a) }.map(|_| json!(null)).map_err(|e| e.to_string())
}

fn short_path(p: &Path) -> std::result::Result<String, String> {
    let w = wide(&p.to_string_lossy());
    let mut buf = [0u16; 1024];
    let n = unsafe { GetShortPathNameW(PCWSTR(w.as_ptr()), Some(&mut buf)) } as usize;
    if n == 0 || n > buf.len() {
        return Err("GetShortPathNameW failed".into());
    }
    let s = String::from_utf16_lossy(&buf[..n]);
    if s.eq_ignore_ascii_case(&p.to_string_lossy()) {
        return Err(format!("no 8.3 name generated for {s} (8dot3 disabled on this volume?)"));
    }
    Ok(s)
}

fn file_hold(d: &Path, delay_ms: u64) -> Result<()> {
    let mut s = Steps { n: 0 };
    let path = d.join("held.txt");
    let mut f = OpenOptions::new().write(true).create_new(true).open(&path).map_err(os_err)?;
    eprintln!("act: pid {} holds {}; writing in {delay_ms} ms", std::process::id(), path.display());
    std::thread::sleep(Duration::from_millis(delay_ms));
    s.step("write_on_preopened_handle", || {
        f.write_all(b"after the trace started").map(|_| json!(null)).map_err(os_err)
    });
    s.step("close_preopened_handle", || {
        drop(f);
        Ok(json!(null))
    });
    Ok(())
}

// ---------- registry ----------

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn create_key(parent: HKEY, sub: &str, sam: REG_SAM_FLAGS) -> std::result::Result<(Key, &'static str), String> {
    let w = wide(sub);
    let mut k = HKEY::default();
    let mut disp = REG_CREATE_KEY_DISPOSITION::default();
    let r = unsafe {
        RegCreateKeyExW(
            parent,
            PCWSTR(w.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            sam,
            None,
            &mut k,
            Some(&mut disp),
        )
    };
    if r.is_err() {
        return Err(format!("RegCreateKeyExW({sub}): win32 {}", r.0));
    }
    Ok((Key(k), if disp == REG_CREATED_NEW_KEY { "created_new" } else { "opened_existing" }))
}

fn set_value(k: &Key, name: &str, ty: REG_VALUE_TYPE, data: &[u8]) -> StepResult {
    let w = wide(name);
    w32(unsafe { RegSetValueExW(k.0, PCWSTR(w.as_ptr()), None, ty, Some(data)) })
        .map(|_| json!({ "bytes": data.len() }))
}

fn utf16_bytes(parts: &[&str]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in parts {
        for u in p.encode_utf16().chain(std::iter::once(0)) {
            v.extend(u.to_le_bytes());
        }
    }
    v
}

fn registry() -> Result<()> {
    let mut s = Steps { n: 0 };
    let rw = KEY_ALL_ACCESS;
    let base = r"Software\AtlasSpike";
    s.step("create_key_absolute_hkcu", || {
        create_key(HKEY_CURRENT_USER, &format!(r"{base}\A"), rw).map(|(_, d)| json!(d))
    });
    s.step("create_key_again_existing", || {
        create_key(HKEY_CURRENT_USER, &format!(r"{base}\A"), rw).map(|(_, d)| json!(d))
    });
    let (a, _) = create_key(HKEY_CURRENT_USER, &format!(r"{base}\A"), rw).map_err(|e| e.to_string())?;
    s.step("create_key_relative_nested", || create_key(a.0, r"B\C", rw).map(|(_, d)| json!(d)));
    s.step("set_sz", || set_value(&a, "s", REG_SZ, &utf16_bytes(&["hello"])));
    s.step("set_sz_overwrite", || set_value(&a, "s", REG_SZ, &utf16_bytes(&["hello again"])));
    s.step("set_dword", || set_value(&a, "d", REG_DWORD, &0x1234_5678u32.to_le_bytes()));
    s.step("set_qword", || set_value(&a, "q", REG_QWORD, &0x1122_3344_5566_7788u64.to_le_bytes()));
    s.step("set_binary_16", || set_value(&a, "b", REG_BINARY, &(0u8..16).collect::<Vec<_>>()));
    s.step("set_binary_8k", || set_value(&a, "big", REG_BINARY, &vec![0xAB; 8192]));
    s.step("set_multi_sz", || set_value(&a, "m", REG_MULTI_SZ, &[utf16_bytes(&["one", "two"]), vec![0, 0]].concat()));
    s.step("set_expand_sz", || set_value(&a, "e", REG_EXPAND_SZ, &utf16_bytes(&[r"%SystemRoot%\x.exe"])));
    s.step("delete_value", || w32(unsafe { RegDeleteValueW(a.0, PCWSTR(wide("s").as_ptr())) }));
    s.step("delete_value_missing_FAILS", || w32(unsafe { RegDeleteValueW(a.0, PCWSTR(wide("nope").as_ptr())) }));
    s.step("rename_key", || {
        w32(unsafe { RegRenameKey(a.0, PCWSTR(wide("B").as_ptr()), PCWSTR(wide("B-renamed").as_ptr())) })
    });
    s.step("delete_key_leaf", || w32(unsafe { RegDeleteKeyW(a.0, PCWSTR(wide(r"B-renamed\C").as_ptr())) }));
    s.step("delete_key", || w32(unsafe { RegDeleteKeyW(a.0, PCWSTR(wide("B-renamed").as_ptr())) }));
    s.step("delete_key_missing_FAILS", || w32(unsafe { RegDeleteKeyW(a.0, PCWSTR(wide("nope").as_ptr())) }));
    drop(a);
    s.step("hklm_create_set_delete", || {
        let (k, d) = create_key(HKEY_LOCAL_MACHINE, r"SOFTWARE\AtlasSpike", rw)?;
        set_value(&k, "v", REG_DWORD, &1u32.to_le_bytes())?;
        drop(k);
        // RegDeleteTreeW with a subkey name deletes that key too.
        w32(unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(wide(r"SOFTWARE\AtlasSpike").as_ptr())) })
            .map(|_| json!(d))
    });
    s.step("current_control_set_create_delete", || {
        let path = r"SYSTEM\CurrentControlSet\Control\AtlasSpike";
        let (k, d) = create_key(HKEY_LOCAL_MACHINE, path, rw)?;
        set_value(&k, "v", REG_DWORD, &1u32.to_le_bytes())?;
        drop(k);
        w32(unsafe { RegDeleteKeyW(HKEY_LOCAL_MACHINE, PCWSTR(wide(path).as_ptr())) }).map(|_| json!(d))
    });
    s.step("wow64_32bit_view", || {
        let (k, d) = create_key(HKEY_LOCAL_MACHINE, r"SOFTWARE\AtlasSpike32", rw | KEY_WOW64_32KEY)?;
        set_value(&k, "v", REG_DWORD, &1u32.to_le_bytes())?;
        drop(k);
        w32(unsafe {
            RegDeleteKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(wide(r"SOFTWARE\AtlasSpike32").as_ptr()),
                KEY_WOW64_32KEY.0,
                None,
            )
        })
        .map(|_| json!(d))
    });
    s.step("cleanup_hkcu", || w32(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide(base).as_ptr())) }));
    Ok(())
}

// ---------- dns ----------

fn dns_query(name: &str, ty: DNS_TYPE, opts: DNS_QUERY_OPTIONS) -> StepResult {
    let w = wide(name);
    let mut rec: *mut DNS_RECORDA = std::ptr::null_mut();
    let st = unsafe { DnsQuery_W(PCWSTR(w.as_ptr()), ty, opts, None, &mut rec, None) };
    let mut count = 0;
    let mut cur = rec;
    while !cur.is_null() {
        count += 1;
        cur = unsafe { (*cur).pNext };
    }
    if !rec.is_null() {
        unsafe { DnsFree(Some(rec as *const _), DnsFreeRecordList) };
    }
    if st.is_ok() { Ok(json!({ "records": count })) } else { Err(format!("DNS status {}", st.0)) }
}

fn dns() -> Result<()> {
    let mut s = Steps { n: 0 };
    s.step("a_fresh_example_com", || dns_query("example.com", DNS_TYPE_A, DNS_QUERY_STANDARD));
    s.step("a_cached_example_com", || dns_query("example.com", DNS_TYPE_A, DNS_QUERY_STANDARD));
    s.step("a_bypass_cache_example_com", || dns_query("example.com", DNS_TYPE_A, DNS_QUERY_BYPASS_CACHE));
    s.step("aaaa_example_com", || dns_query("example.com", DNS_TYPE_AAAA, DNS_QUERY_STANDARD));
    s.step("cname_chain_www_microsoft_com", || dns_query("www.microsoft.com", DNS_TYPE_A, DNS_QUERY_STANDARD));
    s.step("nxdomain_FAILS", || dns_query("atlas-spike-does-not-exist.invalid", DNS_TYPE_A, DNS_QUERY_STANDARD));
    Ok(())
}

// ---------- spawn ----------

#[derive(Args)]
pub struct SpawnArgs {
    #[arg(long, default_value_t = 100)]
    count: u32,
    /// Spawning threads.
    #[arg(long, default_value_t = 1)]
    parallel: u32,
    /// Pause between spawns per thread.
    #[arg(long, default_value_t = 0)]
    interval_ms: u64,
    /// Program to start (default: this probe with `noop`).
    #[arg(long)]
    exe: Option<PathBuf>,
    #[arg(last = true)]
    args: Vec<String>,
}

/// CPU cycles of a process, converted to ms at the nominal clock. GetProcessTimes ticks every
/// 15.6 ms, too coarse for a process that lives a few ms.
fn cycles(h: HANDLE) -> Option<u64> {
    let mut v = 0u64;
    unsafe { QueryProcessCycleTime(h, &mut v) }.ok().map(|_| v)
}

fn nominal_mhz() -> f64 {
    let (key, val) = (wide(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"), wide("~MHz"));
    let mut data = 0u32;
    let mut size = 4u32;
    let st = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(val.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if st.is_ok() && data > 0 { f64::from(data) } else { f64::NAN }
}

pub fn spawn(a: SpawnArgs) -> Result<()> {
    let exe = match &a.exe {
        Some(e) => e.clone(),
        None => std::env::current_exe().map_err(|e| e.to_string())?,
    };
    let args = if a.exe.is_none() && a.args.is_empty() { vec!["noop".to_string()] } else { a.args.clone() };
    let mhz = nominal_mhz();
    let cyc0 = cycles(unsafe { GetCurrentProcess() }).unwrap_or(0);
    let t0 = Instant::now();
    let per_thread = a.count.div_ceil(a.parallel.max(1));
    let threads: Vec<_> = (0..a.parallel.max(1))
        .map(|_| {
            let (exe, args) = (exe.clone(), args.clone());
            std::thread::spawn(move || {
                let mut out = Vec::new();
                for _ in 0..per_thread {
                    let before = qpc_now();
                    let r = Command::new(&exe).args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                    let after = qpc_now();
                    match r {
                        Ok(mut c) => {
                            let pid = c.id();
                            let _ = c.wait();
                            out.push(json!({ "pid": pid, "qpc_before": before, "qpc_after": after,
                                             "spawn_ms": qpc_to_ms(after - before),
                                             "child_cpu_ms": cycles(HANDLE(c.as_raw_handle())).map(|n| n as f64 / (mhz * 1e3)) }));
                        }
                        Err(e) => out.push(json!({ "error": e.to_string() })),
                    }
                    if a.interval_ms > 0 {
                        std::thread::sleep(Duration::from_millis(a.interval_ms));
                    }
                }
                out
            })
        })
        .collect();
    let mut spawned = Vec::new();
    for t in threads {
        spawned.extend(t.join().map_err(|_| "spawn thread panicked")?);
    }
    for s in &spawned {
        println!("{s}");
    }
    let child_cpu: Vec<f64> = spawned.iter().filter_map(|s| s["child_cpu_ms"].as_f64()).collect();
    let spawn_ms: Vec<f64> = spawned.iter().filter_map(|s| s["spawn_ms"].as_f64()).collect();
    let mean = |v: &[f64]| {
        if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 }
    };
    println!(
        "{}",
        json!({ "summary": {
            "count": spawned.len(), "wall_s": t0.elapsed().as_secs_f64(),
            "spawner_cpu_ms": (cycles(unsafe { GetCurrentProcess() }).unwrap_or(0) - cyc0) as f64 / (mhz * 1e3),
            "nominal_mhz": mhz,
            "child_cpu_ms_mean": mean(&child_cpu), "spawn_ms_mean": mean(&spawn_ms),
            "exe": exe, "args": args,
        }})
    );
    Ok(())
}

// ---------- network ----------

#[derive(Args)]
pub struct TcpArgs {
    /// Accept one connection on ADDR:PORT, read to EOF, then send --send bytes.
    #[arg(long)]
    listen: Option<String>,
    /// Connect to ADDR:PORT, send --send bytes, half-close, read to EOF.
    #[arg(long)]
    connect: Option<String>,
    #[arg(long, default_value_t = 0)]
    send: u64,
}

fn pump(stream: &mut TcpStream, n: u64) -> std::io::Result<()> {
    let chunk = vec![0x5Au8; 64 * 1024];
    let mut left = n;
    while left > 0 {
        let k = left.min(chunk.len() as u64) as usize;
        stream.write_all(&chunk[..k])?;
        left -= k as u64;
    }
    Ok(())
}

fn drain(stream: &mut TcpStream) -> std::io::Result<u64> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(total);
        }
        total += n as u64;
    }
}

pub fn tcp(a: TcpArgs) -> Result<()> {
    let (role, mut stream, open_qpc) = match (&a.listen, &a.connect) {
        (Some(l), None) => {
            let listener = TcpListener::bind(l).map_err(os_err)?;
            println!("{}", json!({ "ready": true, "pid": std::process::id(), "listen": l }));
            let _ = std::io::stdout().flush();
            let (s, _) = listener.accept().map_err(os_err)?;
            ("server", s, qpc_now())
        }
        (None, Some(c)) => ("client", TcpStream::connect(c).map_err(os_err)?, qpc_now()),
        _ => return Err("give exactly one of --listen or --connect".into()),
    };
    let (local, remote) = (stream.local_addr().ok(), stream.peer_addr().ok());
    let received = if role == "client" {
        pump(&mut stream, a.send).map_err(os_err)?;
        stream.shutdown(Shutdown::Write).map_err(os_err)?;
        drain(&mut stream).map_err(os_err)?
    } else {
        let r = drain(&mut stream).map_err(os_err)?;
        pump(&mut stream, a.send).map_err(os_err)?;
        stream.shutdown(Shutdown::Both).map_err(os_err)?;
        r
    };
    drop(stream);
    println!(
        "{}",
        json!({ "role": role, "pid": std::process::id(), "local": local.map(|x| x.to_string()),
                "remote": remote.map(|x| x.to_string()), "sent": a.send, "received": received,
                "qpc_open": open_qpc, "qpc_close": qpc_now() })
    );
    Ok(())
}

#[derive(Args)]
pub struct UdpArgs {
    #[arg(long, default_value = "127.0.0.1:9")]
    target: String,
    /// Datagrams per second.
    #[arg(long, default_value_t = 1000)]
    pps: u64,
    #[arg(long, default_value_t = 30)]
    seconds: u64,
    #[arg(long, default_value_t = 1200)]
    size: usize,
}

pub fn udp(a: UdpArgs) -> Result<()> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(os_err)?;
    let payload = vec![0u8; a.size];
    let t0 = Instant::now();
    let mut sent = 0u64;
    while t0.elapsed() < Duration::from_secs(a.seconds) {
        let due = (t0.elapsed().as_secs_f64() * a.pps as f64) as u64;
        while sent < due {
            let _ = sock.send_to(&payload, &a.target); // ICMP "port unreachable" errors are expected
            sent += 1;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "{}",
        json!({ "pid": std::process::id(), "target": a.target, "sent": sent, "seconds": a.seconds, "size": a.size })
    );
    Ok(())
}
```

### `src/enrich.rs`

```rust
//! S10 enrichment timing: SHA-256 + Authenticode (embedded, then catalog) for each image the
//! first time it is launched, on 2 below-normal-priority workers (spec §6.3).

use crate::util::{percentile, qpc_now, qpc_to_ms, wide};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::Read;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Security::Cryptography::Catalog::*;
use windows::Win32::Security::WinTrust::*;
use windows::Win32::Storage::FileSystem::QueryDosDeviceW;
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
use windows::core::{GUID, PCWSTR, w};

const SIZE_CAP: u64 = 100 * 1024 * 1024;
const TRUST_E_NOSIGNATURE: i32 = 0x800B_0100_u32 as i32;
const TRUST_E_SUBJECT_FORM_UNKNOWN: i32 = 0x800B_0003_u32 as i32;

struct Job {
    nt_path: String,
    event_ts: i64,
}

struct Outcome {
    path: String,
    size: Option<u64>,
    hash_ms: f64,
    verify_ms: f64,
    total_ms: f64,
    since_event_ms: f64,
    signature: String,
    error: Option<String>,
}

pub struct Pool {
    tx: Mutex<Option<Sender<Job>>>,
    seen: Mutex<HashSet<String>>,
    repeats: Mutex<u64>,
    results: Arc<Mutex<Vec<Outcome>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    deadline_ms: u64,
}

impl Pool {
    pub fn new(n: usize, deadline_ms: u64) -> Pool {
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let results = Arc::new(Mutex::new(Vec::new()));
        let devices = Arc::new(device_map());
        let workers = (0..n)
            .map(|_| {
                let (rx, results, devices) = (rx.clone(), results.clone(), devices.clone());
                std::thread::spawn(move || worker(&rx, &results, &devices))
            })
            .collect();
        Pool {
            tx: Mutex::new(Some(tx)),
            seen: Mutex::new(HashSet::new()),
            repeats: Mutex::new(0),
            results,
            workers: Mutex::new(workers),
            deadline_ms,
        }
    }

    /// Queues the image unless it was already enriched in this run (a cache hit in the agent).
    pub fn submit(&self, nt_path: String, event_ts: i64) {
        if !self.seen.lock().expect("seen").insert(nt_path.to_ascii_lowercase()) {
            *self.repeats.lock().expect("repeats") += 1;
            return;
        }
        if let Some(tx) = self.tx.lock().expect("tx").as_ref() {
            let _ = tx.send(Job { nt_path, event_ts });
        }
    }

    /// Waits for queued work, then summarizes.
    pub fn finish(&self) -> Value {
        self.tx.lock().expect("tx").take();
        for w in self.workers.lock().expect("workers").drain(..) {
            let _ = w.join();
        }
        let r = self.results.lock().expect("results");
        let mut total: Vec<f64> = r.iter().map(|o| o.total_ms).collect();
        let mut since: Vec<f64> = r.iter().map(|o| o.since_event_ms).collect();
        total.sort_by(f64::total_cmp);
        since.sort_by(f64::total_cmp);
        let over = |ms: f64| total.iter().filter(|&&t| t > ms).count();
        let mut sigs = std::collections::BTreeMap::<String, u64>::new();
        for o in r.iter() {
            *sigs.entry(o.signature.clone()).or_default() += 1;
        }
        json!({
            "first_launches": r.len(),
            "repeat_launches": *self.repeats.lock().expect("repeats"),
            "deadline_ms": self.deadline_ms,
            "over_deadline": over(self.deadline_ms as f64),
            "over_500ms": over(500.0),
            "over_2000ms": over(2000.0),
            "total_ms": { "p50": percentile(&total, 50.0), "p95": percentile(&total, 95.0),
                          "p99": percentile(&total, 99.0), "max": total.last() },
            "since_event_ms": { "p50": percentile(&since, 50.0), "p95": percentile(&since, 95.0),
                                "max": since.last() },
            "signatures": sigs,
            "items": r.iter().map(|o| json!({
                "path": o.path, "size": o.size, "hash_ms": o.hash_ms, "verify_ms": o.verify_ms,
                "total_ms": o.total_ms, "since_event_ms": o.since_event_ms,
                "signature": o.signature, "error": o.error,
            })).collect::<Vec<_>>(),
        })
    }
}

fn worker(rx: &Mutex<Receiver<Job>>, results: &Mutex<Vec<Outcome>>, devices: &[(String, String)]) {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    let mut cat_admin = 0isize;
    let sha256 = w!("SHA256");
    let have_admin =
        unsafe { CryptCATAdminAcquireContext2(&mut cat_admin, Some(&DRIVER_ACTION_VERIFY), sha256, None, None) }
            .is_ok();
    loop {
        let job = match rx.lock().expect("rx").recv() {
            Ok(j) => j,
            Err(_) => break,
        };
        let start = Instant::now();
        let path = to_dos_path(&job.nt_path, devices);
        let (size, hash_ms, mut error) = match hash_file(&path) {
            Ok((s, ms)) => (Some(s), ms, None),
            Err(e) => (None, 0.0, Some(e)),
        };
        let t = Instant::now();
        let signature = if error.is_none() {
            match verify(&path, have_admin.then_some(cat_admin)) {
                Ok(s) => s,
                Err(e) => {
                    error = Some(e);
                    "error".into()
                }
            }
        } else {
            "skipped".into()
        };
        let verify_ms = t.elapsed().as_secs_f64() * 1e3;
        results.lock().expect("results").push(Outcome {
            path,
            size,
            hash_ms,
            verify_ms,
            total_ms: start.elapsed().as_secs_f64() * 1e3,
            since_event_ms: qpc_to_ms(qpc_now() - job.event_ts),
            signature,
            error,
        });
    }
    if have_admin {
        unsafe {
            let _ = CryptCATAdminReleaseContext(cat_admin, 0);
        }
    }
}

/// `\Device\HarddiskVolumeN` → `C:` for every drive letter, longest device name first.
fn device_map() -> Vec<(String, String)> {
    let mut map = Vec::new();
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:", letter as char);
        let mut buf = [0u16; 1024];
        let n = unsafe { QueryDosDeviceW(PCWSTR(wide(&drive).as_ptr()), Some(&mut buf)) };
        if n > 0 {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
            map.push((String::from_utf16_lossy(&buf[..len]), drive));
        }
    }
    map.sort_by_key(|(dev, _)| std::cmp::Reverse(dev.len()));
    map
}

fn to_dos_path(nt: &str, devices: &[(String, String)]) -> String {
    for (dev, drive) in devices {
        if let Some(rest) = nt.strip_prefix(dev.as_str())
            && (rest.is_empty() || rest.starts_with('\\'))
        {
            return format!("{drive}{rest}");
        }
    }
    nt.strip_prefix(r"\??\").unwrap_or(nt).to_string()
}

fn hash_file(path: &str) -> Result<(u64, f64), String> {
    let t = Instant::now();
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(7) // FILE_SHARE_READ | WRITE | DELETE
        .open(path)
        .map_err(|e| format!("open: {e}"))?;
    let size = f.metadata().map_err(|e| e.to_string())?.len();
    if size > SIZE_CAP {
        return Ok((size, 0.0));
    }
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let _digest = h.finalize();
    Ok((size, t.elapsed().as_secs_f64() * 1e3))
}

fn trust(data: &mut WINTRUST_DATA) -> i32 {
    let mut action: GUID = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    let r = unsafe { WinVerifyTrust(HWND::default(), &mut action, (data as *mut WINTRUST_DATA).cast()) };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe { WinVerifyTrust(HWND::default(), &mut action, (data as *mut WINTRUST_DATA).cast()) };
    r
}

fn base_data() -> WINTRUST_DATA {
    WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE,
        ..Default::default()
    }
}

/// "valid-embedded" | "valid-catalog" | "unsigned" | "invalid:<hresult>"
fn verify(path: &str, cat_admin: Option<isize>) -> Result<String, String> {
    let path_w = wide(path);
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(path_w.as_ptr()),
        ..Default::default()
    };
    let mut data = base_data();
    data.dwUnionChoice = WTD_CHOICE_FILE;
    data.Anonymous.pFile = &mut file;
    let r = trust(&mut data);
    if r == 0 {
        return Ok("valid-embedded".into());
    }
    if r != TRUST_E_NOSIGNATURE && r != TRUST_E_SUBJECT_FORM_UNKNOWN {
        return Ok(format!("invalid:{:#010x}", r as u32));
    }
    let Some(admin) = cat_admin else {
        return Ok("unsigned(no-catalog-context)".into());
    };
    // Catalog lookup, as for catalog-signed OS files.
    let f = std::fs::OpenOptions::new().read(true).share_mode(7).open(path).map_err(|e| format!("open: {e}"))?;
    let hfile = HANDLE(f.as_raw_handle());
    let mut cb = 0u32;
    unsafe { CryptCATAdminCalcHashFromFileHandle2(admin, hfile, &mut cb, None, None) }
        .or_else(|e| if cb > 0 { Ok(()) } else { Err(e) })
        .map_err(|e| format!("hash size: {e}"))?;
    let mut hash = vec![0u8; cb as usize];
    unsafe { CryptCATAdminCalcHashFromFileHandle2(admin, hfile, &mut cb, Some(hash.as_mut_ptr()), None) }
        .map_err(|e| format!("catalog hash: {e}"))?;
    let cat = unsafe { CryptCATAdminEnumCatalogFromHash(admin, &hash, None, None) };
    if cat == 0 {
        return Ok("unsigned".into());
    }
    let mut info = CATALOG_INFO { cbStruct: size_of::<CATALOG_INFO>() as u32, ..Default::default() };
    let res = unsafe { CryptCATCatalogInfoFromContext(cat, &mut info, 0) };
    let out = match res {
        Err(e) => Err(format!("catalog info: {e}")),
        Ok(()) => {
            let tag = wide(&hash.iter().map(|b| format!("{b:02X}")).collect::<String>());
            let mut cinfo = WINTRUST_CATALOG_INFO {
                cbStruct: size_of::<WINTRUST_CATALOG_INFO>() as u32,
                pcwszCatalogFilePath: PCWSTR(info.wszCatalogFile.as_ptr()),
                pcwszMemberTag: PCWSTR(tag.as_ptr()),
                pcwszMemberFilePath: PCWSTR(path_w.as_ptr()),
                hMemberFile: hfile,
                pbCalculatedFileHash: hash.as_mut_ptr(),
                cbCalculatedFileHash: cb,
                hCatAdmin: admin,
                ..Default::default()
            };
            let mut data = base_data();
            data.dwUnionChoice = WTD_CHOICE_CATALOG;
            data.Anonymous.pCatalog = &mut cinfo;
            let r = trust(&mut data);
            Ok(if r == 0 { "valid-catalog".into() } else { format!("invalid-catalog:{:#010x}", r as u32) })
        }
    };
    unsafe {
        let _ = CryptCATAdminReleaseCatalogContext(admin, cat, 0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dos_path_maps_on_component_boundary_only() {
        let devices = vec![
            (r"\Device\HarddiskVolume10".to_string(), "D:".to_string()),
            (r"\Device\HarddiskVolume1".to_string(), "C:".to_string()),
        ];
        assert_eq!(to_dos_path(r"\Device\HarddiskVolume1\x.exe", &devices), r"C:\x.exe");
        assert_eq!(to_dos_path(r"\Device\HarddiskVolume10\x.exe", &devices), r"D:\x.exe");
        assert_eq!(to_dos_path(r"\Device\HarddiskVolume12\x.exe", &devices), r"\Device\HarddiskVolume12\x.exe");
    }

    #[test]
    fn verify_classifies_os_and_unsigned_files() {
        let mut admin = 0isize;
        let ok =
            unsafe { CryptCATAdminAcquireContext2(&mut admin, Some(&DRIVER_ACTION_VERIFY), w!("SHA256"), None, None) }
                .is_ok();
        let notepad = verify(r"C:\Windows\System32\notepad.exe", ok.then_some(admin)).unwrap();
        assert!(notepad.starts_with("valid"), "notepad: {notepad}");
        let me = std::env::current_exe().unwrap();
        assert_eq!(verify(&me.to_string_lossy(), ok.then_some(admin)).unwrap(), "unsigned");
    }
}
```

### `src/report.rs`

```rust
//! Lines up a trace's events with the action steps that caused them (by QPC window) and prints
//! Markdown, so each S3/S4/S6/S7 question reads off one table per step.

use crate::util::{Result, ms_to_qpc, qpc_to_ms};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

fn read_jsonl(p: &Path) -> Result<Vec<Value>> {
    let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect())
}

fn short(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if s.chars().count() > 120 { format!("{}…", s.chars().take(120).collect::<String>()) } else { s }
}

fn describe(e: &Value) -> String {
    let fields = e["fields"]
        .as_object()
        .map(|m| m.iter().map(|(k, v)| format!("{k}={}", short(v))).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    let err = e["decode_error"].as_str().map(|d| format!(" **decode error: {d}**")).unwrap_or_default();
    format!(
        "{}/{} v{} {}{} (pid {}, key {}): {fields}{err}",
        e["provider"].as_str().unwrap_or("?"),
        e["id"],
        e["version"],
        e["task_name"].as_str().unwrap_or(""),
        e["opcode_name"].as_str().map(|o| format!("/{o}")).unwrap_or_default(),
        e["pid"],
        e["start_key"].as_str().unwrap_or("-"),
    )
}

pub fn run(events: &Path, actions: &Path, slack_ms: u64) -> Result<()> {
    let evs = read_jsonl(events)?;
    let steps: Vec<Value> = read_jsonl(actions)?.into_iter().filter(|a| a.get("step").is_some()).collect();
    let slack = ms_to_qpc(slack_ms);
    let mut used = vec![false; evs.len()];
    println!("# Report: {}\n", events.display());
    for s in &steps {
        let (b, a) = (s["qpc_before"].as_i64().unwrap_or(0), s["qpc_after"].as_i64().unwrap_or(0) + slack);
        let status =
            if s["ok"].as_bool() == Some(true) { "ok".to_string() } else { format!("FAILED: {}", short(&s["error"])) };
        println!("## {} `{}` ({status})\n", s["step"], s["name"].as_str().unwrap_or("?"));
        if !s["info"].is_null() {
            println!("info: `{}`\n", s["info"]);
        }
        let mut n = 0;
        for (i, e) in evs.iter().enumerate() {
            let ts = e["ts"].as_i64().unwrap_or(0);
            if ts >= b && ts <= a {
                used[i] = true;
                n += 1;
                println!("- +{:.3} ms {}", qpc_to_ms(ts - b), describe(e));
            }
        }
        if n == 0 {
            println!("- (no events)");
        }
        println!();
    }
    let mut outside: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for (i, e) in evs.iter().enumerate() {
        if !used[i] {
            outside.entry(format!("{}/{}", e["provider"].as_str().unwrap_or("?"), e["id"])).or_default().push(e);
        }
    }
    println!("## Outside every step\n");
    for (k, list) in &outside {
        println!("- {k}: {} event(s); first: {}", list.len(), describe(list[0]));
    }
    Ok(())
}
```
