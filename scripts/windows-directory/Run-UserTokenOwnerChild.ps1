# Internal fixed-target child; there is no switch that bypasses owner or probe gates.
[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$ConfigurationPath)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
# Capture child/bootstrap diagnostics to disk; CreateProcessWithTokenW console
# inheritance is deliberately not relied upon. Native target output is also
# explicitly piped to Tee-Object below and replayed by the parent.
$bootstrapLog = Join-Path (Split-Path -Parent ([IO.Path]::GetFullPath($ConfigurationPath))) 'child-bootstrap-error.log'
trap {
    ($_ | Out-String) | Add-Content -LiteralPath $bootstrapLog -Encoding utf8NoBOM
    exit 1
}
if ($env:OS -ne 'Windows_NT' -or $PSVersionTable.PSEdition -cne 'Core') { throw 'Native Windows PowerShell 7 required' }
$config = Get-Content -LiteralPath $ConfigurationPath -Raw | ConvertFrom-Json -AsHashtable
if ($config.schemaVersion -ne 1 -or $config.attemptId -cnotmatch '^[0-9a-f]{32}$') { throw 'Invalid owner fixture configuration' }
$evidence = (Resolve-Path -LiteralPath $config.evidenceDirectory).Path
if ((Split-Path -Leaf $evidence) -cne $config.attemptId -or [IO.Path]::GetFullPath($ConfigurationPath) -cne (Join-Path $evidence 'configuration.json')) { throw 'Owner fixture evidence path mismatch' }
$state = @{attemptId=$config.attemptId;result='inProgress';processId=$PID;probeVerified=$false;probeRemoved=$false;targetStarted=$false;targetExitCode=$null}
$probe = $null; $probeCreated = $false; $exitCode = 1
function Save-ChildEvidence {
    $state | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $evidence 'child.json') -Encoding utf8NoBOM
}
function Convert-TokenSnapshot([hashtable]$value) {
    $snapshot = [Polaris.WindowsAcceptance.TokenSnapshot]::new()
    $snapshot.User = $value.User; $snapshot.Owner = $value.Owner
    $snapshot.TokenId = $value.TokenId; $snapshot.TokenType = $value.TokenType
    return $snapshot
}
try {
    Save-ChildEvidence
    $native = Join-Path $PSScriptRoot 'UserTokenOwnerNative.cs'
    $target = Join-Path $PSScriptRoot 'Build-And-Test.ps1'
    foreach ($entry in @(@($native,'nativeSHA256'),@($PSCommandPath,'workerSHA256'),@($target,'targetSHA256'))) {
        if ((Get-FileHash -LiteralPath $entry[0] -Algorithm SHA256).Hash.ToLowerInvariant() -cne $config[$entry[1]]) { throw 'Fixed acceptance source hash changed before child execution' }
    }
    Add-Type -Path $native
    $current = [Polaris.WindowsAcceptance.Native]::Current()
    $state.actualChildToken = $current
    [Polaris.WindowsAcceptance.Native]::AssertPrivate((Convert-TokenSnapshot $config.originalToken),$current)
    [Polaris.WindowsAcceptance.Native]::AssertUnchanged((Convert-TokenSnapshot $config.actualChildToken),$current)
    if ($PID -ne $config.childProcessId) { throw 'Actual child PID mismatch' }
    Save-ChildEvidence
    $probe = Join-Path ([IO.Path]::GetTempPath()) ('PolarisOwnerProbe-' + [guid]::NewGuid().ToString('N'))
    $state.probePath = $probe
    [Polaris.WindowsAcceptance.Native]::CreateOwnerProbe($probe)
    $probeCreated = $true
    $state.probeOwner = [Polaris.WindowsAcceptance.Native]::ReadDirectoryOwner($probe)
    if ($state.probeOwner -cne $current.User) { throw 'Fresh TEMP directory owner does not match TokenUser' }
    $state.probeVerified = $true
    # Cleanup is part of the preflight: never repair ACLs or leave a probe behind.
    [IO.Directory]::Delete($probe,$false)
    $probeCreated = $false; $state.probeRemoved = $true
    Save-ChildEvidence
    $parameters = $config.parameters
    $keys = @($parameters.Keys | Sort-Object)
    $formal = ($keys -join ',') -ceq 'FormalContract,KernelRoot,OutputDirectory'
    $candidate = ($keys -join ',') -ceq 'CandidateSourceCommit,KernelRoot,OutputDirectory'
    if (-not ($formal -xor $candidate)) { throw 'Only fixed acceptance parameters are permitted' }
    if ($formal -and $parameters.FormalContract -ne $true) { throw 'Formal contract flag must be true' }
    if ($candidate -and $parameters.CandidateSourceCommit -cnotmatch '^[0-9a-f]{40}$') { throw 'Exact candidate source SHA required' }
    if ((Resolve-Path -LiteralPath $parameters.OutputDirectory).Path -cne (Split-Path -Parent (Split-Path -Parent $evidence))) { throw 'Acceptance output and owner evidence directory differ' }
    $state.targetStarted = $true
    Save-ChildEvidence
    $global:LASTEXITCODE = 0
    # First and only test/build entrypoint, after actual-token and directory probes.
    & $target @parameters *>&1 | Tee-Object -FilePath (Join-Path $evidence 'child-output.log')
    $exitCode = $LASTEXITCODE
    $state.targetExitCode = $exitCode
    if ($exitCode -ne 0) { throw "Fixed acceptance target exited $exitCode" }
    $state.result = 'targetPassed'
} catch {
    $state.result = 'failed'; $state.error = "$_"
    if ($state.targetStarted -and $null -ne (Get-Variable LASTEXITCODE -ErrorAction SilentlyContinue) -and $LASTEXITCODE -ne 0) { $exitCode = $LASTEXITCODE }
    elseif ($exitCode -eq 0) { $exitCode = 1 }
    if ($state.targetStarted) { $state.targetExitCode = $exitCode }
    ($_ | Out-String) | Add-Content -LiteralPath $bootstrapLog -Encoding utf8NoBOM
    Write-Error -Message "Acceptance child owner/target failure: $_" -ErrorAction Continue
} finally {
    try {
        if ($probeCreated) { [IO.Directory]::Delete($probe,$false); $state.probeRemoved = $true }
    } catch { $state.result = 'failed'; $state.cleanupError = "$_"; $exitCode = 1 }
    Save-ChildEvidence
}
exit $exitCode
