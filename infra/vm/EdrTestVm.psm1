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
