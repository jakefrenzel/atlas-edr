@{
    Severity            = @('Error', 'Warning')
    IncludeDefaultRules = $true
    Rules               = @{
        # The guest script and the module it imports run on a fresh Windows install, which only has Windows
        # PowerShell 5.1. Host-only code must parse there too, because it lives in the same module.
        PSUseCompatibleSyntax = @{
            Enable         = $true
            TargetVersions = @('5.1', '7.0')
        }
    }
}
