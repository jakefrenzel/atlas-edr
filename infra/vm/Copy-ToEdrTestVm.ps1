<#
.SYNOPSIS
Copies files into the guest's C:\atlas\ over VMBus (works while isolated).
.NOTES
Runs on the host, elevated. Supports -WhatIf. Runbook: docs/runbooks/edr-test-vm.md
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSShouldProcess', '', Justification = 'Delegates to the module function, which calls ShouldProcess.')]
[CmdletBinding(SupportsShouldProcess)]
param([Parameter(Mandatory)][string[]]$Path)

Import-Module (Join-Path $PSScriptRoot 'EdrTestVm.psm1') -Force
Copy-ToEdrTestVm @PSBoundParameters
