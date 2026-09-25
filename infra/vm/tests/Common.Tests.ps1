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

Describe 'Get-EdrTestVmConfig' {
    It 'holds the values from spec section 4.1' {
        $c = Get-EdrTestVmConfig
        $c.VmName | Should -Be 'edr-test'
        $c.InternalSwitch | Should -Be 'edr-internal'
        $c.InternalAdapter | Should -Be 'edr-internal'
        $c.OnlineAdapter | Should -Be 'edr-online'
        $c.OnlineSwitch | Should -Be 'Default Switch'
        $c.HostIp | Should -Be '192.168.77.1'
        $c.GuestIp | Should -Be '192.168.77.10'
        $c.PrefixLength | Should -Be 24
        $c.KdnetPort | Should -Be 50000
        $c.GuestDropPath | Should -Be 'C:\atlas'
        $c.BaselineCheckpoint | Should -Be 'baseline'
        $c.ProcessorCount | Should -Be 4
        $c.MemoryBytes | Should -Be 8GB
        $c.VhdSizeBytes | Should -Be 80GB
        $c.WingetPackages | Should -Be @('Microsoft.WinDbg', 'Microsoft.Sysinternals.Suite')
    }
}

Describe 'Assert-EdrTestVmName' {
    It 'accepts edr-test' {
        InModuleScope EdrTestVm { { Assert-EdrTestVmName -Name 'edr-test' } | Should -Not -Throw }
    }
    It 'refuses <Name>' -TestCases @(
        @{ Name = 'prod-dc' }
        @{ Name = 'edr-test-2' }
        @{ Name = '' }
    ) {
        InModuleScope EdrTestVm -Parameters @{ Name = $Name } {
            { Assert-EdrTestVmName -Name $Name } | Should -Throw '*Refusing*'
        }
    }
}

Describe 'Assert-EdrElevated' {
    It 'throws when not elevated' {
        Mock -ModuleName EdrTestVm Test-EdrElevated { $false }
        InModuleScope EdrTestVm { { Assert-EdrElevated } | Should -Throw '*elevated*' }
    }
    It 'passes when elevated' {
        Mock -ModuleName EdrTestVm Test-EdrElevated { $true }
        InModuleScope EdrTestVm { { Assert-EdrElevated } | Should -Not -Throw }
    }
}

Describe 'Test stubs' {
    It 'make an unmocked Hyper-V call fail loudly instead of reaching the real host' {
        InModuleScope EdrTestVm { { Get-VM -Name 'edr-test' } | Should -Throw 'Unmocked call to Get-VM' }
    }
}

Describe 'Windows PowerShell 5.1 compatibility' {
    # The guest runs the module under Windows PowerShell 5.1, which reads BOM-less files as ANSI.
    It '<File> is ASCII-only' -TestCases @(
        Get-ChildItem -Path (Join-Path $PSScriptRoot '..') -Recurse -File -Include '*.ps1', '*.psm1', '*.psd1' |
            ForEach-Object { @{ File = $_.Name; FullName = $_.FullName } }
    ) {
        $bytes = [IO.File]::ReadAllBytes($FullName)
        @($bytes | Where-Object { $_ -gt 0x7F }).Count | Should -Be 0
    }

    It 'the module imports under powershell.exe' -Skip:(-not (Get-Command powershell.exe -ErrorAction SilentlyContinue)) {
        $module = Join-Path $PSScriptRoot '..\EdrTestVm.psm1'
        $out = & powershell.exe -NoProfile -NonInteractive -Command "Import-Module '$module'; (Get-EdrTestVmConfig).VmName"
        $LASTEXITCODE | Should -Be 0
        $out | Should -Be 'edr-test'
    }
}
