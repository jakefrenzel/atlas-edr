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

Export-ModuleMember -Function @(
    'Get-EdrTestVmConfig'
    'New-EdrTestVm'
    'Complete-EdrTestVmInstall'
    'Set-EdrTestNetwork'
    'Copy-ToEdrTestVm'
    'Reset-EdrTestVm'
    'Initialize-EdrTestGuest'
)
