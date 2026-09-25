<#
.SYNOPSIS
Isolated removes the NAT NIC; Online adds it. The internal NIC is never touched.
.NOTES
Runs on the host, elevated. Supports -WhatIf. Runbook: docs/runbooks/edr-test-vm.md
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSShouldProcess', '', Justification = 'Delegates to the module function, which calls ShouldProcess.')]
[CmdletBinding(SupportsShouldProcess)]
param([Parameter(Mandatory)][ValidateSet('Isolated', 'Online')][string]$Mode)

Import-Module (Join-Path $PSScriptRoot 'EdrTestVm.psm1') -Force
Set-EdrTestNetwork @PSBoundParameters
