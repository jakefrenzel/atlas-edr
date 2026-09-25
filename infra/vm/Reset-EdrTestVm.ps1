<#
.SYNOPSIS
Restores a checkpoint (default: baseline) and starts the VM.
.NOTES
Runs on the host, elevated. Supports -WhatIf. Runbook: docs/runbooks/edr-test-vm.md
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSShouldProcess', '', Justification = 'Delegates to the module function, which calls ShouldProcess.')]
[CmdletBinding(SupportsShouldProcess)]
param([string]$Checkpoint)

Import-Module (Join-Path $PSScriptRoot 'EdrTestVm.psm1') -Force
Reset-EdrTestVm @PSBoundParameters
