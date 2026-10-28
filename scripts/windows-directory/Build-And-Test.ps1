[CmdletBinding(DefaultParameterSetName='ExplicitModeRequired')]
param(
    [Parameter(Mandatory=$true)][string]$KernelRoot,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [Parameter(Mandatory=$true,ParameterSetName='Formal')][switch]$FormalContract,
    [Parameter(Mandatory=$true,ParameterSetName='Candidate')][string]$CandidateSourceCommit
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$outputRoot = (Resolve-Path $OutputDirectory).Path
$validationReceipt = Join-Path $outputRoot 'validation-receipt.json'
# Preserve earlier evidence, but invalidate the current success pointer before
# resolving sources or running any build. Failed archival stops this attempt.
$attemptId = [guid]::NewGuid().ToString('N')
$attemptPath = Join-Path $outputRoot 'validation-attempt.json'
@{attemptId=$attemptId;result='inProgress'} | ConvertTo-Json | Set-Content -Encoding utf8NoBOM $attemptPath
if (Test-Path -LiteralPath $validationReceipt) {
    $oldHash = (Get-FileHash -LiteralPath $validationReceipt -Algorithm SHA256).Hash.ToLowerInvariant()
    $history = Join-Path $outputRoot 'receipt-history'
    New-Item -ItemType Directory -Force -Path $history | Out-Null
    Move-Item -LiteralPath $validationReceipt -Destination (Join-Path $history "${oldHash}-${attemptId}.json") -ErrorAction Stop
}
try {
$appRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$kernelRoot = (Resolve-Path $KernelRoot).Path
$win = (Get-Content -LiteralPath (Join-Path $appRoot 'scripts/desktop-core/release-contract.json') -Raw | ConvertFrom-Json -AsHashtable).platforms.win
if ($PSCmdlet.ParameterSetName -eq 'Candidate') {
    $source = $CandidateSourceCommit; $mode = 'candidate'
} elseif ($PSCmdlet.ParameterSetName -eq 'Formal' -and $FormalContract) {
    $source = $win.sourceCommit; $mode = 'formal'
} else { throw 'Select exactly one explicit candidate source or formal contract' }
if ($source -cnotmatch '^[0-9a-f]{40}$') { throw 'Exact lowercase source SHA required' }
$origin = & git -C $kernelRoot remote get-url origin
if ($LASTEXITCODE -ne 0 -or $origin -cnotmatch '^https://github[.]com/polaris-arch/sing-box-polaris([.]git)?$') { throw 'Wrong kernel fork origin' }
$actual = & git -C $kernelRoot rev-parse HEAD
if ($LASTEXITCODE -ne 0 -or $actual -cne $source) { throw 'Kernel HEAD differs from selected source' }
$dirty = & git -C $kernelRoot status --porcelain --untracked-files=all
if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'Kernel producer source is not clean' }
$appSource = & git -C $appRoot rev-parse HEAD
if ($LASTEXITCODE -ne 0 -or $appSource -cnotmatch '^[0-9a-f]{40}$') { throw 'App actual source identity unavailable' }
function Assert-AppTrackedSource {
    $head = & git -C $appRoot rev-parse HEAD
    if ($LASTEXITCODE -ne 0 -or $head -cne $appSource) { throw 'App HEAD changed during validation' }
    # Staged and unstaged tracked changes both fail; normal untracked resources,
    # target and producer checkout outputs do not invalidate the source claim.
    & git -C $appRoot diff --quiet HEAD --
    if ($LASTEXITCODE -ne 0) { throw 'App tracked source dirty or verification failed' }
}
Assert-AppTrackedSource
$tags = $win.tags
if (-not ($tags -is [string]) -or -not ($tags.Split(',') -ccontains 'with_tailscale')) { throw 'Frozen production tags missing tailscale' }
if ($win.schema -ceq 'polaris-upstream-native-receipt-v1') {
    # This is a new directory-test build, not the published package producer's
    # command. Read flags from the exact clean selected source, and retain the
    # original release row without fabricating buildCommand/ldflags evidence.
    Push-Location $appRoot
    try {
        $verifiedSource = & node --input-type=module -e 'import {readFileSync} from "node:fs"; import {loadReleaseContract} from "./scripts/desktop-core/release-consumer.mjs"; console.log(loadReleaseContract(process.cwd(),JSON.parse(readFileSync("src-tauri/core-manifest.json"))).platforms.win.sourceCommit)'
        if ($LASTEXITCODE -ne 0 -or $verifiedSource -cne $win.sourceCommit) { throw 'Verified upstream directory source unavailable' }
    } finally { Pop-Location }
    $flagsPath = Join-Path $kernelRoot 'release/LDFLAGS'
    if ((Get-Item -LiteralPath $flagsPath).LinkType) { throw 'Directory-test flags must be a regular source file' }
    $sharedFlags = (Get-Content -LiteralPath $flagsPath -Raw).Trim()
    if ([string]::IsNullOrEmpty($sharedFlags) -or $sharedFlags.Contains('SourceRevision=') -or $sharedFlags.Contains('constant.Version=')) { throw 'Conflicting source-owned directory-test flags' }
    $ldflags = "$sharedFlags -X github.com/sagernet/sing-box/constant.Version=$($win.version)"
    @{scope='directory-test-build-only';kernelSource=$source;releaseSource=$win.sourceCommit;tags=$tags;sharedFlagsPath='release/LDFLAGS';sharedFlagsSHA256=(Get-FileHash -LiteralPath $flagsPath -Algorithm SHA256).Hash.ToLowerInvariant();version=$win.version} | ConvertTo-Json | Set-Content -Encoding utf8NoBOM (Join-Path $outputRoot 'test-build-inputs.json')
} else {
    $command = @($win.buildCommand)
    $index = [Array]::IndexOf($command,'-ldflags')
    if ($index -lt 0 -or $index + 1 -ge $command.Count) { throw 'Frozen production LDFLAGS missing' }
    $ldflags = [string]$command[$index+1]
}
$key = 'github.com/sagernet/sing-box/constant.SourceRevision='
if ($ldflags.Contains($key)) {
    $matches = [regex]::Matches($ldflags,[regex]::Escape($key)+'([^\s]+)')
    if ($matches.Count -ne 1 -or $matches[0].Groups[1].Value -cne $win.sourceCommit) { throw 'Conflicting formal SourceRevision flag' }
    if ($mode -eq 'candidate') { $ldflags = $ldflags.Replace($key+$win.sourceCommit,$key+$source) }
} else { $ldflags += " -X ${key}${source}" }
$kernelExe = Join-Path $outputRoot 'tailscale-tests.exe'
$activationExe = Join-Path $outputRoot 'directory-activation-tests.exe'
Push-Location $kernelRoot
try {
    $actualGo = & go env GOVERSION
    if ($LASTEXITCODE -ne 0) { throw 'Go compiler identity unavailable' }
    $toolchainJson = & python (Join-Path $PSScriptRoot 'resolve-toolchain.py') --kernel-root $kernelRoot --mode $mode --source $source --actual-go $actualGo
    if ($LASTEXITCODE -ne 0) { throw 'Exact source/compiler policy verification failed' }
    $toolchain = $toolchainJson | ConvertFrom-Json -AsHashtable
    $expectedGo = $toolchain.selectedCompiler
    if ($actualGo -cne $expectedGo) { throw 'Go compiler differs from exact selected compiler' }
    $toolchainJson | Set-Content -LiteralPath (Join-Path $outputRoot 'toolchain-evidence.json') -Encoding utf8NoBOM
    & go test -c -buildvcs=true -mod=readonly -trimpath -tags $tags -ldflags $ldflags -o $kernelExe ./protocol/tailscale
    if ($LASTEXITCODE -ne 0) { throw 'Native kernel test producer build failed' }
    & go test -c -buildvcs=true -mod=readonly -trimpath -tags $tags -ldflags $ldflags -o $activationExe .
    if ($LASTEXITCODE -ne 0) { throw 'Native kernel activation test build failed' }
} finally { Pop-Location }
Push-Location $appRoot
try {
    $buildLog = Join-Path $outputRoot 'app-test-build.jsonl'
    & cargo test -p polaris --lib --no-run --message-format=json-render-diagnostics 1> $buildLog
    if ($LASTEXITCODE -ne 0) { throw 'Native App test consumer build failed' }
    $executables = @(Get-Content -LiteralPath $buildLog | ForEach-Object {
        $row = $_ | ConvertFrom-Json
        if ($row.reason -eq 'compiler-artifact' -and $row.profile.test -eq $true -and $row.executable -and $row.target.name -ceq 'polaris_lib') { $row.executable }
    } | Select-Object -Unique)
    if ($executables.Count -ne 1) { throw 'Expected exactly one actual App test executable' }
    $appExe = $executables[0]
} finally { Pop-Location }
Assert-AppTrackedSource
$kernelManifest = Join-Path $outputRoot 'kernel-artifact.json'
$appManifest = Join-Path $outputRoot 'app-artifact.json'
@{path=$kernelExe;sha256=(Get-FileHash $kernelExe -Algorithm SHA256).Hash.ToLowerInvariant();kernelSource=$source;validationMode=$mode;repository='polaris-arch/sing-box-polaris'} | ConvertTo-Json | Set-Content -Encoding utf8NoBOM $kernelManifest
@{path=$appExe;sha256=(Get-FileHash $appExe -Algorithm SHA256).Hash.ToLowerInvariant();appSource=$appSource;validationMode=$mode} | ConvertTo-Json | Set-Content -Encoding utf8NoBOM $appManifest
$activationNames = @('TestWindowsDirectorySourceContract','TestWindowsDirectoryReservedNamespaceWholeConfig','TestWindowsDirectoryActivationAndOriginalBinding')
$activationList = & $activationExe '-test.list=^TestWindowsDirectory'
if ($LASTEXITCODE -ne 0) { throw 'Activation test discovery failed' }
foreach ($name in $activationNames) { if (-not ($activationList -ccontains $name)) { throw "Missing exact activation regression: $name" } }
& $activationExe ('-test.run=^(' + ($activationNames -join '|') + ')$') '-test.v' '-test.timeout=2m'
if ($LASTEXITCODE -ne 0) { throw 'Production activation regressions failed' }
@{path=$activationExe;sha256=(Get-FileHash $activationExe -Algorithm SHA256).Hash.ToLowerInvariant();kernelSource=$source;validationMode=$mode;repository='polaris-arch/sing-box-polaris';tests=$activationNames} | ConvertTo-Json | Set-Content -Encoding utf8NoBOM (Join-Path $outputRoot 'activation-artifact.json')
$listing = & $kernelExe '-test.list=^TestWindows'
if ($LASTEXITCODE -ne 0) { throw 'Kernel test discovery failed' }
$required = @(
 'TestWindowsDirectoryInteropChild',
 'TestWindowsDirectoryCrossProcessWriterAndRetirementExclude',
 'TestWindowsDirectoryCrossProcessRevokeFailureRetainsOldLease',
 'TestWindowsDirectoryCrossProcessCrashReopensSameGeneration',
 'TestWindowsDirectoryCrossProcessCrashCannotUndoRevocation',
 'TestWindowsDirectoryCrossProcessReplacementCannotAdoptOldIdentity',
 'TestWindowsDirectoryCrossProcessDACLRejectsNewOwner',
 'TestWindowsDirectoryCrossProcessOldLeasePoisonsOnDACLChange',
 'TestWindowsDirectoryForeignOwnerRejectsUnverifiedParent',
 'TestWindowsDirectoryForeignOwnerRejectsNonSystemToken'
)
foreach ($name in $required) { if (-not ($listing -ccontains $name)) { throw "Missing exact kernel regression: $name" } }
& $kernelExe '-test.run=^TestWindows' '-test.v' '-test.timeout=10m'
if ($LASTEXITCODE -ne 0) { throw 'Native kernel Windows regressions failed' }
& (Join-Path $PSScriptRoot 'Run-WindowsDirectoryInterop.ps1') -AppArtifactManifest $appManifest -KernelArtifactManifest $kernelManifest -ExpectedAppSource $appSource -ExpectedKernelSource $source -ValidationMode $mode
Assert-AppTrackedSource
@{attemptId=$attemptId;validationMode=$mode;kernelSource=$source;appSource=$appSource;repository='polaris-arch/sing-box-polaris';result='passed';scope='directory-protocol-tests-only';formalSourceContractMatched=($mode -eq 'formal')} | ConvertTo-Json | Set-Content -Encoding utf8NoBOM $validationReceipt
@{attemptId=$attemptId;result='passed';receipt='validation-receipt.json';receiptSHA256=(Get-FileHash -LiteralPath $validationReceipt -Algorithm SHA256).Hash.ToLowerInvariant()} | ConvertTo-Json | Set-Content -Encoding utf8NoBOM $attemptPath
} catch {
    # If final receipt publication succeeded but attempt publication failed,
    # revoke that success pointer as well; no failed attempt may advertise pass.
    if (Test-Path -LiteralPath $validationReceipt) {
        $history = Join-Path $outputRoot 'receipt-history'
        New-Item -ItemType Directory -Force -Path $history | Out-Null
        $failedHash = (Get-FileHash -LiteralPath $validationReceipt -Algorithm SHA256).Hash.ToLowerInvariant()
        Move-Item -LiteralPath $validationReceipt -Destination (Join-Path $history "${failedHash}-${attemptId}-incomplete.json") -ErrorAction Stop
    }
    @{attemptId=$attemptId;result='failed';error="$_"} | ConvertTo-Json | Set-Content -Encoding utf8NoBOM $attemptPath
    throw
}
