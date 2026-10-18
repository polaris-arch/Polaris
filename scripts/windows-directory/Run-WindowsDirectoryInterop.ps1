[CmdletBinding()]
param(
 [Parameter(Mandatory=$true)][string]$AppArtifactManifest,
 [Parameter(Mandatory=$true)][string]$KernelArtifactManifest,
 [Parameter(Mandatory=$true)][string]$ExpectedAppSource,
 [Parameter(Mandatory=$true)][string]$ExpectedKernelSource,
 [Parameter(Mandatory=$true)][ValidateSet('formal','candidate')][string]$ValidationMode
)
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Native Windows runner required' }
function Read-PinnedArtifact([string]$manifest,[string]$field,[string]$expected) {
    if ($expected -cnotmatch '^[0-9a-f]{40}$') { throw 'Exact source identity required' }
    $value = Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json
    if ($value.$field -cne $expected -or $value.validationMode -cne $ValidationMode) { throw 'Artifact source/mode mismatch' }
    if ($field -eq 'kernelSource' -and $value.repository -cne 'polaris-arch/sing-box-polaris') { throw 'Wrong kernel repository' }
    if (-not [IO.Path]::IsPathFullyQualified($value.path) -or $value.sha256 -cnotmatch '^[0-9a-f]{64}$') { throw 'Invalid artifact path/hash' }
    if ((Get-FileHash -LiteralPath $value.path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $value.sha256) { throw 'Artifact hash mismatch' }
    return $value.path
}
$app = Read-PinnedArtifact $AppArtifactManifest 'appSource' $ExpectedAppSource
$kernel = Read-PinnedArtifact $KernelArtifactManifest 'kernelSource' $ExpectedKernelSource
$env:POLARIS_WINDOWS_KERNEL_TEST_ARTIFACT = (Resolve-Path -LiteralPath $KernelArtifactManifest).Path
$env:POLARIS_WINDOWS_TEST_VALIDATION_MODE = $ValidationMode
$env:POLARIS_WINDOWS_TEST_EXPECTED_SOURCE = $ExpectedKernelSource
$names = @(
 'runtime::windows_directory::tests::interop_writer_retirement_and_stale_intent',
 'runtime::windows_directory::tests::interop_crash_reopen_and_revocation',
 'runtime::windows_directory::tests::interop_replaced_identity_is_retained'
)
$listing = & $app --list
if ($LASTEXITCODE -ne 0) { throw 'App test discovery failed' }
foreach ($name in $names) {
    if (-not ($listing -ccontains "${name}: test")) { throw "Missing exact App integration test: $name" }
    & $app --exact $name --ignored --nocapture --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw "App integration failed: $name" }
}
Write-Output "Directory integration executed: mode=$ValidationMode source=$ExpectedKernelSource; candidate is not formal release acceptance."
