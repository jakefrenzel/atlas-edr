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
        { Set-EdrTestNetwork -Mode Bridged } | Should -Throw -ErrorId 'ParameterArgumentValidationError,Set-EdrTestNetwork'
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
