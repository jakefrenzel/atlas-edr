<#
.SYNOPSIS
Creates the edr-test VM, the edr-internal switch, the host address and the KDNET firewall rule.
.NOTES
Runs on the host, elevated. Supports -WhatIf. Runbook: docs/runbooks/edr-test-vm.md
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSShouldProcess', '', Justification = 'Delegates to the module function, which calls ShouldProcess.')]
[CmdletBinding(SupportsShouldProcess)]
param([Parameter(Mandatory)][string]$IsoPath)

Import-Module (Join-Path $PSScriptRoot 'EdrTestVm.psm1') -Force
New-EdrTestVm @PSBoundParameters
