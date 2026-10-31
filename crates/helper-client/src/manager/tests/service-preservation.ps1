# Inert SCM + temporary byte fixtures. No actual SCM/UAC/helper invocation.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.ServiceProcess
# PRODUCTION_SERVICE_POLICY
$service = 'InertPolarisFixture'
$binPath = '"C:\inert\helper.exe" --support "C:\inert\support"'
function Assert($condition,$message) { if (-not $condition) { throw $message } }
function Get-ServicePolicySnapshot {
  if ($unknown -or ($waitCompleted -and $scenario -ceq 'final-policy-unknown') -or
      ($rollbackRestored -and $scenario -ceq 'stopped-policy-unknown')) { throw 'injected native query failure' }
  return $policy
}
function Get-ServiceDescription {
  if ($descriptionUnknown -or ($waitCompleted -and $scenario -ceq 'final-description-unknown') -or
      ($rollbackRestored -and $scenario -ceq 'stopped-description-unknown')) { throw 'injected CIM query failure' }
  if ($null -eq $object -or ($waitCompleted -and $scenario -ceq 'final-missing')) { return $null }
  $state = if ($waitCompleted -and $scenario -ceq 'final-stopped') { 'Stopped' } else { $serviceState }
  return [pscustomobject]@{State=$state;PathName=$(if ($null -ne $old) {$old.PathName} else {$binPath})}
}
function Set-InertState($state) { $script:serviceState=$state; $script:running=$state -ceq 'Running' }
function Inert-Status($state) { return [Enum]::Parse([ServiceProcess.ServiceControllerStatus],$state.Replace(' ','')) }
function Get-Service {
  [CmdletBinding()]
  param([string]$Name)
  Assert ($Name -ceq $service) 'unexpected status target'
  $script:statusQueries++
  if ($scenario -ceq 'controller-query-unknown') { throw 'injected status query failure' }
  $controller=[pscustomobject]@{Status=(Inert-Status $serviceState)}
  Add-Member -InputObject $controller -MemberType ScriptMethod -Name WaitForStatus -Value {
    param($desired,$timeout)
    $script:waits++
    Assert ($desired -eq [ServiceProcess.ServiceControllerStatus]::Running -and $timeout.TotalSeconds -eq 15) 'rollback wait contract changed'
    if ($scenario -ceq 'wait-query-unknown') { throw 'injected status wait query failure' }
    foreach ($state in $startStates) {
      $script:stateReads += $state; Set-InertState $state; $this.Status=Inert-Status $state
    }
    if ($this.Status -ne $desired) { throw [ServiceProcess.TimeoutException]::new('inert bounded wait timeout') }
    $script:waitCompleted=$true
    if ($scenario -ceq 'final-policy-conflict') { $script:policy.Dacl='D:(A;;GA;;;WD)' }
  }
  Add-Member -InputObject $controller -MemberType ScriptMethod -Name Refresh -Value {
    $script:refreshes++
    if ($scenario -ceq 'refresh-unknown') { throw 'injected refresh query failure' }
    if ($scenario -ceq 'refresh-pending') { Set-InertState 'Start Pending' }
    if ($scenario -ceq 'refresh-stopped') { Set-InertState 'Stopped' }
    if ($scenario -ceq 'refresh-cim-disagree') { $this.Status=[ServiceProcess.ServiceControllerStatus]::Stopped; return }
    $this.Status=Inert-Status $serviceState
  }
  Add-Member -InputObject $controller -MemberType ScriptMethod -Name Dispose -Value { $script:disposes++ }
  return $controller
}
function Stop-ServiceChecked { if ($null -ne $object) { $script:stops++; Set-InertState 'Stopped' } }
function Delete-ServiceChecked { $script:deletes++; $script:object=$null }
function New-Service($Name,$BinaryPathName,$StartupType) { $script:news++; $script:object=[pscustomobject]@{Identity='fresh';Command=$BinaryPathName}; $script:policy=$freshPolicy }
function Configure-Service { $script:configures++ }
function Assert-NativeExit($message) { if ($LASTEXITCODE -ne 0) { throw $message } }
function Inert-Sc($command,$name) {
  Assert ($command -ceq 'start' -and $name -ceq $service) 'unexpected native command'
  $script:starts++; $global:LASTEXITCODE=$startExit
  if ($startExit -eq 0) { Set-InertState $(if ($scenario -ceq 'immediate-running') { 'Running' } else { 'Start Pending' }) }
}
$sc = 'Inert-Sc'
function Seal-File($path) { Assert ([IO.File]::Exists($path)) 'restore missing file' }
function Restore-Registration($snapshot) {
  [IO.File]::WriteAllBytes($snapshot.Path,[IO.File]::ReadAllBytes($snapshot.Backup))
  $script:rollbackRestored=$true
  if ($scenario -ceq 'stopped-became-running') { Set-InertState 'Running' }
}
function Invoke-ActivationFailure {
  try {
    throw 'inert activation failure'
  }
  catch { throw 'PRODUCTION_SERVICE_RECOVERY_NOT_INJECTED' }
  return [pscustomobject]@{ExitCode=$exitCode;KeepRecovery=$keepRecovery}
}
function Fixture($wasActive) {
  $script:old=[pscustomobject]@{State=$(if ($wasActive) {'Running'} else {'Stopped'});PathName=$binPath}
  $script:object=[pscustomobject]@{Identity=[Guid]::NewGuid().ToString();Account='LocalSystem';Dacl=$expectedServiceDacl;Sacl='retained audit bytes';Dependencies=@();DelayedAutoStart=$true;Extension='retained SCM extension'}
  $script:policy=[pscustomobject]@{Account='LocalSystem';Command=$binPath;Display=$service;EmptyDependencies=$true;Type=16;StartType=3;ErrorControl=1;Tag=0;LoadGroup='';Owner='S-1-5-18';Dacl=$expectedServiceDacl;RecoveryReset=86400;RecoveryCommand='';RebootMessage='';ActionTypes=@(1,1,1);ActionDelays=@(5000,5000,5000);FailureOnNonCrash=$false;SidType=0;LaunchProtected=0;EmptyPrivileges=$true}
  $script:wasRunning=$wasActive; $script:running=$wasActive; $script:unknown=$false; $script:descriptionUnknown=$false
  $script:serviceState=if ($wasActive) {'Running'} else {'Stopped'}
  $script:scenario='pending-running'; $script:startStates=@('Start Pending','Running'); $script:startExit=0
  $global:LASTEXITCODE=0
  $script:stateReads=@(); $script:statusQueries=0; $script:waits=0; $script:refreshes=0; $script:disposes=0
  $script:waitCompleted=$false; $script:rollbackRestored=$false
  $script:stops=0; $script:deletes=0; $script:news=0; $script:configures=0; $script:starts=0
  $script:mutated=$false; $script:newServiceCreated=$false; $script:serviceCreateAttempted=$false
}
$cases = 0
foreach ($bad in @('account','command','cim-command','owner','dacl','recovery','noncrash','privileges','dependencies','display','query','description-query')) {
  Fixture $true
  switch ($bad) {
    'account' {$policy.Account='NT AUTHORITY\LocalService'}
    'command' {$policy.Command='C:\external\replacement.exe'}
    'cim-command' {$old.PathName='C:\external\replacement.exe'}
    'owner' {$policy.Owner='S-1-5-21-1-2-3-1000'}
    'dacl' {$policy.Dacl='D:(A;;GA;;;WD)'}
    'recovery' {$policy.ActionDelays[0]=1}
    'noncrash' {$policy.FailureOnNonCrash=$true}
    'privileges' {$policy.EmptyPrivileges=$false}
    'dependencies' {$policy.EmptyDependencies=$false}
    'display' {$policy.Display='External service'}
    'query' {$script:unknown=$true}
    'description-query' {$script:descriptionUnknown=$true}
  }
  $identity=$object
  $failed=$false; try { Begin-ServiceReplacement } catch { $failed=$true }
  Assert $failed "accepted $bad"
  Assert (-not $mutated -and $stops -eq 0 -and $deletes -eq 0 -and $news -eq 0 -and $configures -eq 0 -and $starts -eq 0) "mutation before rejection: $bad"
  Assert ([Object]::ReferenceEquals($identity,$object)) 'rejected service replaced'
  Assert $running 'rejected service stopped'
  $cases++
}
$root=Join-Path ([IO.Path]::GetTempPath()) ('polaris-inert-scm.'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
try {
  foreach ($active in @($true,$false)) {
    Fixture $active
    $identity=$object; $config=$object | ConvertTo-Json -Depth 5 -Compress
    $path=Join-Path $root 'helper.fixture'; $backup=Join-Path $root 'helper.rollback'
    $registration=Join-Path $root 'registration.fixture'; $regBackup=Join-Path $root 'registration.rollback'
    [IO.File]::WriteAllText($path,'old trusted payload'); Copy-Item $path $backup -Force
    [IO.File]::WriteAllText($registration,'old registration'); Copy-Item $registration $regBackup -Force
    $snapshots=@(@{Path=$path;Backup=$backup;Present=$true})
    $registrationSnapshot=@{Path=$registration;Backup=$regBackup;Present=$true}; $registrationChanged=$true
    Begin-ServiceReplacement
    [IO.File]::WriteAllText($path,'replacement payload')
    [IO.File]::WriteAllText($registration,'replacement registration')
    Register-ProductService
    Assert ([Object]::ReferenceEquals($identity,$object)) 'existing SCM object recreated during upgrade'
    Assert ($news -eq 0 -and $deletes -eq 0 -and $configures -eq 0) 'existing SCM configuration normalized'
    # Inject activation failure and execute the production rollback body.
    # PRODUCTION_SERVICE_ROLLBACK
    Assert ([IO.File]::ReadAllText($path) -ceq 'old trusted payload') 'payload rollback lost'
    Assert ([IO.File]::ReadAllText($registration) -ceq 'old registration') 'registration rollback lost'
    Assert ([Object]::ReferenceEquals($identity,$object)) 'rollback recreated the SCM object'
    Assert (($object | ConvertTo-Json -Depth 5 -Compress) -ceq $config) 'SCM configuration or identity changed'
    Assert ($news -eq 0 -and $deletes -eq 0 -and $configures -eq 0) 'rollback rebuilt/configured existing SCM'
    Assert ($running -eq $active -and $starts -eq [int]$active) 'original running state lost'
    $cases++
  }
  Fixture $false; $freshPolicy=$policy; $freshPolicy.StartType=2; $script:old=$null; $script:object=$null; $script:policy=$null
  Begin-ServiceReplacement; Register-ProductService
  Assert ($news -eq 1 -and $configures -eq 1 -and $deletes -eq 0) 'fresh creation path unavailable'
  Stop-CandidateServiceForRollback
  Assert ($deletes -eq 1) 'fresh candidate not retired on rollback'
  $cases++
  Fixture $false; $freshPolicy=$policy; $script:old=$null; $script:object=$null; $script:policy=$null
  Begin-ServiceReplacement; Register-ProductService; $script:unknown=$true
  $failed=$false; try { Stop-CandidateServiceForRollback } catch { $failed=$true }
  Assert ($failed -and $deletes -eq 0 -and $stops -eq 0) 'fresh candidate query failure allowed Stop/Delete'
  $cases++
  Fixture $false; $script:old=$null; $script:object=$null; $script:policy=$null; $script:serviceCreateAttempted=$true
  $failed=$false; try { Stop-CandidateServiceForRollback } catch { $failed=$true }
  Assert ($failed -and $deletes -eq 0 -and $stops -eq 0) 'unknown creation deleted a possibly foreign object'
  $cases++
  $recoveryCases=0
  foreach ($case in @(
    @{Name='pending-running';Active=$true;Confirmed=$true},
    @{Name='immediate-running';Active=$true;Confirmed=$true},
    @{Name='pending-stopped';Active=$true;Confirmed=$false},
    @{Name='pending-timeout';Active=$true;Confirmed=$false},
    @{Name='controller-query-unknown';Active=$true;Confirmed=$false},
    @{Name='wait-query-unknown';Active=$true;Confirmed=$false},
    @{Name='refresh-unknown';Active=$true;Confirmed=$false},
    @{Name='refresh-pending';Active=$true;Confirmed=$false},
    @{Name='refresh-stopped';Active=$true;Confirmed=$false},
    @{Name='refresh-cim-disagree';Active=$true;Confirmed=$false},
    @{Name='final-description-unknown';Active=$true;Confirmed=$false},
    @{Name='final-stopped';Active=$true;Confirmed=$false},
    @{Name='final-missing';Active=$true;Confirmed=$false},
    @{Name='final-policy-unknown';Active=$true;Confirmed=$false},
    @{Name='final-policy-conflict';Active=$true;Confirmed=$false},
    @{Name='start-exit-failure';Active=$true;Confirmed=$false},
    @{Name='stopped';Active=$false;Confirmed=$true},
    @{Name='stopped-became-running';Active=$false;Confirmed=$false},
    @{Name='stopped-policy-unknown';Active=$false;Confirmed=$false},
    @{Name='stopped-description-unknown';Active=$false;Confirmed=$false}
  )) {
    Fixture $case.Active
    $script:scenario=$case.Name
    switch ($scenario) {
      'immediate-running' {$script:startStates=@('Running')}
      'pending-stopped' {$script:startStates=@('Start Pending','Stopped')}
      'pending-timeout' {$script:startStates=@('Start Pending')}
      'start-exit-failure' {$script:startExit=5}
    }
    $identity=$object; $config=$object | ConvertTo-Json -Depth 5 -Compress
    $tx=Join-Path $root ('recovery.'+$recoveryCases)
    New-Item -ItemType Directory -Path $tx | Out-Null
    $path=Join-Path $root 'helper.fixture'; $backup=Join-Path $tx 'helper.rollback'
    $registration=Join-Path $root 'registration.fixture'; $regBackup=Join-Path $tx 'registration.rollback'
    [IO.File]::WriteAllText($path,'old trusted payload'); Copy-Item $path $backup -Force
    [IO.File]::WriteAllText($registration,'old registration'); Copy-Item $registration $regBackup -Force
    $snapshots=@(@{Path=$path;Backup=$backup;Present=$true})
    $registrationSnapshot=@{Path=$registration;Backup=$regBackup;Present=$true}; $registrationChanged=$true
    $keepRecovery=$false; $exitCode=0; $failureStage=86; $admitted=$false; $lock=$null
    Begin-ServiceReplacement
    [IO.File]::WriteAllText($path,'replacement payload')
    [IO.File]::WriteAllText($registration,'replacement registration')
    Register-ProductService
    # This executes the generated production catch/finally, not a fixture copy
    # of keepRecovery/exitCode or the transaction deletion condition.
    $recovery=Invoke-ActivationFailure 2>$null
    Assert ([IO.File]::ReadAllText($path) -ceq 'old trusted payload') "payload not restored: $scenario"
    Assert ([IO.File]::ReadAllText($registration) -ceq 'old registration') "registration not restored: $scenario"
    Assert ([Object]::ReferenceEquals($identity,$object)) "SCM object replaced: $scenario"
    Assert (($object | ConvertTo-Json -Depth 5 -Compress) -ceq $config) "SCM configuration changed: $scenario"
    Assert ($news -eq 0 -and $deletes -eq 0 -and $configures -eq 0) "existing SCM normalized: $scenario"
    if ($case.Confirmed) {
      Assert ($recovery.ExitCode -eq 86 -and -not $recovery.KeepRecovery -and -not (Test-Path -LiteralPath $tx)) "confirmed recovery retained/failed: $scenario"
      Assert ($running -eq $case.Active) "restored running state differs: $scenario"
    } else {
      Assert ($recovery.ExitCode -eq 87 -and $recovery.KeepRecovery -and (Test-Path -LiteralPath $tx)) "unconfirmed recovery cleared transaction: $scenario"
      Assert ([IO.File]::ReadAllText($backup) -ceq 'old trusted payload') "payload recovery material lost: $scenario"
      Assert ([IO.File]::ReadAllText($regBackup) -ceq 'old registration') "registration recovery material lost: $scenario"
    }
    if (-not $case.Active) {
      Assert ($starts -eq 0 -and $statusQueries -eq 0 -and $waits -eq 0 -and $disposes -eq 0) "stopped service restarted: $scenario"
    } elseif ($scenario -ceq 'start-exit-failure') {
      Assert ($starts -eq 1 -and $statusQueries -eq 0 -and $waits -eq 0) 'failed start waited or retried'
    } else {
      Assert ($starts -eq 1 -and $statusQueries -eq 1) "start/status count differs: $scenario"
      if ($scenario -ceq 'controller-query-unknown') { Assert ($waits -eq 0 -and $disposes -eq 0) 'unknown query yielded controller' }
      else { Assert ($waits -eq 1 -and $disposes -eq 1) "status handle not closed: $scenario" }
    }
    if ($scenario -ceq 'pending-running') { Assert (($stateReads -join ',') -ceq 'Start Pending,Running') 'asynchronous sequence not consumed' }
    $recoveryCases++
  }
} finally { Remove-Item -LiteralPath $root -Recurse -Force }
Write-Output "Passed $cases inert SCM cases."
Write-Output "Passed $recoveryCases rollback state/transaction cases."
