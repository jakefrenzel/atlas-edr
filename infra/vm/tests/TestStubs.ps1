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
    # Registry writes (Write-EdrRegistryDword)
    'New-ItemProperty'            = @('LiteralPath', 'Name', 'Value', 'PropertyType', '[switch]Force')
    # Programs the module runs. A function named 'x.exe' outranks the application, and takes any arguments.
    'bcdedit.exe'                 = $null
    'winget.exe'                  = $null
}

function Register-EdrTestStub {
    foreach ($name in $script:EdrTestStubs.Keys) {
        if ($null -eq $script:EdrTestStubs[$name]) {
            $body = "throw 'Unmocked call to $name'"
            Set-Item -Path "function:global:$name" -Value ([scriptblock]::Create($body))
            continue
        }
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
