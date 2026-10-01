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
   - **The "Press any key to boot from CD or DVD" prompt lasts only about 5 seconds.** Click Start, then immediately click inside the VM screen and tap Space until Setup loads. If you miss it, the VM tries the empty disk and then a network boot for about a minute, then shows "No operating system was loaded" (Hyper-V logs event 18603, "failed to boot an operating system"). Nothing is broken: use **Action → Reset** (Ctrl+R) and try again.
   - Choose "I don't have a product key" if asked (the eval needs none).
   - At the Microsoft account screen, pick **Sign-in options → Domain join instead** to create a **local** account.
   - Decline all optional diagnostics and experience settings.
   - Finish setup, sign in once, then **shut down from inside Windows**.
4. **Finish the install** (host, with the VM off). This turns Secure Boot off, ejects the ISO and boots from the disk:
   ```powershell
   .\infra\vm\Complete-EdrTestVmInstall.ps1
   Start-VM edr-test
   ```
5. **Turn off Tamper Protection** (**inside the VM**, manual): Windows Security → Virus & threat protection → Manage settings → Tamper Protection **Off**. This is the one setting a script cannot change.
6. **Go online** (host):
   ```powershell
   .\infra\vm\Set-EdrTestNetwork.ps1 -Mode Online
   ```
   If `winget` isn't available **inside the VM** yet, open Microsoft Store in the VM and update "App Installer". (Running `winget` on the host tells you nothing about the VM; the guest script checks for it itself.)
7. **Copy the guest script in** (host):
   ```powershell
   .\infra\vm\Copy-ToEdrTestVm.ps1 -Path .\infra\vm\EdrTestVm.psm1, .\infra\vm\guest\Initialize-EdrTestGuest.ps1
   ```
8. **Initialize the guest**: **inside the VM, never on the host.** In the VM's Start menu, right-click **Windows PowerShell** → **Run as administrator**, then:
   ```powershell
   powershell -ExecutionPolicy Bypass -File C:\atlas\Initialize-EdrTestGuest.ps1
   ```
   It checks its preconditions first, lists everything unmet, and changes nothing until all are met. The first check refuses to run anywhere except a Hyper-V VM, because the script turns off Defender real-time protection, HVCI and driver-signature enforcement.
   Packages install from the `winget` source only. On a fresh install the Microsoft Store source (`msstore`) can fail with a certificate error (`0x8A15005E`); that no longer matters.
   **Save the printed `windbg -k net:...` line** in your password manager; it holds the KDNET key. The script is safe to re-run, and a re-run keeps the same key. One exception: if, after the reboot, KDNET has replaced the internal NIC (checklist item 10), a re-run stops at "No NIC named 'edr-internal'".
9. **Go isolated, then restart** (host). The order matters. The baseline checkpoint includes memory, so every restore resumes the kernel from this boot. That kernel must have booted with only `edr-internal` present, so KDNET binds to that NIC and not to one that is later removed:
   ```powershell
   .\infra\vm\Set-EdrTestNetwork.ps1 -Mode Isolated
   Restart-VM -Name edr-test -Force
   ```
   Wait until the guest reaches the sign-in screen.
10. **Re-apply the internal address** (**inside the VM**, Administrator Windows PowerShell). KDNET took over the internal NIC during that boot: Windows now shows only "Ethernet (Kernel Debugger)" (Microsoft Kernel Debug Network Adapter), which still carries normal traffic but has lost the static address. Copy the latest guest files in first if they changed (step 7), then:
    ```powershell
    powershell -ExecutionPolicy Bypass -File C:\atlas\Initialize-EdrTestGuest.ps1 -NetworkOnly
    ```
    It only sets `192.168.77.10` on that adapter, and warns if Defender real-time protection is still on. If it warns, stop and report acceptance item 9 before taking the baseline.
11. **Take the baseline** (host). If a `baseline` already exists, remove it first (`Remove-VMCheckpoint -VMName edr-test -Name baseline`), because two checkpoints with the same name make the restore ambiguous:
    ```powershell
    Checkpoint-VM -Name edr-test -SnapshotName baseline
    ```
