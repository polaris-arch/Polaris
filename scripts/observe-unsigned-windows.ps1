param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$Path
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$signature = Get-AuthenticodeSignature -LiteralPath $Path
if ($signature.Status -ne 'NotSigned') {
    throw "expected explicit unsigned distribution state: $($signature.Status)"
}
@{ status = $signature.Status.ToString(); signerThumbprint = $null } | ConvertTo-Json -Compress
