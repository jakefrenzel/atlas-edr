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

Export-ModuleMember -Function @(
    'Get-EdrTestVmConfig'
    'New-EdrTestVm'
)
