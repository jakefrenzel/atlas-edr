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
