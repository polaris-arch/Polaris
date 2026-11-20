[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$KernelRoot,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [Parameter(Mandatory=$true)][switch]$FormalContract
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
if (-not $FormalContract) { throw 'Select the explicit formal contract' }
$source = $win.sourceCommit; $mode = 'formal'
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
} else { $ldflags += " -X ${key}${source}" }
$kernelExe = Join-Path $outputRoot 'tailscale-tests.exe'
$componentTests = @('TestWindowsDirectorySourceContract','TestWindowsDirectoryReservedNamespaceWholeConfig')
$bindingTest = 'TestWindowsDirectoryActivationAndOriginalBinding'
# The historical checkout already has the component test file, with these cases still in the root package.
$testOwners = @{
    'directory_windows_test.go' = '.'
    'internal/boxruntime/directory_windows_test.go' = './internal/boxruntime'
    'experimental/directorywindows/prepared_activation_windows_test.go' = './experimental/directorywindows'
}
$testPackages = @{}
foreach ($name in ($componentTests + $bindingTest)) {
    $declarations = @(& git -C $kernelRoot grep -l -e "^func ${name}(" $source -- $testOwners.Keys)
    if ($LASTEXITCODE -ne 0 -or $declarations.Count -ne 1) { throw "Expected one tracked owner for activation test: $name" }
    $testPackages[$name] = $testOwners[$declarations[0].Substring($source.Length + 1)]
}
$bindingPackage = $testPackages[$bindingTest]
$componentPackage = $testPackages[$componentTests[0]]
if ($bindingPackage -cnotin @('.', './internal/boxruntime') -or
    $componentPackage -cnotin @('.', './experimental/directorywindows') -or
    $testPackages[$componentTests[1]] -cne $componentPackage -or
    ($componentPackage -ceq '.' -and $bindingPackage -cne '.')) { throw 'Unexpected activation test ownership' }
$activation = @(@{package=$bindingPackage;path=(Join-Path $outputRoot 'directory-activation-tests.exe');tests=@($bindingTest)})
if ($componentPackage -ceq '.') {
    $activation[0].tests = $componentTests + $bindingTest
} else {
    $activation += @{package=$componentPackage;path=(Join-Path $outputRoot 'directory-activation-component-tests.exe');tests=$componentTests}
}
Push-Location $kernelRoot
try {
    # Read effective flags, including persisted GOENV settings; do not modify them.
    $effectiveGoFlags = & go env GOFLAGS
    if ($LASTEXITCODE -ne 0) { throw 'Effective Go flags unavailable' }
    if ($effectiveGoFlags -match '(?:^|[\s"''])-{1,2}overlay(?:=|\s|["'']|$)') { throw 'Ambient Go overlay is forbidden' }
    $actualGo = & go env GOVERSION
    if ($LASTEXITCODE -ne 0) { throw 'Go compiler identity unavailable' }
    $toolchainJson = & node (Join-Path $PSScriptRoot 'resolve-toolchain.mjs') --kernel-root $kernelRoot --mode $mode --source $source --actual-go $actualGo
    if ($LASTEXITCODE -ne 0) { throw 'Exact source/compiler policy verification failed' }
    $toolchainJson | Set-Content -LiteralPath (Join-Path $outputRoot 'toolchain-evidence.json') -Encoding utf8NoBOM
    & go test -c -buildvcs=true -mod=readonly -trimpath -tags $tags -ldflags $ldflags -o $kernelExe ./protocol/tailscale
    if ($LASTEXITCODE -ne 0) { throw 'Native kernel test producer build failed' }
    foreach ($target in $activation) {
        & go test -c -buildvcs=true -mod=readonly -trimpath -tags $tags -ldflags $ldflags -o $target.path $target.package
        if ($LASTEXITCODE -ne 0) { throw "Native kernel activation test build failed: $($target.package)" }
        $binary = Get-Item -LiteralPath $target.path
        if ($binary.PSIsContainer -or $binary.LinkType) { throw "Invalid activation test binary: $($target.path)" }
        $target.sha256 = (Get-FileHash -LiteralPath $target.path -Algorithm SHA256).Hash.ToLowerInvariant()
    }
} finally { Pop-Location }
$actual = & git -C $kernelRoot rev-parse HEAD
if ($LASTEXITCODE -ne 0 -or $actual -cne $source) { throw 'Kernel HEAD changed during test build' }
$dirty = & git -C $kernelRoot status --porcelain --untracked-files=all
if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'Kernel source changed during test build' }
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
foreach ($target in $activation) {
    if ((Get-FileHash -LiteralPath $target.path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $target.sha256) { throw "Activation test binary changed before execution: $($target.path)" }
    $activationList = & $target.path '-test.list=^TestWindowsDirectory'
    if ($LASTEXITCODE -ne 0) { throw "Activation test discovery failed: $($target.package)" }
    foreach ($name in $target.tests) { if (-not ($activationList -ccontains $name)) { throw "Missing exact activation regression: $name" } }
    & $target.path ('-test.run=^(' + ($target.tests -join '|') + ')$') '-test.v' '-test.timeout=2m'
    if ($LASTEXITCODE -ne 0) { throw "Production activation regressions failed: $($target.package)" }
    if ((Get-FileHash -LiteralPath $target.path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $target.sha256) { throw "Activation test binary changed during execution: $($target.path)" }
}
$activationManifest = @{kernelSource=$source;validationMode=$mode;repository='polaris-arch/sing-box-polaris'}
if ($activation.Count -eq 1) {
    $activationManifest += @{path=$activation[0].path;sha256=$activation[0].sha256;tests=$activation[0].tests}
} else {
    $activationManifest.artifacts = $activation
}
$activationManifest | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8NoBOM (Join-Path $outputRoot 'activation-artifact.json')
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
@{attemptId=$attemptId;validationMode=$mode;kernelSource=$source;appSource=$appSource;repository='polaris-arch/sing-box-polaris';result='passed';scope='directory-protocol-tests-only';formalSourceContractMatched=($mode -eq 'formal')} | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8NoBOM $validationReceipt
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
