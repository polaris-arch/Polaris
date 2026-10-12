//! Root-sealed installation on Windows. No service mutation occurs until the
//! native helper atomically admits replacement with an empty custody slot.
use super::*;

pub(super) fn build_win_install_script(
    paths: &InstallPaths,
    params: &InstallParams,
    token: &str,
) -> String {
    let helper = params.src_binary.to_string_lossy();
    let payload = helper
        .rsplit_once(['\\', '/'])
        .expect("staged helper parent")
        .0;
    let bin_path = format!(
        "\"{}\" --support \"{}\"",
        paths.binary.to_string_lossy(),
        WIN_SUPPORT_DIR
    );
    format!(
        r#"$ErrorActionPreference = 'Stop'
$sc = "$env:SystemRoot\System32\sc.exe"
$icacls = "$env:SystemRoot\System32\icacls.exe"
$support = '{support}'
$coreDir = '{core}'
$helperDst = '{destination}'
$cleanerDst = Join-Path $support 'polaris-cleaner.exe'
$record = Join-Path $support 'helper-installed.json'
$service = '{service}'
$identity = '{identity}'
$binPath = '{bin_path}'
function Assert-NativeExit($message) {{ if ($LASTEXITCODE -ne 0) {{ throw "$message (exit $LASTEXITCODE)" }} }}
function Assert-Regular($path) {{
  $item = Get-Item -LiteralPath $path -Force
  if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or $item.PSIsContainer) {{ throw "invalid installation object: $path" }}
}}
function Secure-Directory($path, $public) {{
  if (-not (Test-Path -LiteralPath $path)) {{ New-Item -ItemType Directory -Path $path | Out-Null }}
  $item = Get-Item -LiteralPath $path -Force
  if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {{ throw "invalid installation directory: $path" }}
  & $icacls $path /setowner '*S-1-5-32-544' | Out-Null
  Assert-NativeExit 'directory owner'
  # Construct a complete replacement DACL. grant:r alone retains unrelated
  # explicit ACEs on an older or precreated directory.
  $acl = [Security.AccessControl.DirectorySecurity]::new()
  $acl.SetAccessRuleProtection($true, $false)
  $acl.SetOwner(([Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')))
  foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {{
    $rule = [Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($sid), [Security.AccessControl.FileSystemRights]::FullControl, [Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit', [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
    $acl.AddAccessRule($rule)
  }}
  if ($public) {{
    $rule = [Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new('S-1-5-32-545'), [Security.AccessControl.FileSystemRights]::ReadAndExecute, [Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit', [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
    $acl.AddAccessRule($rule)
  }}
  Set-Acl -LiteralPath $path -AclObject $acl
}}
function Seal-File($path) {{
  Assert-Regular $path
  & $icacls $path /setowner '*S-1-5-32-544' | Out-Null
  Assert-NativeExit 'file owner'
  $acl = [Security.AccessControl.FileSecurity]::new()
  $acl.SetAccessRuleProtection($true, $false)
  $acl.SetOwner(([Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')))
  foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {{
    $rule = [Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($sid), [Security.AccessControl.FileSystemRights]::FullControl, [Security.AccessControl.AccessControlType]::Allow)
    $acl.AddAccessRule($rule)
  }}
  Set-Acl -LiteralPath $path -AclObject $acl
}}
function Check-Hash($path, $expected) {{
  Assert-Regular $path
  if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $expected) {{ throw "installation byte mismatch: $path" }}
}}
function Invoke-Utility([string[]]$arguments) {{
  $result = & $utility @arguments
  Assert-NativeExit ('helper ' + $arguments[0])
  return $result
}}
function Stop-ServiceChecked {{
  $current = Get-Service -Name $service -ErrorAction SilentlyContinue
  if ($null -ne $current -and $current.Status -ne 'Stopped') {{
    & $sc stop $service | Out-Null
    Assert-NativeExit 'SCM stop'
    $current.WaitForStatus([ServiceProcess.ServiceControllerStatus]::Stopped, [TimeSpan]::FromSeconds(15))
  }}
}}
function Delete-ServiceChecked {{
  if ($null -eq (Get-Service -Name $service -ErrorAction SilentlyContinue)) {{ return }}
  & $sc delete $service | Out-Null
  Assert-NativeExit 'SCM delete'
  $deadline = (Get-Date).AddSeconds(15)
  while ($null -ne (Get-Service -Name $service -ErrorAction SilentlyContinue)) {{
    if ((Get-Date) -ge $deadline) {{ throw 'SCM deletion timeout' }}
    Start-Sleep -Milliseconds 300
  }}
}}
function Configure-Service {{
  & $sc sdset $service 'D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)(A;;RP;;;IU)S:(AU;FA;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;WD)' | Out-Null
  Assert-NativeExit 'SCM start permission'
  & $sc failure $service reset= 86400 actions= restart/5000/restart/5000/restart/5000 | Out-Null
  Assert-NativeExit 'SCM recovery'
}}
Secure-Directory $support $true
$lockPath = Join-Path $support 'helper-install.lock'
if (Test-Path -LiteralPath $lockPath) {{ Assert-Regular $lockPath }}
$lock = [IO.File]::Open($lockPath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
$tx = $null
$utility = $null
$admitted = $false
$mutated = $false
$keepHelper = $false
$keepRecovery = $false
try {{
  $tx = Join-Path $support ('helper-upgrade.' + [Guid]::NewGuid().ToString('N'))
  Secure-Directory $tx $false
  $utility = Join-Path $tx 'polaris-helper.exe'
  $manifest = Join-Path $tx 'core-manifest.json'
  $cleaner = Join-Path $tx 'polaris-cleaner.exe'
  Copy-Item -LiteralPath '{helper}' -Destination $utility
  Copy-Item -LiteralPath '{manifest}' -Destination $manifest
  Seal-File $utility
  Seal-File $manifest
  Check-Hash $utility '{helper_hash}'
  Check-Hash $manifest '{manifest_hash}'
  $selection = Get-Content -LiteralPath $manifest -Raw -Encoding UTF8 | ConvertFrom-Json
  if ($selection.id -cne '{core_id}' -or $selection.core.platform -cne 'win' -or $selection.core.cleanerSha256 -cnotmatch '^[0-9a-f]{{64}}$') {{ throw 'invalid sealed Windows installation selection' }}
  Copy-Item -LiteralPath '{payload}\polaris-cleaner.exe' -Destination $cleaner
  Seal-File $cleaner
  Check-Hash $cleaner $selection.core.cleanerSha256
  $tokenFile = Join-Path $tx 'user-token'
  [IO.File]::WriteAllText($tokenFile, '{token}', [Text.Encoding]::ASCII)
  Seal-File $tokenFile
  Secure-Directory $coreDir $true
  $users = Join-Path $support 'users'
  Secure-Directory $users $false
  Secure-Directory (Join-Path $users $identity) $false
  Invoke-Utility @('core-install', '--root', $coreDir, '--source', '{payload}', '--manifest', $manifest, '--source-owner', $identity) | Out-Null
  Invoke-Utility @('register-user', '--support', $support, '--identity', $identity, '--confdir', '{config}', '--token-file', $tokenFile) | Out-Null
  $policy = Invoke-Utility @('helper-policy', '--binary', $helperDst, '--metadata', $record)
  if ($policy -ceq 'helper-policy retain') {{ $keepHelper = $true }}
  if (-not $keepHelper -and $policy -cne 'helper-policy replace' -and $policy -cne 'helper-policy replace-legacy') {{ throw 'invalid installed helper policy' }}
  $old = Get-CimInstance -ClassName Win32_Service -Filter "Name='$service'" -ErrorAction Stop
  $wasRunning = $null -ne $old -and $old.State -ne 'Stopped'
  if ($keepHelper -and $wasRunning) {{
    & $utility 'retain-probe' '--support' $support '--id' '{core_id}' | Out-Null
    if ($LASTEXITCODE -eq 0) {{ Write-Output 'polaris-helper-core-only-ok'; return }}
  }}
  $snapshots = @()
  $replaceObjects = if ($keepHelper) {{ @() }} else {{ @($helperDst, $cleanerDst, $record) }}
  foreach ($path in $replaceObjects) {{
    $backup = Join-Path $tx ((Split-Path -Leaf $path) + '.rollback')
    $present = Test-Path -LiteralPath $path
    if ($present) {{ Assert-Regular $path; Copy-Item -LiteralPath $path -Destination $backup; Seal-File $backup }}
    $snapshots += @{{ Path = $path; Backup = $backup; Present = $present }}
  }}
  if ($wasRunning) {{
    Invoke-Utility @('prepare-upgrade', '--support', $support) | Out-Null
    $admitted = $true
  }}
  $mutated = $true
  Stop-ServiceChecked
  Delete-ServiceChecked
  if (-not $keepHelper) {{
  $copied = $false
  for ($i = 0; $i -lt 10 -and -not $copied; $i++) {{
    try {{ Copy-Item -LiteralPath $utility -Destination $helperDst -Force; Copy-Item -LiteralPath $cleaner -Destination $cleanerDst -Force; $copied = $true }}
    catch {{ if ($i -eq 9) {{ throw }}; Start-Sleep -Milliseconds 300 }}
  }}
  Seal-File $helperDst
  Seal-File $cleanerDst
  Check-Hash $helperDst '{helper_hash}'
  Check-Hash $cleanerDst $selection.core.cleanerSha256
  Invoke-Utility @('helper-record', '--binary', $helperDst, '--metadata', $record) | Out-Null
  }}
  New-Service -Name $service -BinaryPathName $binPath -StartupType Automatic | Out-Null
  Configure-Service
  & $sc start $service | Out-Null
  Assert-NativeExit 'SCM start'
  (Get-Service -Name $service).WaitForStatus([ServiceProcess.ServiceControllerStatus]::Running, [TimeSpan]::FromSeconds(15))
  $admitted = $false
  Write-Output 'polaris-helper-install-ok'
}} catch {{
  $failure = $_
  try {{
    if ($mutated) {{
      Stop-ServiceChecked
      Delete-ServiceChecked
      foreach ($snapshot in $snapshots) {{
        if ($snapshot.Present) {{ Copy-Item -LiteralPath $snapshot.Backup -Destination $snapshot.Path -Force; Seal-File $snapshot.Path }}
        elseif (Test-Path -LiteralPath $snapshot.Path) {{ Remove-Item -LiteralPath $snapshot.Path -Force }}
      }}
      if ($null -ne $old) {{
        $startType = switch ($old.StartMode) {{ 'Disabled' {{ 'Disabled' }} 'Manual' {{ 'Manual' }} default {{ 'Automatic' }} }}
        New-Service -Name $service -BinaryPathName $old.PathName -StartupType $startType | Out-Null
        Configure-Service
        if ($wasRunning) {{ & $sc start $service | Out-Null; Assert-NativeExit 'SCM rollback start' }}
      }}
    }} elseif ($admitted) {{ Invoke-Utility @('cancel-upgrade', '--support', $support) | Out-Null }}
  }} catch {{
    $keepRecovery = $true
    Write-Error -ErrorAction Continue "helper rollback incomplete; recovery transaction retained: $tx; $($_.Exception.Message)"
  }}
  throw $failure
}} finally {{
  try {{ if ($null -ne $tx -and -not $keepRecovery) {{ Remove-Item -LiteralPath $tx -Recurse -Force }} }}
  finally {{ $lock.Dispose() }}
}}
"#,
        support = ps_quote(WIN_SUPPORT_DIR),
        core = ps_quote(&paths.core_dir.to_string_lossy()),
        destination = ps_quote(&paths.binary.to_string_lossy()),
        service = WIN_SERVICE_NAME,
        identity = ps_quote(&params.user_identity.key()),
        bin_path = ps_quote(&bin_path),
        helper = ps_quote(&helper),
        manifest = ps_quote(&params.manifest_path.to_string_lossy()),
        helper_hash = params.helper_sha256,
        manifest_hash = params.manifest_sha256,
        core_id = params.core_id.as_str(),
        payload = ps_quote(payload),
        config = ps_quote(&params.conf_dir.to_string_lossy()),
        token = ps_quote(token),
    )
}
