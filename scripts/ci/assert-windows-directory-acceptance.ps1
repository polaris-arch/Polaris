# CI consumer of the current Build-And-Test attempt; never searches receipt-history.
[CmdletBinding(DefaultParameterSetName='NewAttempt')]
param(
    [Parameter(Mandatory=$true)][string]$EvidenceDirectory,
    [Parameter(Mandatory=$true)][string]$ExpectedAppSource,
    [Parameter(Mandatory=$true)][string]$ExpectedKernelSource,
    [Parameter(Mandatory=$true)][ValidateSet('formal','candidate')][string]$ValidationMode,
    [Parameter(Mandatory=$true,ParameterSetName='NewAttempt')][AllowEmptyString()][string]$PreviousAttemptId,
    [Parameter(Mandatory=$true,ParameterSetName='BoundAttempt')][string]$ExpectedAttemptId,
    [Parameter(Mandatory=$true,ParameterSetName='BoundAttempt')][string]$ExpectedReceiptSHA256,
    [Parameter(Mandatory=$true,ParameterSetName='BoundAttempt')][string]$ExpectedAppManifestSHA256,
    [Parameter(Mandatory=$true,ParameterSetName='BoundAttempt')][string]$ExpectedKernelManifestSHA256,
    [Parameter(Mandatory=$true,ParameterSetName='BoundAttempt')][string]$ExpectedActivationManifestSHA256,
    [Parameter(ParameterSetName='BoundAttempt')][string]$SystemEvidenceDirectory,
    [Parameter(ParameterSetName='BoundAttempt')][AllowEmptyString()][string]$PreviousSystemNonce,
    [Parameter(ParameterSetName='BoundAttempt')][switch]$PrebuildUserDomain
)
$ErrorActionPreference = 'Stop'
$boundAttempt = $PSCmdlet.ParameterSetName -eq 'BoundAttempt'
foreach ($source in @($ExpectedAppSource,$ExpectedKernelSource)) {
    if ($source -cnotmatch '^[0-9a-f]{40}$') { throw 'Exact source identity required' }
}
$attemptPath = Join-Path $EvidenceDirectory 'validation-attempt.json'
$receiptPath = Join-Path $EvidenceDirectory 'validation-receipt.json'
$attempt = Get-Content -LiteralPath $attemptPath -Raw | ConvertFrom-Json
if ($attempt.result -cne 'passed' -or $attempt.attemptId -cnotmatch '^[0-9a-f]{32}$' -or
    $attempt.receipt -cne 'validation-receipt.json' -or $attempt.receiptSHA256 -cnotmatch '^[0-9a-f]{64}$') {
    throw 'Current attempt is missing, incomplete, failed, or malformed'
}
if ($PSCmdlet.ParameterSetName -eq 'NewAttempt') {
    if ($PreviousAttemptId -and $PreviousAttemptId -cnotmatch '^[0-9a-f]{32}$') { throw 'Malformed previous attempt identity' }
    if ($attempt.attemptId -ceq $PreviousAttemptId) { throw 'Build did not produce a new current attempt' }
} else {
    if ($ExpectedAttemptId -cnotmatch '^[0-9a-f]{32}$' -or $attempt.attemptId -cne $ExpectedAttemptId) { throw 'Current attempt differs from this job build output' }
    if ($ExpectedReceiptSHA256 -cnotmatch '^[0-9a-f]{64}$' -or $attempt.receiptSHA256 -cne $ExpectedReceiptSHA256) { throw 'Current receipt differs from this job build output' }
}
$receiptSHA256 = (Get-FileHash -LiteralPath $receiptPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($receiptSHA256 -cne $attempt.receiptSHA256) { throw 'Current receipt hash mismatch' }
$receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
if ($receipt.result -cne 'passed' -or $receipt.attemptId -cne $attempt.attemptId -or
    $receipt.validationMode -cne $ValidationMode -or $receipt.appSource -cne $ExpectedAppSource -or
    $receipt.kernelSource -cne $ExpectedKernelSource -or $receipt.repository -cne 'polaris-arch/sing-box-polaris' -or
    $receipt.scope -cne 'directory-protocol-tests-only') { throw 'Current receipt source, mode, attempt or scope mismatch' }
function Read-ManifestDigest([string]$Name,[string]$SourceField,[string]$Source,[string]$ExpectedDigest) {
    $path = Join-Path $EvidenceDirectory $Name
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($boundAttempt) {
        if ($ExpectedDigest -cnotmatch '^[0-9a-f]{64}$' -or $hash -cne $ExpectedDigest) { throw "Manifest changed since this job build: $Name" }
    }
    $manifest = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    if ($manifest.$SourceField -cne $Source -or $manifest.validationMode -cne $ValidationMode -or
        $manifest.sha256 -cnotmatch '^[0-9a-f]{64}$' -or -not [IO.Path]::IsPathFullyQualified($manifest.path)) { throw "Invalid current artifact: $Name" }
    if ($SourceField -ceq 'kernelSource' -and $manifest.repository -cne 'polaris-arch/sing-box-polaris') { throw 'Wrong kernel artifact repository' }
    if ((Get-FileHash -LiteralPath $manifest.path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $manifest.sha256) { throw "Current binary hash mismatch: $Name" }
    return $hash
}
$appHash = Read-ManifestDigest 'app-artifact.json' 'appSource' $ExpectedAppSource $ExpectedAppManifestSHA256
$kernelHash = Read-ManifestDigest 'kernel-artifact.json' 'kernelSource' $ExpectedKernelSource $ExpectedKernelManifestSHA256
$activationHash = Read-ManifestDigest 'activation-artifact.json' 'kernelSource' $ExpectedKernelSource $ExpectedActivationManifestSHA256
# Re-read the marker after all checks; a replaced pointer cannot be returned as verified.
$after = Get-Content -LiteralPath $attemptPath -Raw | ConvertFrom-Json
if ($after.result -cne 'passed' -or $after.attemptId -cne $attempt.attemptId -or $after.receiptSHA256 -cne $receiptSHA256) { throw 'Current attempt changed during verification' }
$systemNonce = $null
$systemReceiptSHA256 = $null
if ($SystemEvidenceDirectory) {
    $systemPath = Join-Path $SystemEvidenceDirectory 'system-bootstrap-receipt.json'
    $system = Get-Content -LiteralPath $systemPath -Raw | ConvertFrom-Json
    if ($system.result -cne 'passed' -or $system.validationMode -cne $ValidationMode -or
        $system.appSource -cne $ExpectedAppSource -or $system.kernelSource -cne $ExpectedKernelSource -or
        $system.scope -cne 'system-producer-user-consumer-bootstrap-only' -or
        $system.nonce -cnotmatch '^[0-9a-f]{32}$' -or $system.nonce -ceq $PreviousSystemNonce -or
        $system.prebuiltUserDomain -isnot [bool] -or $system.prebuiltUserDomain -ne [bool]$PrebuildUserDomain -or
        $system.taskExitCode -ne 0) { throw 'Missing or stale SYSTEM receipt for this invocation' }
    if ($PreviousSystemNonce -and $PreviousSystemNonce -cnotmatch '^[0-9a-f]{32}$') { throw 'Malformed previous SYSTEM nonce' }
    $appArtifact = Get-Content -LiteralPath (Join-Path $EvidenceDirectory 'app-artifact.json') -Raw | ConvertFrom-Json
    $kernelArtifact = Get-Content -LiteralPath (Join-Path $EvidenceDirectory 'kernel-artifact.json') -Raw | ConvertFrom-Json
    if ($system.appSHA256 -cne $appArtifact.sha256 -or $system.kernelSHA256 -cne $kernelArtifact.sha256) { throw 'SYSTEM receipt uses different binary identities' }
    $systemNonce = $system.nonce
    $systemReceiptSHA256 = (Get-FileHash -LiteralPath $systemPath -Algorithm SHA256).Hash.ToLowerInvariant()
}
[pscustomobject]@{SystemNonce=$systemNonce;SystemReceiptSHA256=$systemReceiptSHA256;AttemptId=$attempt.attemptId;ReceiptSHA256=$receiptSHA256;AppManifestSHA256=$appHash;KernelManifestSHA256=$kernelHash;ActivationManifestSHA256=$activationHash}