12. Run the **acceptance checklist** (§6).

## 2. Daily loop

This loop needs a built agent (`cargo build --release -p atlas-agent`, from sub-project 1 onward). Until then, skip it.

```powershell
.\infra\vm\Reset-EdrTestVm.ps1                                   # restore baseline and start
.\infra\vm\Copy-ToEdrTestVm.ps1 -Path .\target\release\atlas-agent.exe
# ... test inside the VM ...
.\infra\vm\Reset-EdrTestVm.ps1                                   # throw the state away
```

Attack simulations (e.g. Atomic Red Team) run **only in Isolated mode**. If a test needs the internet, go Online, download what you need, go Isolated again, then run the test.

## 3. Kernel debugging (KDNET)

WinDbg must be installed **on the host** (one-time): `winget install Microsoft.WinDbg --source winget`.

1. On the host: start WinDbg with the saved line (`windbg -k net:port=50000,key=<key>`) and wait for "Waiting to reconnect...".
2. Restart the guest. WinDbg connects during boot; press **Break** (Ctrl+Break) to stop in the debugger.
3. The first time, allow WinDbg through the host firewall if prompted. The `Atlas-EdrTest-KDNET` rule already opens UDP 50000 on `vEthernet (edr-internal)` only.

Debug in **Isolated** mode. KDNET binds to a NIC at boot. With only `edr-internal` present the binding is unambiguous. Acceptance item 6 records whether it also works after a reboot in Online mode.

Enhanced session can time out while the guest is stopped at a breakpoint; use a basic session (View → Enhanced session off) while debugging.

## 4. The 90-day rebuild

The evaluation expires after 90 days. Rebuilding also tests that the scripts still work.

By now the VM runs from a checkpoint's differencing disk (`edr-test_<GUID>.avhdx`), so delete the base disk and every differencing disk by name, not by the VM's current disk path:

```powershell
Stop-VM edr-test -TurnOff
Remove-VM edr-test -Force
$dir = (Get-VMHost).VirtualHardDiskPath
Get-ChildItem $dir -Filter 'edr-test*.*vhdx' | Remove-Item
```

Check that `Get-ChildItem $dir -Filter 'edr-test*'` prints nothing. Then repeat §1 from step 2. The switch, host address and firewall rule are reused. `New-EdrTestVm` refuses to run while an old `edr-test.vhdx` is still present.

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
| 9 | Protections off | Guest (admin), after a restore: `Get-MpComputerStatus \| Select RealTimeProtectionEnabled, IsTamperProtected`; `(Get-CimInstance -Namespace root\Microsoft\Windows\DeviceGuard Win32_DeviceGuard).SecurityServicesRunning` | `False`, `False`; and the list has no `2` (HVCI not running) |
| 10 | Internal NIC after KDNET | Guest: `Get-NetAdapter \| Format-Table Name, InterfaceDescription`; `Get-NetIPAddress -IPAddress 192.168.77.10` | `192.168.77.10` is on "Ethernet (Kernel Debugger)" (Microsoft Kernel Debug Network Adapter). If the address is missing, step 10 (`-NetworkOnly`) was skipped |

If item 9 shows real-time protection back on, Defender reverted the setting. Since 2026-10-01 the guest script also sets the `DisableRealtimeMonitoring` **policy** value, which Defender honours while Tamper Protection is off. If it still comes back, record it; fixes go through a script change, not a manual edit.

### Results

| Date | Build | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 2026-10-01 | Win11 Enterprise Eval, kernel 26100.1 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ connects | ✅ | ✅ | ✅ | ✅ | First build. 9 and 10 **failed** on the first pass (Defender real-time protection came back; KDNET replaced the internal NIC and the static IP was lost). Both were fixed in the scripts (0b spec §8) and passed after re-running setup. Item 6: online, WinDbg reconnects at boot and KDNET still takes the **internal** NIC: `192.168.77.10` stays on "Ethernet (Kernel Debugger)", and `edr-online` is a normal adapter with a Default Switch DHCP address. KDNET's own address is a self-assigned 169.254.x.x, which is harmless. |
