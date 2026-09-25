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

Describe 'Wrapper scripts' {
    It '<Script> forwards its arguments and -WhatIf to <Function>' -TestCases @(
        @{ Script = 'New-EdrTestVm.ps1'; Function = 'New-EdrTestVm'; Params = @{ IsoPath = 'C:\eval.iso' } }
        @{ Script = 'Complete-EdrTestVmInstall.ps1'; Function = 'Complete-EdrTestVmInstall'; Params = @{} }
        @{ Script = 'Set-EdrTestNetwork.ps1'; Function = 'Set-EdrTestNetwork'; Params = @{ Mode = 'Isolated' } }
        @{ Script = 'Copy-ToEdrTestVm.ps1'; Function = 'Copy-ToEdrTestVm'; Params = @{ Path = @('C:\a', 'C:\b') } }
        @{ Script = 'Reset-EdrTestVm.ps1'; Function = 'Reset-EdrTestVm'; Params = @{ Checkpoint = 'pre-driver' } }
        @{ Script = 'guest\Initialize-EdrTestGuest.ps1'; Function = 'Initialize-EdrTestGuest'; Params = @{} }
    ) {
        Mock Import-Module {}
        Mock $Function {}
        & (Join-Path $PSScriptRoot "..\$Script") @Params -WhatIf
        Should -Invoke $Function -Times 1 -Exactly -ParameterFilter {
            $ok = $PesterBoundParameters.ContainsKey('WhatIf')
            foreach ($k in $Params.Keys) {
                $ok = $ok -and ("$($PesterBoundParameters[$k])" -eq "$($Params[$k])")
            }
            $ok
        }
        Should -Invoke Import-Module -Times 1 -Exactly -ParameterFilter {
            $Name -like '*EdrTestVm.psm1' -and (Test-Path -LiteralPath $Name)
        }
    }
}
