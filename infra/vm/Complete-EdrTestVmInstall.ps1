<#
.SYNOPSIS
After the manual Windows install: turns Secure Boot off, ejects the ISO, boots from disk.
.NOTES
Runs on the host, elevated. Supports -WhatIf. Runbook: docs/runbooks/edr-test-vm.md
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSShouldProcess', '', Justification = 'Delegates to the module function, which calls ShouldProcess.')]
[CmdletBinding(SupportsShouldProcess)]
param()

Import-Module (Join-Path $PSScriptRoot 'EdrTestVm.psm1') -Force
Complete-EdrTestVmInstall @PSBoundParameters
