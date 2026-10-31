# Read native SCM fields, never localized sc.exe display text. Existing services
# keep their original object (including credentials/SACL/SCM extension data).
# Query failure is not permission to stop or normalize that object.
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
public sealed class PolarisServicePolicy {
  public string Account, Command, Display, Owner, Dacl, LoadGroup, RecoveryCommand, RebootMessage;
  public uint Type, StartType, ErrorControl, Tag, RecoveryReset, SidType, LaunchProtected;
  public bool FailureOnNonCrash, EmptyPrivileges, EmptyDependencies;
  public uint[] ActionTypes, ActionDelays;
  [StructLayout(LayoutKind.Sequential)] struct Config {
    public uint Type, Start, Error; public IntPtr Binary, Group; public uint Tag;
    public IntPtr Dependencies, Account, Display;
  }
  [StructLayout(LayoutKind.Sequential)] struct Failure {
    public uint Reset; public IntPtr Reboot, Command; public uint Count; public IntPtr Actions;
  }
  [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr OpenSCManager(string machine, string database, uint access);
  [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr OpenService(IntPtr manager, string name, uint access);
  [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool QueryServiceConfig(IntPtr service, IntPtr buffer, uint size, out uint needed);
  [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool QueryServiceConfig2(IntPtr service, uint level, IntPtr buffer, uint size, out uint needed);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool QueryServiceObjectSecurity(IntPtr service, uint information, IntPtr buffer, uint size, out uint needed);
  [DllImport("advapi32.dll")] static extern bool CloseServiceHandle(IntPtr handle);
  static IntPtr Buffer(IntPtr service, int level, out uint length) {
    length = 0;
    uint size = 0; bool ok;
    if (level == -1) ok = QueryServiceConfig(service, IntPtr.Zero, 0, out size);
    else if (level == -2) ok = QueryServiceObjectSecurity(service, 5, IntPtr.Zero, 0, out size);
    else ok = QueryServiceConfig2(service, (uint)level, IntPtr.Zero, 0, out size);
    if (ok || Marshal.GetLastWin32Error() != 122 || size == 0 || size > 65536) throw new Win32Exception(Marshal.GetLastWin32Error(), "SCM policy size unavailable");
    IntPtr memory = Marshal.AllocHGlobal((int)size);
    try {
      uint needed;
      if (level == -1) ok = QueryServiceConfig(service, memory, size, out needed);
      else if (level == -2) ok = QueryServiceObjectSecurity(service, 5, memory, size, out needed);
      else ok = QueryServiceConfig2(service, (uint)level, memory, size, out needed);
      if (!ok || needed > size) throw new Win32Exception(Marshal.GetLastWin32Error(), "SCM policy query failed");
      length = size;
      return memory;
    } catch { Marshal.FreeHGlobal(memory); throw; }
  }
  static string Text(IntPtr value) { return value == IntPtr.Zero ? "" : Marshal.PtrToStringUni(value); }
  static uint Word(IntPtr service, int level) {
    uint length; IntPtr memory = Buffer(service, level, out length);
    try { return unchecked((uint)Marshal.ReadInt32(memory)); } finally { Marshal.FreeHGlobal(memory); }
  }
  public static PolarisServicePolicy Read(string name) {
    IntPtr manager = OpenSCManager(null, null, 1);
    if (manager == IntPtr.Zero) throw new Win32Exception();
    IntPtr service = IntPtr.Zero;
    try {
      service = OpenService(manager, name, 0x20001); // READ_CONTROL | QUERY_CONFIG
      if (service == IntPtr.Zero) {
        int error = Marshal.GetLastWin32Error();
        if (error == 1060) return null;
        throw new Win32Exception(error);
      }
      PolarisServicePolicy result = new PolarisServicePolicy();
      uint length; IntPtr memory = Buffer(service, -1, out length);
      try {
        Config config = (Config)Marshal.PtrToStructure(memory, typeof(Config));
        result.Account = Text(config.Account); result.Command = Text(config.Binary); result.Display = Text(config.Display);
        result.EmptyDependencies = config.Dependencies == IntPtr.Zero || Marshal.ReadInt16(config.Dependencies) == 0;
        result.Type = config.Type; result.StartType = config.Start; result.ErrorControl = config.Error;
        result.LoadGroup = Text(config.Group); result.Tag = config.Tag;
      } finally { Marshal.FreeHGlobal(memory); }
      memory = Buffer(service, -2, out length);
      try {
        // The self-relative length is bounded by the successful native query.
        byte[] bytes = new byte[length]; Marshal.Copy(memory, bytes, 0, (int)length);
        RawSecurityDescriptor security = new RawSecurityDescriptor(bytes, 0);
        if (security.Owner == null || security.DiscretionaryAcl == null) throw new InvalidOperationException("SCM owner/DACL unavailable");
        result.Owner = security.Owner.Value; result.Dacl = security.GetSddlForm(AccessControlSections.Access);
      } finally { Marshal.FreeHGlobal(memory); }
      memory = Buffer(service, 2, out length);
      try {
        Failure failure = (Failure)Marshal.PtrToStructure(memory, typeof(Failure));
        if (failure.Count != 3 || failure.Actions == IntPtr.Zero) throw new InvalidOperationException("unsupported SCM recovery actions");
        result.RecoveryReset = failure.Reset; result.RecoveryCommand = Text(failure.Command); result.RebootMessage = Text(failure.Reboot);
        result.ActionTypes = new uint[3]; result.ActionDelays = new uint[3];
        for (int i = 0; i < 3; i++) { result.ActionTypes[i] = unchecked((uint)Marshal.ReadInt32(failure.Actions, i*8)); result.ActionDelays[i] = unchecked((uint)Marshal.ReadInt32(failure.Actions, i*8+4)); }
      } finally { Marshal.FreeHGlobal(memory); }
      result.FailureOnNonCrash = Word(service, 4) != 0;
      result.SidType = Word(service, 5); result.LaunchProtected = Word(service, 12);
      memory = Buffer(service, 6, out length);
      try { IntPtr privileges = Marshal.ReadIntPtr(memory); result.EmptyPrivileges = privileges == IntPtr.Zero || Marshal.ReadInt16(privileges) == 0; }
      finally { Marshal.FreeHGlobal(memory); }
      return result;
    } finally { if (service != IntPtr.Zero) CloseServiceHandle(service); CloseServiceHandle(manager); }
  }
}
'@
$expectedServiceDacl = ([Security.AccessControl.RawSecurityDescriptor]::new('D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)(A;;RP;;;IU)')).GetSddlForm([Security.AccessControl.AccessControlSections]::Access)
function Get-ServicePolicySnapshot { return [PolarisServicePolicy]::Read($service) }
function Get-ServiceDescription { Get-CimInstance -ClassName Win32_Service -Filter "Name='$service'" -ErrorAction Stop }
function Assert-SupportedExistingService($oldService) {
  $policy = Get-ServicePolicySnapshot
  if ($null -eq $oldService) {
    if ($null -ne $policy) { throw 'SCM service appeared outside this installation' }
    return
  }
  if ($null -eq $policy -or $oldService.State -cnotin @('Running', 'Stopped') -or
      $policy.Account -cnotin @('LocalSystem', 'NT AUTHORITY\SYSTEM') -or
      $policy.Command -cne $binPath -or $oldService.PathName -cne $binPath -or
      $policy.Display -cne $service -or $policy.EmptyDependencies -ne $true -or
      $policy.Type -ne 16 -or $policy.StartType -notin @(2, 3) -or
      $policy.ErrorControl -ne 1 -or $policy.Tag -ne 0 -or $policy.LoadGroup -cne '' -or
      $policy.Owner -cnotin @('S-1-5-18', 'S-1-5-32-544') -or $policy.Dacl -cne $expectedServiceDacl -or
      $policy.RecoveryReset -ne 86400 -or $policy.RecoveryCommand -cne '' -or $policy.RebootMessage -cne '' -or
      $policy.ActionTypes.Count -ne 3 -or $policy.ActionDelays.Count -ne 3 -or
      $policy.FailureOnNonCrash -ne $false -or $policy.SidType -ne 0 -or
      $policy.LaunchProtected -ne 0 -or $policy.EmptyPrivileges -ne $true) { throw 'unsupported or unknown existing SCM configuration; preserved without mutation' }
  for ($index = 0; $index -lt 3; $index++) {
    if ($policy.ActionTypes[$index] -ne 1 -or $policy.ActionDelays[$index] -ne 5000) { throw 'unsupported SCM recovery policy; preserved without mutation' }
  }
}
function Assert-CurrentProductService {
  $current = Get-ServiceDescription
  if ($null -eq $current) { throw 'SCM object disappeared; recovery required' }
  Assert-SupportedExistingService $current
}
# These are the actual transaction seams, also exercised with inert SCM ops.
function Begin-ServiceReplacement {
  if ($null -eq $old) { Assert-SupportedExistingService $null }
  else { Assert-CurrentProductService }
  $script:mutated = $true
  Stop-ServiceChecked
}
function Register-ProductService {
  if ($null -eq $old) {
    $script:serviceCreateAttempted = $true
    New-Service -Name $service -BinaryPathName $binPath -StartupType Automatic | Out-Null
    $script:newServiceCreated = $true
    Configure-Service
  }
  Assert-CurrentProductService
}
function Stop-CandidateServiceForRollback {
  if ($null -ne $old) { Assert-CurrentProductService; Stop-ServiceChecked }
  elseif ($newServiceCreated) {
    Assert-CurrentProductService
    Stop-ServiceChecked
    Delete-ServiceChecked
  }
  elseif ($serviceCreateAttempted) { throw 'SCM creation completion unknown; recovery required' }
}
function Restore-PreviousService {
  if ($null -ne $old) {
    Assert-CurrentProductService
    if ($wasRunning) {
      & $sc start $service | Out-Null
      Assert-NativeExit 'SCM rollback start'
      $restored = Get-Service -Name $service -ErrorAction Stop
      if ($null -eq $restored) { throw 'SCM rollback status unavailable; recovery required' }
      try {
        $restored.WaitForStatus([ServiceProcess.ServiceControllerStatus]::Running, [TimeSpan]::FromSeconds(15))
        $restored.Refresh()
        if ($restored.Status -ne [ServiceProcess.ServiceControllerStatus]::Running) { throw 'SCM rollback running state not confirmed; recovery required' }
      } finally { $restored.Dispose() }
    }
    $current = Get-ServiceDescription
    $requiredState = if ($wasRunning) { 'Running' } else { 'Stopped' }
    if ($null -eq $current -or $current.State -cne $requiredState) { throw 'SCM rollback state not confirmed; recovery required' }
    Assert-SupportedExistingService $current
  }
}
