#Requires -Version 5.0
# Read-only: hash selected files and query OS metadata. No helper IPC or process execution.
param(
  [Parameter(Mandatory=$true)][string]$Candidate,
  [Parameter(Mandatory=$true)][string]$DeviceId,
  [Parameter(Mandatory=$true)][string]$Nonce,
  [Parameter(Mandatory=$true)][string]$App,
  [Parameter(Mandatory=$true)][string]$Helper,
  [Parameter(Mandatory=$true)][string]$Core,
  [Parameter(Mandatory=$true)][string]$AppVersion,
  [Parameter(Mandatory=$true)][string]$HelperVersion,
  [Parameter(Mandatory=$true)][string]$CoreVersion
)
$ErrorActionPreference = 'Stop'
try {
  if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'Windows OS required' }
  if ($Candidate -cnotmatch '^[0-9a-f]{40}$' -or $Nonce -cnotmatch '^[0-9a-f]{32,64}$') {
    throw 'Exact candidate SHA and fresh 128-256 bit nonce required'
  }
  if ($DeviceId -cnotmatch '^[A-Za-z0-9._-]{1,96}$') { throw 'Nonsecret inventory device ID required' }
  foreach ($Label in @($AppVersion,$HelperVersion,$CoreVersion)) {
    if ($Label -cnotmatch '^[A-Za-z0-9._+-]{1,96}$') { throw 'Nonsecret version label required' }
  }
  $Artifacts = [ordered]@{}
  $Paths = [ordered]@{ app=$App; helper=$Helper; core=$Core }
  foreach ($Role in $Paths.Keys) {
    $File = Get-Item -LiteralPath $Paths[$Role]
    if ($File.PSIsContainer -or $File.Length -le 0) { throw "$Role must be a nonempty installed binary file" }
    $Length = $File.Length
    $LastWriteTicks = $File.LastWriteTimeUtc.Ticks
    $Hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $File.FullName).Hash.ToLowerInvariant()
    $After = Get-Item -LiteralPath $Paths[$Role]
    if ($After.PSIsContainer -or $After.Length -ne $Length -or $After.LastWriteTimeUtc.Ticks -ne $LastWriteTicks) {
      throw 'Selected artifact changed during read'
    }
    $Artifacts[$Role] = [ordered]@{
      sha256=$Hash; bytes=$Length
    }
  }
  $Capabilities = [ordered]@{
    transitionExecutor='NOT_IMPLEMENTED'; trustedNativeObserver='NOT_IMPLEMENTED'; helperProtocolObserved='UNKNOWN'
    fourProducerDrainObserved='UNKNOWN'; helperTailObserved='UNKNOWN'; restoreObserved='UNKNOWN'
    independentPostcheckObserved='UNKNOWN'; artifactBuildProvenance='UNVERIFIED'
  }
  $Architecture = if ([Environment]::Is64BitOperatingSystem) { '64bit-os' } else { '32bit-os' }
  $Report = [ordered]@{
    schemaVersion=1; kind='READ_ONLY_PREFLIGHT'
    binding=[ordered]@{ candidateSha=$Candidate; deviceId=$DeviceId; nonce=$Nonce; platform='windows' }
    artifacts=$Artifacts
    versions=[ordered]@{ app=$AppVersion; helper=$HelperVersion; core=$CoreVersion; source='operator-declared-unverified' }
    host=[ordered]@{
      system='Windows'; release=[Environment]::OSVersion.Version.ToString()
      architecture=$Architecture
      hostnameSha256=([BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes([Environment]::MachineName)))).Replace('-','').ToLowerInvariant()
    }
    readOnly=$true; capabilities=$Capabilities; claims=@()
  }
  $Report | ConvertTo-Json -Depth 12
  exit 0
} catch {
  [Console]::Error.WriteLine('PREFLIGHT_REJECTED: invalid metadata or selected artifact IO failure')
  exit 1
}
