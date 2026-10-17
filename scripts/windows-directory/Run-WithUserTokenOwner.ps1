# CI-only owner fixture. The original shell token is never opened for mutation.
[CmdletBinding(DefaultParameterSetName='ExplicitModeRequired')]
param(
    [Parameter(Mandatory=$true)][string]$KernelRoot,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [Parameter(Mandatory=$true,ParameterSetName='Formal')][switch]$FormalContract,
    [Parameter(Mandatory=$true,ParameterSetName='Candidate')][string]$CandidateSourceCommit
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:OS -ne 'Windows_NT' -or $PSVersionTable.PSEdition -cne 'Core') { throw 'Native Windows PowerShell 7 required' }
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$outputRoot = (Resolve-Path -LiteralPath $OutputDirectory).Path
$attemptId = [guid]::NewGuid().ToString('N')
$receipt = Join-Path $outputRoot 'validation-receipt.json'
$attemptPointer = Join-Path $outputRoot 'token-owner-attempt.json'
$evidence = Join-Path $outputRoot "token-owner/$attemptId"
New-Item -ItemType Directory -Path $evidence | Out-Null
function Invalidate-ValidationReceipt([string]$reason) {
    if (Test-Path -LiteralPath $receipt) {
        $history = Join-Path $outputRoot 'receipt-history'
        New-Item -ItemType Directory -Force -Path $history | Out-Null
        $hash = (Get-FileHash -LiteralPath $receipt -Algorithm SHA256).Hash.ToLowerInvariant()
        Move-Item -LiteralPath $receipt -Destination (Join-Path $history "${hash}-${attemptId}-token-owner-${reason}.json") -ErrorAction Stop
    }
}
function Save-OwnerAttempt([hashtable]$value) {
    $value | ConvertTo-Json -Depth 64 | Set-Content -LiteralPath (Join-Path $evidence 'parent.json') -Encoding utf8NoBOM
    $value | ConvertTo-Json -Depth 64 | Set-Content -LiteralPath $attemptPointer -Encoding utf8NoBOM
}
function Convert-ExceptionEvidence([System.Exception]$exception, [int]$depth = 0) {
    if ($null -eq $exception) { return $null }
    $item = @{type=$exception.GetType().FullName;message=$exception.Message;hresult=$exception.HResult;children=@()}
    if ($exception -is [System.ComponentModel.Win32Exception]) {
        $item.nativeErrorCode = $exception.NativeErrorCode
        $item.systemMessage = [System.ComponentModel.Win32Exception]::new($exception.NativeErrorCode).Message
    }
    if ($depth -ge 16) { $item.truncated = $true; return $item }
    if ($exception -is [System.AggregateException]) {
        foreach ($inner in $exception.InnerExceptions) { $item.children += ,(Convert-ExceptionEvidence $inner ($depth + 1)) }
    } elseif ($null -ne $exception.InnerException) {
        $item.children += ,(Convert-ExceptionEvidence $exception.InnerException ($depth + 1))
    }
    return $item
}
$state = @{attemptId=$attemptId;result='inProgress';scope='windows-directory-acceptance-token-fixture';evidenceDirectory=$evidence;childExitCode=$null;childFailureEvidence=$null;originalUnchanged=$false;cleanupConfirmed=$false}
$lease = $null; $child = $null; $original = $null; $failure = $null; $exitCode = 1
try {
    # Invalidate the old success pointer BEFORE loading native code or probing.
    Invalidate-ValidationReceipt 'start'
    Save-OwnerAttempt $state
    if ($PSCmdlet.ParameterSetName -eq 'Candidate') {
        if ($CandidateSourceCommit -cnotmatch '^[0-9a-f]{40}$') { throw 'Exact lowercase candidate source SHA required' }
        $parameters = @{KernelRoot=(Resolve-Path -LiteralPath $KernelRoot).Path;OutputDirectory=$outputRoot;CandidateSourceCommit=$CandidateSourceCommit}
    } elseif ($PSCmdlet.ParameterSetName -eq 'Formal' -and $FormalContract) {
        $parameters = @{KernelRoot=(Resolve-Path -LiteralPath $KernelRoot).Path;OutputDirectory=$outputRoot;FormalContract=$true}
    } else { throw 'Select exactly one explicit candidate source or formal contract' }
    $native = Join-Path $PSScriptRoot 'UserTokenOwnerNative.cs'
    $worker = Join-Path $PSScriptRoot 'Run-UserTokenOwnerChild.ps1'
    $target = Join-Path $PSScriptRoot 'Build-And-Test.ps1'
    $executable = Join-Path $PSHOME 'pwsh.exe'
    foreach ($file in @($native,$worker,$target,$executable)) {
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Missing fixed acceptance dependency: $file" }
    }
    Add-Type -Path $native
    $original = [Polaris.WindowsAcceptance.Native]::Current()
    $state.originalToken = $original
    # Names/flags only: no token bytes, credentials, or environment dump.
    $state.originalPrivileges = [Polaris.WindowsAcceptance.Native]::CurrentPrivileges()
    Save-OwnerAttempt $state
    $lease = [Polaris.WindowsAcceptance.PrivateToken]::new()
    [Polaris.WindowsAcceptance.Native]::AssertUnchanged($original,$lease.Original)
    $state.duplicateBefore = $lease.DuplicateBefore
    Save-OwnerAttempt $state
    $lease.Normalize()
    $state.duplicateNormalized = $lease.Normalized
    Save-OwnerAttempt $state
    $configuration = @{
        schemaVersion=1;attemptId=$attemptId;evidenceDirectory=$evidence;parameters=$parameters
        originalToken=$original
        nativeSHA256=(Get-FileHash -LiteralPath $native -Algorithm SHA256).Hash.ToLowerInvariant()
        workerSHA256=(Get-FileHash -LiteralPath $worker -Algorithm SHA256).Hash.ToLowerInvariant()
        targetSHA256=(Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    $configurationPath = Join-Path $evidence 'configuration.json'
    $configuration | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $configurationPath -Encoding utf8NoBOM
    $workingDirectory = (Get-Location).Path
    # Fixed launch objects only; diagnostic read failures never replace launch errors.
    try {
        $state.launchDiagnostics = $lease.ReadLaunchDiagnostics($executable,$worker,$configurationPath,$workingDirectory)
        Save-OwnerAttempt $state
    }
    catch { $state.launchDiagnosticsFailureEvidence = Convert-ExceptionEvidence $_.Exception }
    $child = $lease.Start($executable,$worker,$configurationPath,$workingDirectory)
    $state.suspendedChildToken = $child.Token
    $state.childProcessId = $child.ProcessId
    $configuration.actualChildToken = $child.Token
    $configuration.childProcessId = $child.ProcessId
    $configuration | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $configurationPath -Encoding utf8NoBOM
    Save-OwnerAttempt $state
    $child.Resume()
    # The workflow has an independent job deadline. A private kill-on-close Job
    # owns this process and every ordinary descendant, including Go test children.
    $exitCode = $child.Wait(3600000)
    $state.childExitCode = $exitCode
    if ($exitCode -ne 0) {
        $childFailurePath = Join-Path $evidence 'child.json'
        if (Test-Path -LiteralPath $childFailurePath) { $state.childFailureEvidence = Get-Content -LiteralPath $childFailurePath -Raw }
        throw "Acceptance child failed with exit code $exitCode; $($state.childFailureEvidence)"
    }
    $childEvidence = Get-Content -LiteralPath (Join-Path $evidence 'child.json') -Raw | ConvertFrom-Json
    if ($childEvidence.attemptId -cne $attemptId -or $childEvidence.result -cne 'targetPassed' -or -not $childEvidence.probeVerified -or -not $childEvidence.probeRemoved -or -not $childEvidence.targetStarted) {
        throw 'Missing verified child owner/probe/target evidence'
    }
    if (-not (Test-Path -LiteralPath $receipt -PathType Leaf)) { throw 'Acceptance child did not publish its validation receipt' }
} catch {
    $failure = "$_"
    $state.failureEvidence = Convert-ExceptionEvidence $_.Exception
    if ($exitCode -eq 0) { $exitCode = 1 }
} finally {
    if ($null -ne $lease) { $state.launchPrivileges = $lease.LaunchPrivileges }
    # Start can throw before returning a handle, including during native cleanup.
    # A missing handle cannot prove that constructor-time teardown succeeded.
    try { if ($null -ne $child) { $child.Dispose(); $state.cleanupConfirmed = $true } }
    catch { $state.cleanupFailureEvidence = Convert-ExceptionEvidence $_.Exception; $failure = "${failure}; process cleanup failed: $_"; $exitCode = 1 }
    try {
        if ($null -ne $original) {
            $state.originalAfter = [Polaris.WindowsAcceptance.Native]::Current()
            [Polaris.WindowsAcceptance.Native]::AssertUnchanged($original,$state.originalAfter)
            $state.originalUnchanged = $true
        }
    } catch { $state.readbackFailureEvidence = Convert-ExceptionEvidence $_.Exception; $failure = "${failure}; original token final readback failed: $_"; $exitCode = 1 }
    finally { if ($null -ne $lease) { $lease.Dispose() } }
    if ($null -ne $failure -or -not $state.originalUnchanged -or -not $state.cleanupConfirmed) {
        $state.result = 'failed'; $state.error = $failure
        # Even a late cleanup/readback failure revokes a newly published receipt.
        Invalidate-ValidationReceipt 'failed'
    } else { $state.result = 'passed' }
    try { Save-OwnerAttempt $state }
    catch { Invalidate-ValidationReceipt 'evidence-failed'; throw }
    foreach ($name in @('child-output.log','child-bootstrap-error.log')) {
        $log = Join-Path $evidence $name
        if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log | Write-Output }
    }
    Write-Output "Windows acceptance token evidence: $evidence"
}
if ($state.result -cne 'passed') {
    Write-Error -Message "Private TokenOwner acceptance failed: $failure" -ErrorAction Continue
    exit $exitCode
}
exit 0
