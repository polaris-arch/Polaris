[CmdletBinding()]
param(
 [Parameter(Mandatory=$true)][string]$Manifest,
 [Parameter(Mandatory=$true)][string]$SourceField,
 [Parameter(Mandatory=$true)][string]$ExpectedSource,
 [Parameter(Mandatory=$true)][ValidateSet('formal')][string]$ValidationMode
)
$ErrorActionPreference = 'Stop'
if ($ExpectedSource -cnotmatch '^[0-9a-f]{40}$') { throw 'Exact source identity required' }
$artifact = Get-Content -LiteralPath $Manifest -Raw | ConvertFrom-Json
if ($artifact.$SourceField -cne $ExpectedSource -or $artifact.validationMode -cne $ValidationMode) { throw 'Artifact source/mode mismatch' }
if ($SourceField -eq 'kernelSource' -and $artifact.repository -cne 'polaris-arch/sing-box-polaris') { throw 'Wrong kernel repository' }
if (-not [IO.Path]::IsPathFullyQualified($artifact.path) -or $artifact.sha256 -cnotmatch '^[0-9a-f]{64}$') { throw 'Invalid artifact path/hash' }
if ((Get-FileHash -LiteralPath $artifact.path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $artifact.sha256) { throw 'Artifact hash mismatch' }
return $artifact
