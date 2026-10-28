# Windows PowerShell 5.1+; use an isolated test account/runner.
# Does not target Polaris paths, services, tokens, UAC or the installed helper.
# NOT EXECUTED by the source-preparation worker.
$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) ('polaris-registration-rollback-test.' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
function Assert-Regular($path) {
  $item = Get-Item -LiteralPath $path -Force
  if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'not regular' }
}
# Fixtures are already private to this test. Do not require administrator owner
# changes merely to exercise the production byte/security restore function.
function Seal-File($path) { Assert-Regular $path }
function Check-Hash($path, $expected) {
  Assert-Regular $path
  if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $expected) { throw 'hash mismatch' }
}
function Assert($condition, $message) { if (-not $condition) { throw $message } }
# PRODUCTION_REGISTRATION_ROLLBACK_FUNCTIONS

function Fixture($name) {
  $dir = Join-Path $root $name
  New-Item -ItemType Directory -Path $dir | Out-Null
  $path = Join-Path $dir 'registration'
  $backup = Join-Path $dir 'registration.rollback'
  [IO.File]::WriteAllText($path, 'original fixture bytes')
  # A non-default protected DACL makes silent Seal-File substitution detectable.
  $acl = Get-Acl -LiteralPath $path
  $acl.SetAccessRuleProtection($true, $true)
  Set-Acl -LiteralPath $path -AclObject $acl
  Copy-Item -LiteralPath $path -Destination $backup
  $snapshot = @{Path=$path; Backup=$backup; Present=$true; Hash=(Get-FileHash $path).Hash.ToLowerInvariant(); Sddl=(Registration-Sddl $path)}
  [IO.File]::WriteAllText($path, 'replacement fixture bytes')
  $replacementAcl = Get-Acl -LiteralPath $path
  $replacementAcl.SetAccessRuleProtection($false, $true)
  Set-Acl -LiteralPath $path -AclObject $replacementAcl
  Assert ((Registration-Sddl $path) -cne $snapshot.Sddl) 'fixture did not change replacement DACL'
  return $snapshot
}
$passed = 0
try {
  $s = Fixture 'present'
  Restore-Registration $s
  Check-Hash $s.Path $s.Hash
  Assert ((Registration-Sddl $s.Path) -ceq $s.Sddl) 'original security not restored'
  $passed++

  $s = Fixture 'missing-destination'
  Remove-Item -LiteralPath $s.Path
  Restore-Registration $s
  Check-Hash $s.Path $s.Hash
  Assert ((Registration-Sddl $s.Path) -ceq $s.Sddl) 'missing destination security differs'
  $passed++

  $s = Fixture 'original-absent'
  $s.Present = $false
  Restore-Registration $s
  Assert (-not (Test-Path -LiteralPath $s.Path)) 'original absence not restored'
  $passed++

  $s = Fixture 'tampered-backup'
  [IO.File]::WriteAllText($s.Backup, 'tampered fixture')
  $before = (Get-FileHash $s.Path).Hash
  $failed = $false
  try { Restore-Registration $s } catch { $failed = $true }
  Assert $failed 'corrupt backup accepted'
  Assert ((Get-FileHash $s.Path).Hash -ceq $before) 'destination changed despite corrupt backup'
  $passed++

  $s = Fixture 'blocked-replacement'
  $handle = [IO.File]::Open($s.Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
  $failed = $false
  try { Restore-Registration $s } catch { $failed = $true } finally { $handle.Dispose() }
  Assert $failed 'locked destination was reported restored'
  Assert (Test-Path -LiteralPath $s.Backup) 'backup was lost after replace failure'
  $passed++

  $s = Fixture 'invalid-security'
  $s.Sddl = 'not valid SDDL'
  $failed = $false
  try { Restore-Registration $s } catch { $failed = $true }
  Assert $failed 'invalid security was reported restored'
  Assert (Test-Path -LiteralPath $s.Backup) 'backup was lost after security failure'
  $passed++
  $parentFile = Join-Path $root 'parent-is-file'
  [IO.File]::WriteAllText($parentFile, 'synthetic parent')
  $failed = $false
  try { Assert-PrivateRegistrationDirectory $parentFile } catch { $failed = $true }
  Assert $failed 'non-directory registration parent accepted'
  $passed++

  $realParent = Join-Path $root 'real-parent'
  $junction = Join-Path $root 'reparse-parent'
  New-Item -ItemType Directory -Path $realParent | Out-Null
  New-Item -ItemType Junction -Path $junction -Target $realParent | Out-Null
  $failed = $false
  try { Assert-PrivateRegistrationDirectory $junction } catch { $failed = $true }
  Assert $failed 'reparse registration parent accepted'
  # Remove the junction itself, never recursively traverse its target.
  [IO.Directory]::Delete($junction)
  $passed++
  Write-Output "Passed $passed native fixture cases. Full installer/SCM rollback still requires separate integration coverage."
} finally { Remove-Item -LiteralPath $root -Recurse -Force }
