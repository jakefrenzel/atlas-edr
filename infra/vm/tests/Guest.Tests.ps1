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

    It 'tells the user to isolate, then restart, then checkpoint, so the baseline kernel booted with one NIC' {
        $script:said = [System.Collections.Generic.List[string]]::new()
        Mock -ModuleName EdrTestVm Write-Information { $script:said.Add("$MessageData") }

        Initialize-EdrTestGuest

        $isolate = $script:said.FindIndex({ param($l) $l -like '*Set-EdrTestNetwork.ps1 -Mode Isolated*' })
        $restart = $script:said.FindIndex({ param($l) $l -like '*Restart-VM -Name edr-test*' })
        $checkpoint = $script:said.FindIndex({ param($l) $l -like '*Checkpoint-VM -Name edr-test -SnapshotName baseline*' })
        $isolate | Should -BeGreaterOrEqual 0
        $restart | Should -BeGreaterThan $isolate
        $checkpoint | Should -BeGreaterThan $restart
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
