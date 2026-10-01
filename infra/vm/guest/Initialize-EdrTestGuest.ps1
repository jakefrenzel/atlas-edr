<#
.SYNOPSIS
One-time guest setup for edr-test: static IP, test signing, KDNET, HVCI and Defender off, WinDbg + Sysinternals.
.NOTES
Runs INSIDE the VM, as administrator, under Windows PowerShell 5.1. First in Online mode:
  powershell -ExecutionPolicy Bypass -File C:\atlas\Initialize-EdrTestGuest.ps1
then, after the isolated restart (KDNET has replaced the internal NIC by then):
  powershell -ExecutionPolicy Bypass -File C:\atlas\Initialize-EdrTestGuest.ps1 -NetworkOnly
Copy EdrTestVm.psm1 into C:\atlas\ alongside it. Runbook: docs/runbooks/edr-test-vm.md
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSShouldProcess', '', Justification = 'Delegates to the module function, which calls ShouldProcess.')]
[CmdletBinding(SupportsShouldProcess)]
param(
    # Only set the internal address on the adapter KDNET put in place of the internal NIC.
    [switch]$NetworkOnly
)

# In the guest the module sits next to this script; in the repo it is one folder up.
$module = @((Join-Path $PSScriptRoot 'EdrTestVm.psm1'), (Join-Path (Split-Path $PSScriptRoot) 'EdrTestVm.psm1')) |
    Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $module) {
    throw "EdrTestVm.psm1 not found next to this script. Copy it into $PSScriptRoot as well."
}
Import-Module $module -Force
Initialize-EdrTestGuest @PSBoundParameters
