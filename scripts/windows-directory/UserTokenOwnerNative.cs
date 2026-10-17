// Windows acceptance fixture only. Never linked into the product or kernel.
using System;
using System.Collections;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.Threading;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text;

namespace Polaris.WindowsAcceptance {
    public sealed class TokenSnapshot {
        public string User;
        public string Owner;
        public string TokenId;
        public int TokenType;
    }

    public sealed class PrivilegeSnapshot {
        public string Name;
        public bool Enabled, EnabledByDefault, Removed, UsedForAccess;
    }

    public static class Native {
        internal const uint TOKEN_QUERY = 0x0008;
        internal const uint TOKEN_DUPLICATE = 0x0002;
        internal const uint TOKEN_ASSIGN_PRIMARY = 0x0001;
        internal const uint TOKEN_ADJUST_DEFAULT = 0x0080;
        internal const uint READ_CONTROL = 0x00020000;
        internal const uint OWNER_GROUP_DACL = 0x00000007; // No SACL / privilege request.
        internal const int TokenUser = 1, TokenOwner = 4, TokenType = 8, TokenStatistics = 10, TokenPrivileges = 3;
        internal const int TokenDefaultDacl = 6, TokenSessionId = 12, TokenIntegrityLevel = 25;
        internal const uint CREATE_SUSPENDED = 0x00000004, CREATE_UNICODE_ENVIRONMENT = 0x00000400;
        internal const uint WAIT_OBJECT_0 = 0, WAIT_TIMEOUT = 258;

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        internal struct STARTUPINFO {
            public int cb;
            public string lpReserved, lpDesktop, lpTitle;
            public uint dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
            public ushort wShowWindow, cbReserved2;
            public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
        }
        [StructLayout(LayoutKind.Sequential)]
        internal struct PROCESS_INFORMATION {
            public IntPtr hProcess, hThread;
            public uint dwProcessId, dwThreadId;
        }
        [StructLayout(LayoutKind.Sequential)]
        internal struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
            public long PerProcessUserTimeLimit, PerJobUserTimeLimit;
            public uint LimitFlags;
            public UIntPtr MinimumWorkingSetSize, MaximumWorkingSetSize;
            public uint ActiveProcessLimit;
            public UIntPtr Affinity;
            public uint PriorityClass, SchedulingClass;
        }
        [StructLayout(LayoutKind.Sequential)]
        internal struct JOBOBJECT_BASIC_ACCOUNTING_INFORMATION {
            public long TotalUserTime, TotalKernelTime, ThisPeriodTotalUserTime, ThisPeriodTotalKernelTime;
            public uint TotalPageFaultCount, TotalProcesses, ActiveProcesses, TotalTerminatedProcesses;
        }
        [StructLayout(LayoutKind.Sequential)]
        internal struct IO_COUNTERS {
            public ulong ReadOperationCount, WriteOperationCount, OtherOperationCount;
            public ulong ReadTransferCount, WriteTransferCount, OtherTransferCount;
        }
        [StructLayout(LayoutKind.Sequential)]
        internal struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
            public IO_COUNTERS IoInfo;
            public UIntPtr ProcessMemoryLimit, JobMemoryLimit, PeakProcessMemoryUsed, PeakJobMemoryUsed;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct LUID { public uint LowPart; public int HighPart; }
        [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
        internal static extern bool LookupPrivilegeNameW(string system, ref LUID luid, StringBuilder name, ref int length);
        [DllImport("kernel32.dll")] internal static extern IntPtr GetCurrentProcess();
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern bool CloseHandle(IntPtr handle);
        [DllImport("advapi32.dll", SetLastError=true)] internal static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
        [DllImport("advapi32.dll", SetLastError=true)] internal static extern bool GetTokenInformation(IntPtr token, int infoClass, IntPtr info, int size, out int needed);
        [DllImport("advapi32.dll", SetLastError=true)] internal static extern bool SetTokenInformation(IntPtr token, int infoClass, IntPtr info, int size);
        [DllImport("advapi32.dll", SetLastError=true)] internal static extern bool DuplicateTokenEx(IntPtr existing, uint access, IntPtr attributes, int level, int type, out IntPtr token);
        [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
        internal static extern bool CreateProcessWithTokenW(IntPtr token, uint logonFlags, string application, StringBuilder command, uint creationFlags, IntPtr environment, string directory, ref STARTUPINFO startup, out PROCESS_INFORMATION process);
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern uint ResumeThread(IntPtr thread);
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern bool GetExitCodeProcess(IntPtr process, out uint code);
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern bool TerminateProcess(IntPtr process, uint code);
        [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] internal static extern IntPtr CreateJobObjectW(IntPtr attributes, string name);
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern bool SetInformationJobObject(IntPtr job, int infoClass, ref JOBOBJECT_EXTENDED_LIMIT_INFORMATION info, uint size);
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern bool QueryInformationJobObject(IntPtr job, int infoClass, out JOBOBJECT_BASIC_ACCOUNTING_INFORMATION info, uint size, IntPtr returnLength);
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern bool TerminateJobObject(IntPtr job, uint code);
        [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] internal static extern bool CreateDirectoryW(string path, IntPtr attributes);
        [DllImport("advapi32.dll", CharSet=CharSet.Unicode)] internal static extern uint GetNamedSecurityInfoW(string name, int objectType, uint securityInfo, out IntPtr owner, out IntPtr group, out IntPtr dacl, out IntPtr sacl, out IntPtr descriptor);
        [DllImport("kernel32.dll")] internal static extern IntPtr LocalFree(IntPtr memory);
        [DllImport("kernel32.dll", SetLastError=true)] internal static extern bool DuplicateHandle(IntPtr sourceProcess, IntPtr source, IntPtr targetProcess, out IntPtr target, uint access, bool inherit, uint options);
        [DllImport("advapi32.dll")] internal static extern uint GetSecurityInfo(IntPtr handle, int objectType, uint securityInfo, out IntPtr owner, out IntPtr group, out IntPtr dacl, out IntPtr sacl, out IntPtr descriptor);
        [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] internal static extern bool ConvertSecurityDescriptorToStringSecurityDescriptorW(IntPtr descriptor, uint revision, uint securityInfo, out IntPtr sddl, out uint length);
        [DllImport("user32.dll", SetLastError=true)] internal static extern IntPtr GetProcessWindowStation();
        [DllImport("kernel32.dll")] internal static extern uint GetCurrentThreadId();
        [DllImport("user32.dll", SetLastError=true)] internal static extern IntPtr GetThreadDesktop(uint threadId);
        [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] internal static extern bool GetUserObjectInformationW(IntPtr handle, int index, IntPtr info, uint size, out uint needed);
        [DllImport("user32.dll", SetLastError=true)] internal static extern bool GetUserObjectSecurity(IntPtr handle, ref uint securityInfo, IntPtr descriptor, uint size, out uint needed);
        [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] internal static extern IntPtr OpenSCManagerW(string machine, string database, uint access);
        [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] internal static extern IntPtr OpenServiceW(IntPtr manager, string name, uint access);
        [DllImport("advapi32.dll", SetLastError=true)] internal static extern bool CloseServiceHandle(IntPtr handle);
        [DllImport("advapi32.dll", SetLastError=true)] internal static extern bool QueryServiceStatusEx(IntPtr service, int level, IntPtr info, uint size, out uint needed);
        [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] internal static extern bool QueryServiceConfigW(IntPtr service, IntPtr info, uint size, out uint needed);

        // All diagnostics are advisory, independent reads. No ACL/service/token
        // mutation, SACL access, impersonation, privilege enabling, or access test.
        // Values are limited to SIDs, SDDL, fixed paths, numeric state/flags/errors.
        internal static object Observe(Func<object> read) {
            try { return new Dictionary<string,object> { { "result", "read" }, { "value", read() } }; }
            catch (Exception error) {
                var failure = new Dictionary<string,object> { { "result", "unavailable" }, { "hresult", error.HResult } };
                var native = error as Win32Exception;
                if (native != null) failure["nativeErrorCode"] = native.NativeErrorCode;
                return failure;
            }
        }
        internal static string Sddl(IntPtr descriptor) {
            IntPtr text = IntPtr.Zero; uint length;
            try {
                Check(ConvertSecurityDescriptorToStringSecurityDescriptorW(descriptor, 1, OWNER_GROUP_DACL, out text, out length), "Read diagnostic SDDL");
                return Marshal.PtrToStringUni(text);
            } finally { if (text != IntPtr.Zero) LocalFree(text); }
        }
        internal static object TokenDefaultAcl(IntPtr token) {
            IntPtr info = Information(token, TokenDefaultDacl);
            try {
                IntPtr acl = Marshal.ReadIntPtr(info);
                // A NULL default DACL is distinct from an empty ACL or a read failure.
                if (acl == IntPtr.Zero) return new Dictionary<string,object> { { "isNull", true }, { "sddl", null } };
                int size = unchecked((ushort)Marshal.ReadInt16(acl, 2));
                if (size < 8) throw new InvalidOperationException("Invalid default ACL size");
                byte[] bytes = new byte[size]; Marshal.Copy(acl, bytes, 0, size);
                var descriptor = new RawSecurityDescriptor(ControlFlags.DiscretionaryAclPresent, null, null, null, new RawAcl(bytes, 0));
                return new Dictionary<string,object> { { "isNull", false }, { "sddl", descriptor.GetSddlForm(AccessControlSections.Access) } };
            } finally { Marshal.FreeHGlobal(info); }
        }
        internal static object TokenContext(IntPtr token) {
            return new Dictionary<string,object> {
                { "defaultDacl", Observe(() => TokenDefaultAcl(token)) },
                { "sessionId", Observe(() => SessionId(token)) },
                { "integrity", Observe(() => {
                    IntPtr info = Information(token, TokenIntegrityLevel);
                    try { return new Dictionary<string,object> {
                        { "sid", new SecurityIdentifier(Marshal.ReadIntPtr(info)).Value },
                        { "attributes", unchecked((uint)Marshal.ReadInt32(info, IntPtr.Size)) }
                    }; } finally { Marshal.FreeHGlobal(info); }
                }) }
            };
        }
        internal static object TokenObjectSecurity(IntPtr token, bool currentProcessToken) {
            IntPtr readOnly = IntPtr.Zero;
            try {
                if (currentProcessToken)
                    Check(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | READ_CONTROL, out readOnly), "Open original token diagnostic read handle");
                else
                    // Same object, not a new token. Never copy mutation rights or
                    // close/inherit the launch handle; a denied read is evidence.
                    Check(DuplicateHandle(GetCurrentProcess(), token, GetCurrentProcess(), out readOnly, TOKEN_QUERY | READ_CONTROL, false, 0), "Open private token diagnostic read handle");
                IntPtr owner, group, dacl, sacl, descriptor;
                uint error = GetSecurityInfo(readOnly, 6, OWNER_GROUP_DACL, out owner, out group, out dacl, out sacl, out descriptor); // SE_KERNEL_OBJECT
                if (error != 0) throw new Win32Exception((int)error, "Read token object security");
                try { return Sddl(descriptor); }
                finally { if (descriptor != IntPtr.Zero) LocalFree(descriptor); }
            } finally { if (readOnly != IntPtr.Zero) CloseHandle(readOnly); }
        }
        internal static object PathSecurity(string path) {
            return new Dictionary<string,object> { { "path", path }, { "security", Observe(() => {
                IntPtr owner, group, dacl, sacl, descriptor;
                uint error = GetNamedSecurityInfoW(path, 1, OWNER_GROUP_DACL, out owner, out group, out dacl, out sacl, out descriptor);
                if (error != 0) throw new Win32Exception((int)error, "Read fixed launch path security");
                try { return new Dictionary<string,object> {
                    { "ownerSid", owner == IntPtr.Zero ? null : new SecurityIdentifier(owner).Value }, { "sddl", Sddl(descriptor) }
                }; } finally { if (descriptor != IntPtr.Zero) LocalFree(descriptor); }
            }) } };
        }
        internal static object UserObjectInfo(IntPtr handle, int index) {
            uint size;
            bool initial = GetUserObjectInformationW(handle, index, IntPtr.Zero, 0, out size);
            int error = Marshal.GetLastWin32Error();
            if (initial || error != 122 || size == 0 || size > 65536) throw new Win32Exception(error, "Read user object information size");
            IntPtr buffer = Marshal.AllocHGlobal((int)size);
            try {
                Check(GetUserObjectInformationW(handle, index, buffer, size, out size), "Read current user object information");
                if (index == 2) return Marshal.PtrToStringUni(buffer); // UOI_NAME
                return new Dictionary<string,object> { // USEROBJECTFLAGS: two BOOLs then DWORD flags.
                    { "inherit", Marshal.ReadInt32(buffer) != 0 }, { "reserved", Marshal.ReadInt32(buffer, 4) != 0 },
                    { "flags", unchecked((uint)Marshal.ReadInt32(buffer, 8)) }
                };
            } finally { Marshal.FreeHGlobal(buffer); }
        }
        internal static object UserObjectSecurity(IntPtr handle) {
            uint requested = OWNER_GROUP_DACL, size;
            bool initial = GetUserObjectSecurity(handle, ref requested, IntPtr.Zero, 0, out size);
            int error = Marshal.GetLastWin32Error();
            if (initial || error != 122 || size == 0 || size > 1048576) throw new Win32Exception(error, "Read user object security size");
            IntPtr buffer = Marshal.AllocHGlobal((int)size);
            try {
                Check(GetUserObjectSecurity(handle, ref requested, buffer, size, out size), "Read current user object security");
                return Sddl(buffer);
            } finally { Marshal.FreeHGlobal(buffer); }
        }
        internal static uint SessionId(IntPtr token) {
            IntPtr info = Information(token, TokenSessionId);
            try { return unchecked((uint)Marshal.ReadInt32(info)); }
            finally { Marshal.FreeHGlobal(info); }
        }
        internal static int CurrentProcessSessionId() {
            using (var current = Process.GetCurrentProcess()) return current.SessionId;
        }
        internal static string VerifiedExistingDesktop(IntPtr original, IntPtr token) {
            // Single-variable diagnostic: use only the observed existing desktop.
            // Never create/switch a desktop or grant access. An unexpected runner
            // session/name stops this experiment before process creation.
            IntPtr station = GetProcessWindowStation(), desktop = GetThreadDesktop(GetCurrentThreadId());
            Check(station != IntPtr.Zero && desktop != IntPtr.Zero, "Read existing diagnostic desktop handles");
            if (!String.Equals((string)UserObjectInfo(station, 2), "WinSta0", StringComparison.Ordinal)
                || !String.Equals((string)UserObjectInfo(desktop, 2), "Default", StringComparison.Ordinal))
                throw new InvalidOperationException("Diagnostic runner desktop differs from observed WinSta0/Default");
            int currentSession = CurrentProcessSessionId();
            if (currentSession < 0 || SessionId(original) != (uint)currentSession || SessionId(token) != (uint)currentSession)
                throw new InvalidOperationException("Diagnostic runner/token sessions differ");
            return @"winsta0\default";
        }
        internal static object CurrentUserObject(bool station) {
            // Borrow current handles. Do not close them, switch desktop/station,
            // enumerate other objects, or retry an access-denied security read.
            IntPtr handle = station ? GetProcessWindowStation() : GetThreadDesktop(GetCurrentThreadId());
            Check(handle != IntPtr.Zero, "Read current station/desktop handle");
            return new Dictionary<string,object> {
                { "name", Observe(() => UserObjectInfo(handle, 2)) },
                { "flags", Observe(() => UserObjectInfo(handle, 1)) },
                { "security", Observe(() => UserObjectSecurity(handle)) }
            };
        }
        internal static object SecLogon(bool configuration) {
            IntPtr manager = IntPtr.Zero, service = IntPtr.Zero, buffer = IntPtr.Zero;
            try {
                manager = OpenSCManagerW(null, null, 0x0001); // SC_MANAGER_CONNECT, local only.
                Check(manager != IntPtr.Zero, "Read local service manager");
                service = OpenServiceW(manager, "seclogon", configuration ? 0x0001u : 0x0004u); // SERVICE_QUERY_CONFIG or SERVICE_QUERY_STATUS only.
                Check(service != IntPtr.Zero, "Open Secondary Logon for read only");
                uint size;
                if (configuration) {
                    bool initial = QueryServiceConfigW(service, IntPtr.Zero, 0, out size);
                    int error = Marshal.GetLastWin32Error();
                    if (initial || error != 122 || size < 12 || size > 8192) throw new Win32Exception(error, "Read Secondary Logon configuration size");
                    buffer = Marshal.AllocHGlobal((int)size);
                    Check(QueryServiceConfigW(service, buffer, size, out size), "Read Secondary Logon configuration");
                    // Only the leading DWORD flags. No account, command, or dependencies.
                    return new Dictionary<string,object> {
                        { "serviceType", unchecked((uint)Marshal.ReadInt32(buffer)) },
                        { "startType", unchecked((uint)Marshal.ReadInt32(buffer, 4)) },
                        { "errorControl", unchecked((uint)Marshal.ReadInt32(buffer, 8)) }
                    };
                }
                size = 36; // SERVICE_STATUS_PROCESS: nine DWORDs.
                buffer = Marshal.AllocHGlobal((int)size);
                Check(QueryServiceStatusEx(service, 0, buffer, size, out size), "Read Secondary Logon status");
                return new Dictionary<string,object> {
                    { "serviceType", unchecked((uint)Marshal.ReadInt32(buffer)) },
                    { "currentState", unchecked((uint)Marshal.ReadInt32(buffer, 4)) },
                    { "controlsAccepted", unchecked((uint)Marshal.ReadInt32(buffer, 8)) },
                    { "win32ExitCode", unchecked((uint)Marshal.ReadInt32(buffer, 12)) },
                    { "serviceSpecificExitCode", unchecked((uint)Marshal.ReadInt32(buffer, 16)) },
                    { "serviceFlags", unchecked((uint)Marshal.ReadInt32(buffer, 32)) }
                };
            } finally {
                if (buffer != IntPtr.Zero) Marshal.FreeHGlobal(buffer);
                if (service != IntPtr.Zero) CloseServiceHandle(service);
                if (manager != IntPtr.Zero) CloseServiceHandle(manager);
            }
        }

        internal static void Check(bool success, string operation) {
            if (!success) {
                // Capture before formatting or any other native call can change it.
                int code = Marshal.GetLastWin32Error();
                throw new Win32Exception(code, operation + " [Win32=" + code + "]: " + new Win32Exception(code).Message);
            }
        }
        internal static IntPtr Information(IntPtr token, int kind) {
            int size;
            bool initial = GetTokenInformation(token, kind, IntPtr.Zero, 0, out size);
            int error = Marshal.GetLastWin32Error();
            if (initial || error != 122 || size <= 0) throw new Win32Exception(error, "GetTokenInformation size");
            IntPtr buffer = Marshal.AllocHGlobal(size);
            try { Check(GetTokenInformation(token, kind, buffer, size, out size), "GetTokenInformation"); return buffer; }
            catch { Marshal.FreeHGlobal(buffer); throw; }
        }
        public static PrivilegeSnapshot[] CurrentPrivileges() {
            IntPtr token;
            Check(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, out token), "Read current token privileges");
            try {
                IntPtr buffer = Information(token, TokenPrivileges);
                try {
                    // TOKEN_PRIVILEGES: DWORD count followed by 12-byte LUID_AND_ATTRIBUTES.
                    int count = Marshal.ReadInt32(buffer);
                    if (count < 0 || count > 1024) throw new InvalidOperationException("Invalid privilege count");
                    var values = new List<PrivilegeSnapshot>();
                    for (int i = 0; i < count; i++) {
                        int offset = 4 + i * 12;
                        var luid = new LUID { LowPart = unchecked((uint)Marshal.ReadInt32(buffer, offset)), HighPart = Marshal.ReadInt32(buffer, offset + 4) };
                        uint attributes = unchecked((uint)Marshal.ReadInt32(buffer, offset + 8));
                        int length = 0;
                        bool initial = LookupPrivilegeNameW(null, ref luid, null, ref length);
                        int error = Marshal.GetLastWin32Error();
                        if (initial || error != 122 || length <= 0) throw new Win32Exception(error, "Read privilege name size [Win32=" + error + "]: " + new Win32Exception(error).Message);
                        var name = new StringBuilder(length + 1); length = name.Capacity;
                        Check(LookupPrivilegeNameW(null, ref luid, name, ref length), "Read privilege name");
                        values.Add(new PrivilegeSnapshot { Name = name.ToString(), Enabled = (attributes & 2) != 0, EnabledByDefault = (attributes & 1) != 0, Removed = (attributes & 4) != 0, UsedForAccess = (attributes & 0x80000000) != 0 });
                    }
                    return values.ToArray();
                } finally { Marshal.FreeHGlobal(buffer); }
            } finally { CloseHandle(token); }
        }
        internal static string Sid(IntPtr token, int kind) {
            IntPtr buffer = Information(token, kind);
            try { return new SecurityIdentifier(Marshal.ReadIntPtr(buffer)).Value; }
            finally { Marshal.FreeHGlobal(buffer); }
        }
        internal static TokenSnapshot Snapshot(IntPtr token) {
            var value = new TokenSnapshot { User = Sid(token, TokenUser), Owner = Sid(token, TokenOwner) };
            IntPtr stats = Information(token, TokenStatistics);
            try { value.TokenId = unchecked((uint)Marshal.ReadInt32(stats, 4)).ToString("x8") + ":" + unchecked((uint)Marshal.ReadInt32(stats)).ToString("x8"); }
            finally { Marshal.FreeHGlobal(stats); }
            IntPtr type = Information(token, TokenType);
            try { value.TokenType = Marshal.ReadInt32(type); }
            finally { Marshal.FreeHGlobal(type); }
            return value;
        }
        public static TokenSnapshot Current() {
            IntPtr token;
            Check(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, out token), "Open current token for read");
            try { return Snapshot(token); } finally { CloseHandle(token); }
        }
        public static void AssertUnchanged(TokenSnapshot before, TokenSnapshot after) {
            if (before.TokenId != after.TokenId || before.User != after.User || before.Owner != after.Owner || before.TokenType != after.TokenType)
                throw new InvalidOperationException("Original process token changed");
        }
        public static void AssertPrivate(TokenSnapshot original, TokenSnapshot child) {
            if (child.TokenType != 1 || child.TokenId == original.TokenId || child.User != original.User || child.Owner != child.User)
                throw new InvalidOperationException("Private primary token identity/owner check failed");
        }
        public static void CreateOwnerProbe(string path) {
            // Native creation with no explicit security descriptor, matching Go Mkdir.
            // Existing paths are rejected; there is deliberately no Set-Acl/owner repair.
            Check(CreateDirectoryW(path, IntPtr.Zero), "Create fresh default-owner probe");
        }
        public static string ReadDirectoryOwner(string path) {
            IntPtr owner, group, dacl, sacl, descriptor;
            uint error = GetNamedSecurityInfoW(path, 1, 1, out owner, out group, out dacl, out sacl, out descriptor);
            if (error != 0) throw new Win32Exception((int)error, "Read actual probe owner");
            try { return new SecurityIdentifier(owner).Value; }
            finally { if (descriptor != IntPtr.Zero) LocalFree(descriptor); }
        }
        internal static string Quote(string value) {
            // CommandLineToArgvW-compatible quoting; no shell or expression evaluation.
            var result = new StringBuilder("\""); int slashes = 0;
            foreach (char c in value) {
                if (c == '\\') { slashes++; continue; }
                if (c == '"') { result.Append('\\', slashes * 2 + 1); result.Append(c); slashes = 0; continue; }
                result.Append('\\', slashes); slashes = 0; result.Append(c);
            }
            result.Append('\\', slashes * 2); return result.Append('"').ToString();
        }
        internal static IntPtr EnvironmentBlock() {
            var entries = new SortedDictionary<string,string>(StringComparer.OrdinalIgnoreCase);
            foreach (DictionaryEntry entry in Environment.GetEnvironmentVariables()) entries[(string)entry.Key] = (string)entry.Value;
            var block = new StringBuilder();
            foreach (var entry in entries) block.Append(entry.Key).Append('=').Append(entry.Value).Append('\0');
            block.Append('\0');
            return Marshal.StringToHGlobalUni(block.ToString());
        }
    }

    public sealed class PrivateToken : IDisposable {
        private IntPtr original, duplicate;
        public TokenSnapshot Original { get; private set; }
        public TokenSnapshot DuplicateBefore { get; private set; }
        public TokenSnapshot Normalized { get; private set; }
        public PrivateToken() {
            try {
                // Crucially, this handle cannot change the caller's token owner.
                Native.Check(Native.OpenProcessToken(Native.GetCurrentProcess(), Native.TOKEN_QUERY | Native.TOKEN_DUPLICATE, out original), "Open original token read/duplicate only");
                Original = Native.Snapshot(original);
                Native.Check(Native.DuplicateTokenEx(original, Native.TOKEN_QUERY | Native.TOKEN_DUPLICATE | Native.TOKEN_ASSIGN_PRIMARY | Native.TOKEN_ADJUST_DEFAULT, IntPtr.Zero, 2, 1, out duplicate), "Duplicate private primary token");
                DuplicateBefore = Native.Snapshot(duplicate);
                if (DuplicateBefore.TokenId == Original.TokenId || DuplicateBefore.User != Original.User || DuplicateBefore.Owner != Original.Owner || DuplicateBefore.TokenType != 1)
                    throw new InvalidOperationException("Token duplication did not isolate the original identity");
            } catch { Dispose(); throw; }
        }
        public void Normalize() {
            if (duplicate == IntPtr.Zero) throw new ObjectDisposedException("PrivateToken");
            IntPtr user = Native.Information(duplicate, Native.TokenUser);
            IntPtr owner = Marshal.AllocHGlobal(IntPtr.Size);
            try {
                Marshal.WriteIntPtr(owner, Marshal.ReadIntPtr(user));
                Native.Check(Native.SetTokenInformation(duplicate, Native.TokenOwner, owner, IntPtr.Size), "Set private TokenOwner to TokenUser");
            } finally { Marshal.FreeHGlobal(owner); Marshal.FreeHGlobal(user); }
            Normalized = Native.Snapshot(duplicate);
            Native.AssertPrivate(Original, Normalized);
            Native.AssertUnchanged(Original, Native.Current());
        }
        public object ReadLaunchDiagnostics(string executable, string worker, string config, string directory) {
            if (duplicate == IntPtr.Zero) throw new ObjectDisposedException("PrivateToken");
            if (Normalized == null) throw new InvalidOperationException("Owner normalization must precede diagnostics");
            // These observations cannot establish which object denied process creation,
            // or substitute for the actual child-token / owner / target result.
            return new Dictionary<string,object> {
                { "schemaVersion", 1 }, { "phase", "beforeCreateProcessWithTokenW" },
                { "currentProcessSessionId", Native.Observe(() => Native.CurrentProcessSessionId()) },
                { "originalToken", Native.TokenContext(original) },
                { "privateToken", Native.TokenContext(duplicate) },
                { "originalTokenObjectSecurity", Native.Observe(() => Native.TokenObjectSecurity(original, true)) },
                { "privateTokenObjectSecurity", Native.Observe(() => Native.TokenObjectSecurity(duplicate, false)) },
                { "executable", Native.PathSecurity(executable) },
                { "currentDirectory", Native.PathSecurity(directory) },
                { "worker", Native.PathSecurity(worker) },
                { "configuration", Native.PathSecurity(config) },
                { "currentWindowStation", Native.Observe(() => Native.CurrentUserObject(true)) },
                { "currentDesktop", Native.Observe(() => Native.CurrentUserObject(false)) },
                { "secondaryLogonStatus", Native.Observe(() => Native.SecLogon(false)) },
                { "secondaryLogonConfiguration", Native.Observe(() => Native.SecLogon(true)) }
            };
        }
        public ChildProcess Start(string executable, string worker, string config, string directory) {
            if (duplicate == IntPtr.Zero) throw new ObjectDisposedException("PrivateToken");
            if (Normalized == null) throw new InvalidOperationException("Owner normalization must precede child creation");
            return new ChildProcess(duplicate, Original, executable, worker, config, directory, Native.VerifiedExistingDesktop(original, duplicate));
        }
        public void Dispose() {
            if (duplicate != IntPtr.Zero) { Native.CloseHandle(duplicate); duplicate = IntPtr.Zero; }
            if (original != IntPtr.Zero) { Native.CloseHandle(original); original = IntPtr.Zero; }
        }
    }

    public sealed class ChildProcess : IDisposable {
        private IntPtr process, thread, job;
        private bool assigned, resumed;
        public uint ProcessId { get; private set; }
        public TokenSnapshot Token { get; private set; }
        internal ChildProcess(IntPtr token, TokenSnapshot original, string executable, string worker, string config, string directory, string verifiedDesktop) {
            IntPtr environment = IntPtr.Zero;
            try {
                job = Native.CreateJobObjectW(IntPtr.Zero, null);
                Native.Check(job != IntPtr.Zero, "Create private acceptance job");
                var limits = new Native.JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
                limits.BasicLimitInformation.LimitFlags = 0x00002000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                Native.Check(Native.SetInformationJobObject(job, 9, ref limits, (uint)Marshal.SizeOf(limits)), "Configure kill-on-close job");
                var startup = new Native.STARTUPINFO(); startup.cb = Marshal.SizeOf(startup);
                // Explicit existing desktop avoids the API's NULL-desktop permission adjustment.
                startup.lpDesktop = verifiedDesktop;
                var command = new StringBuilder(Native.Quote(executable) + " -NoLogo -NoProfile -NonInteractive -File " + Native.Quote(worker) + " -ConfigurationPath " + Native.Quote(config));
                if (command.Length >= 1024) throw new InvalidOperationException("Acceptance child command exceeds CreateProcessWithTokenW limit");
                environment = Native.EnvironmentBlock();
                Native.PROCESS_INFORMATION child;
                // Requires the runner's existing SeImpersonatePrivilege. No privilege enabling,
                // account changes, credential fallback, scheduled task, or impersonation occurs.
                Native.Check(Native.CreateProcessWithTokenW(token, 0, executable, command, Native.CREATE_SUSPENDED | Native.CREATE_UNICODE_ENVIRONMENT, environment, directory, ref startup, out child), "Create acceptance child with private token (existing SeImpersonatePrivilege required)");
                process = child.hProcess; thread = child.hThread; ProcessId = child.dwProcessId;
                Native.Check(Native.AssignProcessToJobObject(job, process), "Assign suspended acceptance child to private job"); assigned = true;
                IntPtr childToken;
                Native.Check(Native.OpenProcessToken(process, Native.TOKEN_QUERY, out childToken), "Read suspended child's actual token");
                try { Token = Native.Snapshot(childToken); } finally { Native.CloseHandle(childToken); }
                Native.AssertPrivate(original, Token);
                Native.AssertUnchanged(original, Native.Current());
                // The caller persists this actual-child evidence before Resume.
            } catch (Exception launchError) {
                try { Dispose(); }
                catch (Exception cleanupError) { throw new AggregateException("Acceptance launch and constructor cleanup both failed", launchError, cleanupError); }
                throw;
            }
            finally { if (environment != IntPtr.Zero) Marshal.FreeHGlobal(environment); }
        }
        public void Resume() {
            if (process == IntPtr.Zero) throw new ObjectDisposedException("ChildProcess");
            if (resumed) throw new InvalidOperationException("Acceptance child already resumed");
            uint previous = Native.ResumeThread(thread);
            if (previous == 0xffffffff) throw new Win32Exception(Marshal.GetLastWin32Error(), "Resume validated acceptance child");
            if (previous != 1) throw new InvalidOperationException("Unexpected acceptance child suspend count");
            resumed = true;
        }
        public int Wait(uint timeoutMilliseconds) {
            if (process == IntPtr.Zero) throw new ObjectDisposedException("ChildProcess");
            if (!resumed) throw new InvalidOperationException("Acceptance child has not passed its launch checks");
            uint wait = Native.WaitForSingleObject(process, timeoutMilliseconds);
            if (wait == Native.WAIT_TIMEOUT) throw new TimeoutException("Acceptance child timed out; private job will be terminated");
            if (wait != Native.WAIT_OBJECT_0) throw new Win32Exception(Marshal.GetLastWin32Error(), "Wait for acceptance child");
            uint code; Native.Check(Native.GetExitCodeProcess(process, out code), "Read acceptance child exit code");
            return unchecked((int)code);
        }
        public void Dispose() {
            try {
                if (process != IntPtr.Zero) {
                    // Also kills any remaining descendants after a normal child exit.
                    if (assigned && job != IntPtr.Zero) Native.Check(Native.TerminateJobObject(job, 1), "Terminate private acceptance process tree");
                    else if (Native.WaitForSingleObject(process, 0) != Native.WAIT_OBJECT_0)
                        Native.Check(Native.TerminateProcess(process, 1), "Terminate unassigned suspended acceptance child");
                    if (Native.WaitForSingleObject(process, 30000) != Native.WAIT_OBJECT_0)
                        throw new InvalidOperationException("Acceptance child termination unconfirmed");
                    if (assigned && job != IntPtr.Zero) {
                        var clock = Stopwatch.StartNew();
                        while (true) {
                            Native.JOBOBJECT_BASIC_ACCOUNTING_INFORMATION accounting;
                            Native.Check(Native.QueryInformationJobObject(job, 1, out accounting, (uint)Marshal.SizeOf(typeof(Native.JOBOBJECT_BASIC_ACCOUNTING_INFORMATION)), IntPtr.Zero), "Confirm acceptance job process tree is empty");
                            if (accounting.ActiveProcesses == 0) break;
                            if (clock.ElapsedMilliseconds >= 30000) throw new InvalidOperationException("Acceptance descendant termination unconfirmed");
                            Thread.Sleep(20);
                        }
                    }
                }
            } finally {
                if (job != IntPtr.Zero) { Native.CloseHandle(job); job = IntPtr.Zero; }
                if (thread != IntPtr.Zero) { Native.CloseHandle(thread); thread = IntPtr.Zero; }
                if (process != IntPtr.Zero) { Native.CloseHandle(process); process = IntPtr.Zero; }
            }
        }
    }
}
