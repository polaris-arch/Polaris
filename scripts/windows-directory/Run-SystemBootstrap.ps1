# Explicit one-shot CI SYSTEM task only; never called by ordinary tests implicitly.
[CmdletBinding()]
param(
 [Parameter(Mandatory=$true)][string]$AppArtifactManifest,
 [Parameter(Mandatory=$true)][string]$KernelArtifactManifest,
 [Parameter(Mandatory=$true)][string]$ExpectedAppSource,
 [Parameter(Mandatory=$true)][string]$ExpectedKernelSource,
 [Parameter(Mandatory=$true)][ValidateSet('formal','candidate')][string]$ValidationMode,
 [Parameter(Mandatory=$true)][string]$EvidenceDirectory,
 [switch]$ApproveTemporarySystemTask,
 [switch]$PrebuildUserDomain
)
$ErrorActionPreference = 'Stop'
if (-not $ApproveTemporarySystemTask) { throw 'Explicit temporary SYSTEM task approval required' }
if ($env:OS -ne 'Windows_NT') { throw 'Native Windows required' }
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$sid = $identity.User.Value
if ($sid -eq 'S-1-5-18') { throw 'Consumer must be the owning user, not SYSTEM' }
if (-not ([Security.Principal.WindowsPrincipal]::new($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Approved elevated CI user required' }
function Read-Artifact([string]$path,[string]$field,[string]$expected) {
    if ($expected -cnotmatch '^[0-9a-f]{40}$') { throw 'Exact reviewed source required' }
    $value = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    if ($value.$field -cne $expected -or $value.validationMode -cne $ValidationMode) { throw 'Artifact source/mode mismatch' }
    if ($field -eq 'kernelSource' -and $value.repository -cne 'polaris-arch/sing-box-polaris') { throw 'Wrong producer repository' }
    if (-not [IO.Path]::IsPathFullyQualified($value.path) -or $value.sha256 -cnotmatch '^[0-9a-f]{64}$') { throw 'Invalid artifact path/hash' }
    if ((Get-FileHash -LiteralPath $value.path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $value.sha256) { throw 'Artifact hash mismatch' }
    return $value
}
$app = Read-Artifact $AppArtifactManifest 'appSource' $ExpectedAppSource
$kernel = Read-Artifact $KernelArtifactManifest 'kernelSource' $ExpectedKernelSource
$name = 'runtime::windows_directory::tests::interop_system_producer_user_consumer'
$list = & $app.path --list
if ($LASTEXITCODE -ne 0 -or -not ($list -ccontains "${name}: test")) { throw 'Missing user consumer test' }
$list = & $kernel.path '-test.list=^TestWindowsDirectorySystemInteropChild$'
if ($LASTEXITCODE -ne 0 -or -not ($list -ccontains 'TestWindowsDirectorySystemInteropChild')) { throw 'Missing SYSTEM producer test' }
New-Item -ItemType Directory -Force -Path $EvidenceDirectory | Out-Null
$nonce = [guid]::NewGuid().ToString('N')
$taskName = "PolarisDirectoryCI-$nonce"
$private = Join-Path ([IO.Path]::GetTempPath()) $taskName
$receipt = Join-Path $EvidenceDirectory 'system-bootstrap-receipt.json'
if (Test-Path -LiteralPath $receipt) { Remove-Item -LiteralPath $receipt }
$success = $null
try {
    New-Item -ItemType Directory -Path $private | Out-Null
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $acl.SetSecurityDescriptorSddlForm("O:${sid}D:P(A;OICI;FA;;;${sid})(A;OICI;FA;;;SY)")
    Set-Acl -LiteralPath $private -AclObject $acl
    $producer = Join-Path $private 'producer.exe'
    Copy-Item -LiteralPath $kernel.path -Destination $producer
    if ((Get-FileHash -LiteralPath $producer -Algorithm SHA256).Hash.ToLowerInvariant() -cne $kernel.sha256) { throw 'Private producer copy hash mismatch' }
    $config = Join-Path $private 'config'
    New-Item -ItemType Directory -Path $config | Out-Null
    Set-Acl -LiteralPath $config -AclObject $acl
    if ($config.Contains('"')) { throw 'Unsafe fixture spelling' }
    $server = 'node'
    if ($PrebuildUserDomain) {
        $userNonce = [guid]::NewGuid().ToString('N')
        $start = [Diagnostics.ProcessStartInfo]::new($producer)
        $start.UseShellExecute = $false
        $start.ArgumentList.Add('-test.run=^TestWindowsDirectoryInteropChild$')
        $start.ArgumentList.Add('-test.timeout=65s')
        $start.Environment['POLARIS_DIRECTORY_INTEROP_ROOT'] = $config
        $start.Environment['POLARIS_DIRECTORY_INTEROP_NONCE'] = $userNonce
        $start.Environment['POLARIS_DIRECTORY_INTEROP_POISON'] = '0'
        $process = [Diagnostics.Process]::Start($start)
        try {
            $deadline = [DateTime]::UtcNow.AddSeconds(30)
            do {
                $ready = Join-Path $config 'interop-ready'
                if ((Test-Path -LiteralPath $ready) -and (Get-Content -LiteralPath $ready -Raw) -ceq $userNonce) { break }
                if ($process.HasExited -or [DateTime]::UtcNow -ge $deadline) { throw 'User producer failed before ready' }
                Start-Sleep -Milliseconds 20
            } while ($true)
            [IO.File]::WriteAllText((Join-Path $config 'interop-close'),$userNonce)
            if (-not $process.WaitForExit(15000) -or $process.ExitCode -ne 0) { throw 'User producer did not close successfully' }
            if ((Get-Content -LiteralPath (Join-Path $config 'interop-closed') -Raw) -cne $userNonce) { throw 'Missing user close receipt' }
        } finally {
            if (-not $process.HasExited) { $process.Kill(); $process.WaitForExit() }
            $process.Dispose()
        }
        foreach ($part in @('interop-ready','interop-close','interop-closed')) { Remove-Item -LiteralPath (Join-Path $config $part) }
        $server = 'system-node'
    }
    $arguments = "-test.run=^TestWindowsDirectorySystemInteropChild$ -test.v -test.timeout=75s -polaris-directory-system-root=`"$config`" -polaris-directory-system-nonce=$nonce -polaris-directory-system-owner=$sid -polaris-directory-system-server=$server"
    $action = New-ScheduledTaskAction -Execute $producer -Argument $arguments
    $principal = New-ScheduledTaskPrincipal -UserId 'S-1-5-18' -LogonType ServiceAccount -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 2)
    Register-ScheduledTask -TaskName $taskName -TaskPath '\' -Action $action -Principal $principal -Settings $settings -Description 'One-shot approved Polaris directory CI fixture' | Out-Null
    Start-ScheduledTask -TaskName $taskName -TaskPath '\'
    $env:POLARIS_SYSTEM_INTEROP_ROOT = $config
    $env:POLARIS_SYSTEM_INTEROP_NONCE = $nonce
    $env:POLARIS_SYSTEM_INTEROP_MODE = $ValidationMode
    $env:POLARIS_SYSTEM_INTEROP_SOURCE = $ExpectedKernelSource
    $env:POLARIS_SYSTEM_INTEROP_SERVER = $server
    & $app.path --exact $name --ignored --nocapture --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'User consumer rejected SYSTEM bootstrap/retirement' }
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while ((Get-ScheduledTask -TaskName $taskName -TaskPath '\' -ErrorAction Stop).State -in @('Running','Queued')) {
        if ([DateTime]::UtcNow -ge $deadline) { throw 'SYSTEM producer did not exit' }
        Start-Sleep -Milliseconds 50
    }
    $result = Get-ScheduledTaskInfo -TaskName $taskName -TaskPath '\' -ErrorAction Stop
    if ($result.LastTaskResult -ne 0) { throw "SYSTEM producer exit failure: $($result.LastTaskResult)" }
    if ((Get-Content -LiteralPath (Join-Path $config 'interop-consumed') -Raw) -cne $nonce) { throw 'Missing exact consumer receipt' }
    $success = @{validationMode=$ValidationMode;scope='system-producer-user-consumer-bootstrap-only';prebuiltUserDomain=[bool]$PrebuildUserDomain;appSource=$ExpectedAppSource;kernelSource=$ExpectedKernelSource;appSHA256=$app.sha256;kernelSHA256=$kernel.sha256;nonce=$nonce;taskExitCode=$result.LastTaskResult;result='passed'}
} finally {
    foreach ($key in @('ROOT','NONCE','MODE','SOURCE','SERVER')) { Remove-Item "Env:POLARIS_SYSTEM_INTEROP_$key" -ErrorAction SilentlyContinue }
    function Get-OwnedTask {
        # Successful enumeration is required to prove absence. Query errors are
        # never suppressed or interpreted as a nonexistent privileged task.
        $found = @(Get-ScheduledTask -ErrorAction Stop | Where-Object { $_.TaskPath -ceq '\' -and $_.TaskName -ceq $taskName })
        if ($found.Count -gt 1) { throw 'Ambiguous temporary task identity' }
        if ($found.Count -eq 1) { return $found[0] }
        return $null
    }
    try {
        $task = Get-OwnedTask
        if ($task) {
            Stop-ScheduledTask -TaskName $taskName -TaskPath '\' -ErrorAction Stop
            $deadline = [DateTime]::UtcNow.AddSeconds(15)
            do {
                $task = Get-OwnedTask
                if ($null -eq $task -or $task.State -notin @('Running','Queued')) { break }
                if ([DateTime]::UtcNow -ge $deadline) { throw 'SYSTEM task stop unconfirmed' }
                Start-Sleep -Milliseconds 50
            } while ($true)
            if ($task) { Unregister-ScheduledTask -TaskName $taskName -TaskPath '\' -Confirm:$false -ErrorAction Stop }
        }
        if ($null -ne (Get-OwnedTask)) { throw 'SYSTEM task still exists after unregister' }
    } catch {
        $failure = "$_"
        try { @{result='failed';phase='system-task-cleanup';taskName=$taskName;retainedFixture=$private;error=$failure} | ConvertTo-Json | Set-Content -Encoding utf8NoBOM (Join-Path $EvidenceDirectory 'system-bootstrap-cleanup-failure.json') }
        catch { Write-Warning "Could not persist cleanup failure evidence: $_" }
        throw "SYSTEM task cleanup unverified; fixture retained at ${private}: $failure"
    }
    if (Test-Path -LiteralPath $private) { Remove-Item -LiteralPath $private -Recurse -Force }
}
# Never publish success before task absence and private fixture cleanup.
if ($null -eq $success) { throw 'No successful SYSTEM validation result' }
$success | ConvertTo-Json | Set-Content -Encoding utf8NoBOM $receipt
