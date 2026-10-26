// Source-contract/negative-mutation tests only. These do not execute Windows,
// compile the product, or establish a native token/probe acceptance result.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), 'windows-directory');
const sources = {
  workflow: readFileSync(join(root, '..', '..', '.github', 'workflows', 'ci.yml'), 'utf8'),
  parent: readFileSync(join(root, 'Run-WithUserTokenOwner.ps1'), 'utf8'),
  child: readFileSync(join(root, 'Run-UserTokenOwnerChild.ps1'), 'utf8'),
  native: readFileSync(join(root, 'UserTokenOwnerNative.cs'), 'utf8'),
};
function ordered(text, ...parts) {
  let cursor = -1;
  for (const part of parts) {
    const next = text.indexOf(part, cursor + 1);
    assert.ok(next > cursor, `Missing/out-of-order gate: ${part}`);
    cursor = next;
  }
}
function audit({ parent, child, native, workflow }) {
  assert.match(workflow, /\.\/scripts\/windows-directory\/Run-WithUserTokenOwner\.ps1 @args\r?\n[ \t]*if \(\$LASTEXITCODE -ne 0\) \{ throw 'Private TokenOwner acceptance failed' \}/);
  assert.doesNotMatch(workflow, /\.\/scripts\/windows-directory\/Build-And-Test\.ps1 @args/);
  // Absence/failure of every API must stop execution; no fallback to a shared token.
  assert.match(native, /OpenProcessToken\(Native.GetCurrentProcess\(\), Native.TOKEN_QUERY \| Native.TOKEN_DUPLICATE, out original\)/);
  assert.equal([...native.matchAll(/SetTokenInformation\(duplicate, Native.TokenOwner, owner, IntPtr.Size\)/g)].length, 1);
  assert.doesNotMatch(native, /SetTokenInformation\(original|AdjustTokenPrivileges\(|SetThreadToken\(|ImpersonateLoggedOnUser\(|CreateProcessAsUser\(|CreateProcessWithLogon/);
  ordered(native, 'DuplicateBefore = Native.Snapshot(duplicate)', 'DuplicateBefore.TokenId == Original.TokenId', 'public void Normalize()', 'Native.SetTokenInformation(duplicate', 'Normalized = Native.Snapshot(duplicate)', 'Native.AssertPrivate(Original, Normalized)');
  assert.match(native, /if \(duplicate == IntPtr.Zero\) throw new ObjectDisposedException\("PrivateToken"\)/);
  assert.match(native, /if \(Normalized == null\) throw new InvalidOperationException\("Owner normalization must precede child creation"\)/);
  ordered(native, 'if (!success) {', 'int code = Marshal.GetLastWin32Error();', 'throw new Win32Exception(code, operation + " [Win32=" + code');
  assert.match(native, /new Win32Exception\(code\)\.Message/);
  assert.match(native, /OpenProcessToken\(GetCurrentProcess\(\), TOKEN_QUERY, out token\), "Read current token privileges"/);
  assert.match(native, /Information\(token, TokenPrivileges\)/);
  assert.match(native, /Check\(LookupPrivilegeNameW\(null, ref luid, name, ref length\), "Read privilege name"\)/);
  assert.match(native, /Enabled = \(attributes & 2\) != 0/);
  assert.match(native, /new AggregateException\("Acceptance launch and constructor cleanup both failed", launchError, cleanupError\)/);
  assert.match(native, /child.TokenType != 1 \|\| child.TokenId == original.TokenId \|\| child.User != original.User \|\| child.Owner != child.User/);
  assert.match(native, /before.TokenId != after.TokenId \|\| before.User != after.User \|\| before.Owner != after.Owner/);
  ordered(native, 'Native.CreateProcessAsUserW(', 'Native.CREATE_SUSPENDED | Native.CREATE_UNICODE_ENVIRONMENT', 'Native.AssignProcessToJobObject(job, process)', 'Native.OpenProcessToken(process, Native.TOKEN_QUERY', 'Token = Native.Snapshot(childToken)', 'Native.AssertPrivate(original, Token)');
  assert.match(native, /catch \{ Dispose\(\); throw; \}/);
  assert.match(native, /LimitFlags = 0x00002000/);
  assert.match(native, /Native.TerminateJobObject\(job, 1\)/);
  assert.match(native, /Native.TerminateProcess\(process, 1\)/);
  assert.match(native, /Native.WaitForSingleObject\(process, 30000\)/);
  assert.match(native, /accounting.ActiveProcesses == 0/);
  assert.match(native, /clock.ElapsedMilliseconds >= 30000\) throw/);
  assert.match(native, /wait == Native.WAIT_TIMEOUT\) throw new TimeoutException/);
  assert.match(native, /return unchecked\(\(int\)code\)/);
  assert.match(native, /if \(process == IntPtr.Zero\) throw new ObjectDisposedException\("ChildProcess"\)/);
  assert.match(native, /Check\(CreateDirectoryW\(path, IntPtr.Zero\),/);
  assert.match(native, /GetNamedSecurityInfoW\(path, 1, 1,/);
  assert.match(native, /if \(error != 0\) throw new Win32Exception\(\(int\)error, "Read actual probe owner"\)/);
  assert.doesNotMatch(native + child, /SetNamedSecurityInfo|Set-Acl\s+-|icacls|takeown|AdjustTokenPrivileges/);

  ordered(parent, "Invalidate-ValidationReceipt 'start'", 'Add-Type -Path $native', '$lease.Normalize()', '$state.duplicateNormalized', '$child = $lease.Start', '$state.suspendedChildToken', '$configuration.actualChildToken', 'Save-OwnerAttempt $state', '$child.Resume()', '$exitCode = $child.Wait(3600000)');
  assert.match(parent, /if \(\$exitCode -ne 0\) \{[\s\S]*?throw "Acceptance child failed/);
  ordered(parent, '$child.Dispose()', '$state.originalAfter =', '::AssertUnchanged($original,$state.originalAfter)', "Invalidate-ValidationReceipt 'failed'", 'Save-OwnerAttempt $state');
  // Constructor failure leaves child null: only a returned handle whose Dispose
  // succeeds can establish cleanup, even if native constructor cleanup throws.
  assert.match(parent, /cleanupConfirmed=\$false/);
  assert.match(parent, /try \{ if \(\$null -ne \$child\) \{ \$child\.Dispose\(\); \$state\.cleanupConfirmed = \$true \} \}/);
  assert.equal([...parent.matchAll(/\$state\.cleanupConfirmed = \$true/g)].length, 1);
  assert.match(parent, /catch \{ Invalidate-ValidationReceipt 'evidence-failed'; throw \}/);
  assert.equal([...parent.matchAll(/\$child\.Resume\(\)/g)].length, 1);
  assert.match(parent, /exit \$exitCode/);
  assert.match(parent, /\$item\.nativeErrorCode = \$exception\.NativeErrorCode/);
  assert.match(parent, /\$item\.systemMessage = \[System.ComponentModel.Win32Exception\]::new\(\$exception\.NativeErrorCode\)\.Message/);
  assert.match(parent, /\$exception.InnerExceptions/);
  assert.match(parent, /Convert-ExceptionEvidence \$exception.InnerException/);
  assert.match(parent, /\$state.failureEvidence = Convert-ExceptionEvidence \$_\.Exception/);
  assert.match(parent, /\$state.cleanupFailureEvidence = Convert-ExceptionEvidence \$_\.Exception/);
  ordered(parent, '$state.originalPrivileges = [Polaris.WindowsAcceptance.Native]::CurrentPrivileges()', 'Save-OwnerAttempt $state', '$lease =');
  assert.match(parent, /childEvidence.result -cne 'targetPassed'/);
  assert.match(parent, /-not \$childEvidence.probeVerified -or -not \$childEvidence.probeRemoved -or -not \$childEvidence.targetStarted/);
  assert.doesNotMatch(parent, /\[string\]\$ScriptPath|\$ScriptParameters|\[switch\]\$Skip|\[switch\]\$Bypass/);

  ordered(child, '::Current()', '::AssertPrivate(', '::AssertUnchanged(', '$PID -ne $config.childProcessId', '::CreateOwnerProbe($probe)', '::ReadDirectoryOwner($probe)', "$state.probeOwner -cne $current.User", "$state.probeVerified = $true", '[IO.Directory]::Delete($probe,$false)', '$state.probeRemoved = $true', '$state.targetStarted = $true', '& $target @parameters');
  assert.match(child, /if \(\$state.probeOwner -cne \$current.User\) \{ throw/);
  assert.equal([...child.matchAll(/& \$target @parameters/g)].length, 1);
  assert.match(child, /\$target = Join-Path \$PSScriptRoot 'Build-And-Test.ps1'/);
  assert.match(child, /if \(-not \(\$formal -xor \$candidate\)\) \{ throw/);
  assert.match(child, /\$LASTEXITCODE -ne 0\) \{ \$exitCode = \$LASTEXITCODE \}/);
  assert.match(child, /exit \$exitCode/);
  assert.match(child, /catch \{ \$state.result = 'failed'; \$state.cleanupError = "\$_"; \$exitCode = 1 \}/);
}

test('fixed acceptance fixture is fail closed with independent actual-token and object-owner gates', () => audit(sources));
const negatives = [
  ['Win32 code discarded', 'native', 'int code = Marshal.GetLastWin32Error();', 'int code = 0;'],
  ['system error text discarded', 'native', 'new Win32Exception(code).Message', 'operation'],
  ['privilege query requests mutation access', 'native', 'GetCurrentProcess(), TOKEN_QUERY, out token', 'GetCurrentProcess(), TOKEN_QUERY | TOKEN_ADJUST_DEFAULT, out token'],
  ['privilege enabled state guessed', 'native', 'Enabled = (attributes & 2) != 0', 'Enabled = true'],
  ['constructor cleanup masks launch error', 'native', 'new AggregateException("Acceptance launch and constructor cleanup both failed", launchError, cleanupError)', 'cleanupError'],
  ['HRESULT mislabeled as Win32 code', 'parent', '$item.nativeErrorCode = $exception.NativeErrorCode', '$item.nativeErrorCode = $exception.HResult'],
  ['wrapped native exception lost', 'parent', 'Convert-ExceptionEvidence $exception.InnerException', 'Convert-ExceptionEvidence $exception'],
  ['original failure evidence omitted', 'parent', '$state.failureEvidence = Convert-ExceptionEvidence $_.Exception', '# omitted'],
  ['cleanup failure overwrites original evidence', 'parent', '$state.cleanupFailureEvidence = Convert-ExceptionEvidence $_.Exception', '$state.failureEvidence = Convert-ExceptionEvidence $_.Exception'],

  ['constructor cleanup failure falsely confirmed with null child', 'parent', 'try { if ($null -ne $child) { $child.Dispose(); $state.cleanupConfirmed = $true } }', 'try { if ($null -ne $child) { $child.Dispose() }; $state.cleanupConfirmed = $true }'],
  ['cleanup success assumed before launch', 'parent', 'cleanupConfirmed=$false', 'cleanupConfirmed=$true'],
  ['CI continues after failed owner gate', 'workflow', "if ($LASTEXITCODE -ne 0) { throw 'Private TokenOwner acceptance failed' }", '# missing immediate gate'],
  ['CI bypasses private owner entrypoint', 'workflow', './scripts/windows-directory/Run-WithUserTokenOwner.ps1 @args', './scripts/windows-directory/Build-And-Test.ps1 @args'],
  ['original token mutation right', 'native', 'Native.TOKEN_QUERY | Native.TOKEN_DUPLICATE, out original', 'Native.TOKEN_QUERY | Native.TOKEN_DUPLICATE | Native.TOKEN_ADJUST_DEFAULT, out original'],
  ['setter aimed at shared original token', 'native', 'SetTokenInformation(duplicate, Native.TokenOwner', 'SetTokenInformation(original, Native.TokenOwner'],
  ['duplicate owner API failure ignored', 'native', 'if (!success) {', 'if (false) {'],
  ['closed token accepted', 'native', 'if (duplicate == IntPtr.Zero) throw new ObjectDisposedException("PrivateToken")', 'if (false) throw new ObjectDisposedException("PrivateToken")', true],
  ['no normalized-token precondition', 'native', 'if (Normalized == null) throw new InvalidOperationException("Owner normalization must precede child creation")', 'if (false) throw new InvalidOperationException("Owner normalization must precede child creation")'],
  ['same token identity accepted', 'native', 'child.TokenId == original.TokenId', 'false'],
  ['wrong actual-child owner accepted', 'native', 'child.Owner != child.User', 'false'],
  ['original owner readback unchecked', 'native', 'before.Owner != after.Owner', 'false'],
  ['child starts before inspection', 'native', 'Native.CREATE_SUSPENDED | Native.CREATE_UNICODE_ENVIRONMENT', 'Native.CREATE_UNICODE_ENVIRONMENT'],
  ['actual-child token not read', 'native', 'Token = Native.Snapshot(childToken)', 'Token = original'],
  ['job kill-on-close omitted', 'native', 'LimitFlags = 0x00002000', 'LimitFlags = 0'],
  ['unassigned suspended child survives', 'native', 'Native.TerminateProcess(process, 1)', 'true'],
  ['descendant cleanup not verified', 'native', 'accounting.ActiveProcesses == 0', 'true'],
  ['timeout silently accepted', 'native', 'wait == Native.WAIT_TIMEOUT) throw new TimeoutException', 'false) throw new TimeoutException'],
  ['probe creation failure ignored', 'native', 'Check(CreateDirectoryW(path, IntPtr.Zero),', 'Check(true,'],
  ['probe owner read failure ignored', 'native', 'if (error != 0) throw new Win32Exception((int)error, "Read actual probe owner")', 'if (false) throw new Win32Exception((int)error, "Read actual probe owner")'],
  ['old successful receipt retained', 'parent', "Invalidate-ValidationReceipt 'start'", '# omitted'],
  ['late failure keeps new successful receipt', 'parent', "Invalidate-ValidationReceipt 'failed'", '# omitted'],
  ['final evidence write failure keeps receipt', 'parent', "Invalidate-ValidationReceipt 'evidence-failed'", '# omitted'],
  ['child resumed before evidence', 'parent', '$state.suspendedChildToken', '$child.Resume(); $state.suspendedChildToken'],
  ['nonzero target exit masked', 'parent', 'if ($exitCode -ne 0)', 'if ($false)'],
  ['probe owner mismatch permitted', 'child', 'if ($state.probeOwner -cne $current.User) { throw', 'if ($false) { throw'],
  ['target runs before probe', 'child', '    [Polaris.WindowsAcceptance.Native]::CreateOwnerProbe($probe)', '    & $target @parameters\n    [Polaris.WindowsAcceptance.Native]::CreateOwnerProbe($probe)'],
  ['probe cleanup failure masked', 'child', "catch { $state.result = 'failed'; $state.cleanupError = \"$_\"; $exitCode = 1 }", 'catch {}'],
  ['closed child accepted', 'native', 'if (process == IntPtr.Zero) throw new ObjectDisposedException("ChildProcess")', 'if (false) throw new ObjectDisposedException("ChildProcess")', true],
];
for (const [name, file, before, after, all] of negatives) {
  test(`negative source mutation: ${name}`, () => {
    assert.ok(sources[file].includes(before), `Mutation anchor missing: ${name}`);
    const mutated = { ...sources, [file]: all ? sources[file].replaceAll(before, after) : sources[file].replace(before, after) };
    assert.throws(() => audit(mutated), { name: 'AssertionError' });
  });
}

function auditReadOnlyDiagnostics({ native, parent }) {
  assert.match(native, /READ_CONTROL = 0x00020000/);
  assert.match(native, /OWNER_GROUP_DACL = 0x00000007/);
  assert.match(native, /TokenDefaultDacl = 6, TokenSessionId = 12, TokenIntegrityLevel = 25/);
  const tokenSecurity = native.split('internal static object TokenObjectSecurity(')[1]?.split('internal static object PathSecurity(')[0];
  assert.ok(tokenSecurity);
  assert.match(tokenSecurity, /OpenProcessToken\(GetCurrentProcess\(\), TOKEN_QUERY \| READ_CONTROL, out readOnly\)/);
  assert.match(tokenSecurity, /DuplicateHandle\(GetCurrentProcess\(\), token, GetCurrentProcess\(\), out readOnly, TOKEN_QUERY \| READ_CONTROL, false, 0\)/);
  assert.match(tokenSecurity, /GetSecurityInfo\(readOnly, 6, OWNER_GROUP_DACL,/);
  assert.match(tokenSecurity, /if \(error != 0\) throw new Win32Exception/);
  assert.match(tokenSecurity, /finally \{ if \(readOnly != IntPtr.Zero\) CloseHandle\(readOnly\); \}/);
  assert.doesNotMatch(tokenSecurity, /DuplicateTokenEx|TOKEN_ADJUST|TOKEN_ASSIGN|MAXIMUM_ALLOWED/);
  const observe = native.split('internal static object Observe(')[1]?.split('internal static string Sddl(')[0];
  assert.ok(observe);
  assert.match(observe, /catch \(Exception error\)/);
  assert.match(observe, /"result", "unavailable"/);
  assert.match(observe, /failure\["nativeErrorCode"\] = native.NativeErrorCode/);
  assert.match(observe, /return failure;/);
  assert.doesNotMatch(observe, /throw|error.Message|error.ToString/);
  const helpers = native.split('internal static object Observe(')[1]?.split('internal static void Check(')[0];
  assert.ok(helpers);
  assert.doesNotMatch(helpers, /Environment\.|SetTokenInformation\(|AdjustToken\w*\(|CreateProcess\w*\(|SetNamedSecurityInfo|SetSecurityInfo\(|SetUserObjectSecurity\(|SetKernelObjectSecurity\(/);
  assert.equal([...native.matchAll(/\bSetTokenInformation\(/g)].length, 2); // One declaration and the existing private-owner call.
  assert.match(native, /Information\(token, TokenDefaultDacl\)/);
  assert.match(native, /if \(acl == IntPtr.Zero\) return new Dictionary<string,object> \{ \{ "isNull", true \}, \{ "sddl", null \} \}/);
  assert.match(native, /descriptor.GetSddlForm\(AccessControlSections.Access\)/);
  assert.match(native, /Information\(token, TokenSessionId\)/);
  assert.match(native, /Information\(token, TokenIntegrityLevel\)/);
  assert.match(native, /GetNamedSecurityInfoW\(path, 1, OWNER_GROUP_DACL,/);
  const diagnostics = native.split('public object ReadLaunchDiagnostics(')[1]?.split('public ChildProcess Start(')[0];
  assert.ok(diagnostics);
  assert.match(diagnostics, /"phase", "beforeCreateProcessAsUserW"/);
  for (const token of ['original', 'duplicate']) assert.ok(diagnostics.includes(`Native.TokenContext(${token})`));
  assert.match(diagnostics, /Native.Observe\(\(\) => Native.TokenObjectSecurity\(original, true\)\)/);
  assert.match(diagnostics, /Native.Observe\(\(\) => Native.TokenObjectSecurity\(duplicate, false\)\)/);
  for (const path of ['executable', 'directory', 'worker', 'config']) assert.ok(diagnostics.includes(`Native.PathSecurity(${path})`));
  assert.equal([...diagnostics.matchAll(/Native.PathSecurity\(/g)].length, 4);
  for (const station of ['true', 'false']) assert.ok(diagnostics.includes(`Native.Observe(() => Native.CurrentUserObject(${station}))`));
  for (const config of ['true', 'false']) assert.ok(diagnostics.includes(`Native.Observe(() => Native.SecLogon(${config}))`));
  assert.doesNotMatch(diagnostics, /Environment|CreateProcess\w*\(|\.Start\(|SetTokenInformation|Normalize\(/);
  // The phase string contains the API name, but there is still exactly one actual call.
  assert.equal([...native.matchAll(/Native.CreateProcessAsUserW\(/g)].length, 1);
  const userObject = native.split('internal static object CurrentUserObject(')[1]?.split('internal static object SecLogon(')[0];
  assert.match(userObject, /GetProcessWindowStation\(\) : GetThreadDesktop\(GetCurrentThreadId\(\)\)/);
  assert.doesNotMatch(userObject, /CloseHandle\(|CloseDesktop\(|CloseWindowStation\(/);
  assert.match(native, /uint requested = OWNER_GROUP_DACL, size/);
  assert.match(native, /GetUserObjectSecurity\(handle, ref requested, buffer, size, out size\)/);
  assert.match(native, /OpenSCManagerW\(null, null, 0x0001\)/);
  assert.match(native, /OpenServiceW\(manager, "seclogon", configuration \? 0x0001u : 0x0004u\)/);
  assert.match(native, /QueryServiceStatusEx\(service, 0, buffer, size, out size\)/);
  assert.match(native, /QueryServiceConfigW\(service, buffer, size, out size\)/);
  assert.doesNotMatch(native + parent, /StartService\w*\(|ChangeServiceConfig\w*\(|ControlService\w*\(|SetUserObjectSecurity\(|SetSecurityInfo\(|SetKernelObjectSecurity\(|SetThreadDesktop\(|SetProcessWindowStation\(/);
  assert.equal([...native.matchAll(/lpDesktop/g)].length, 2);
  assert.match(native, /startup\.lpDesktop = verifiedDesktop;/);
  ordered(parent, '$lease.Normalize()', '$configuration | ConvertTo-Json', '$workingDirectory = (Get-Location).Path',
    '$state.launchDiagnostics = $lease.ReadLaunchDiagnostics($executable,$worker,$configurationPath,$workingDirectory)', 'Save-OwnerAttempt $state',
    'catch { $state.launchDiagnosticsFailureEvidence = Convert-ExceptionEvidence $_.Exception }',
    '$child = $lease.Start($executable,$worker,$configurationPath,$workingDirectory)');
  assert.match(parent, /try \{\s*\$state.launchDiagnostics = \$lease.ReadLaunchDiagnostics\([^\n]*\)\s*Save-OwnerAttempt \$state\s*\}\s*catch \{ \$state.launchDiagnosticsFailureEvidence = Convert-ExceptionEvidence \$_\.Exception \}/);
  assert.equal([...parent.matchAll(/\$lease.ReadLaunchDiagnostics\(/g)].length, 1);
  assert.equal([...parent.matchAll(/\$lease.Start\(/g)].length, 1);
}
test('launch diagnostics are isolated read-only observations of fixed objects', () => auditReadOnlyDiagnostics(sources));
for (const [name, file, before, after] of [
  ['original SD handle lacks READ_CONTROL', 'native', 'TOKEN_QUERY | READ_CONTROL, out readOnly', 'TOKEN_QUERY, out readOnly'],
  ['original SD handle gains mutation rights', 'native', 'TOKEN_QUERY | READ_CONTROL, out readOnly', 'TOKEN_QUERY | READ_CONTROL | TOKEN_ADJUST_DEFAULT, out readOnly'],
  ['private SD is read with wrong token handle', 'native', 'GetSecurityInfo(readOnly, 6, OWNER_GROUP_DACL', 'GetSecurityInfo(token, 6, OWNER_GROUP_DACL'],
  ['private diagnostic copies launch mutation rights', 'native', 'TOKEN_QUERY | READ_CONTROL, false, 0)', 'TOKEN_QUERY | READ_CONTROL, false, 2)'],
  ['private diagnostic closes launch handle', 'native', 'TOKEN_QUERY | READ_CONTROL, false, 0)', 'TOKEN_QUERY | READ_CONTROL, false, 1)'],
  ['token object diagnostic handle leaks', 'native', 'if (readOnly != IntPtr.Zero) CloseHandle(readOnly)', 'if (false) CloseHandle(readOnly)'],
  ['SACL read introduced', 'native', 'OWNER_GROUP_DACL = 0x00000007', 'OWNER_GROUP_DACL = 0x0000000f'],
  ['NULL default ACL confused with empty ACL', 'native', '{ "isNull", true }, { "sddl", null }', '{ "isNull", false }, { "sddl", "D:" }'],
  ['diagnostic Win32 error lost', 'native', 'failure["nativeErrorCode"] = native.NativeErrorCode', 'failure["nativeErrorCode"] = native.HResult'],
  ['diagnostic failure reported as successful read', 'native', '"result", "unavailable"', '"result", "read"'],
  ['diagnostic failure is thrown', 'native', 'return failure;', 'throw error;'],
  ['helper leaks environment values', 'native', 'internal static object TokenContext(IntPtr token) {', 'internal static object TokenContext(IntPtr token) { Environment.GetEnvironmentVariables();'],
  ['helper mutates token default ACL', 'native', 'internal static object TokenContext(IntPtr token) {', 'internal static object TokenContext(IntPtr token) { SetTokenInformation(token, TokenDefaultDacl, IntPtr.Zero, 0);'],
  ['config ACL omitted', 'native', 'Native.PathSecurity(config)', 'null'],
  ['service mutation access introduced', 'native', 'configuration ? 0x0001u : 0x0004u', 'configuration ? 0x0002u : 0x0010u'],
  ['service configuration status combined', 'native', 'Native.Observe(() => Native.SecLogon(true))', 'Native.SecLogon(true)'],
  ['borrowed desktop closed', 'native', 'Check(handle != IntPtr.Zero, "Read current station/desktop handle");', 'Check(handle != IntPtr.Zero, "Read current station/desktop handle"); CloseHandle(handle);'],
  ['diagnostic exception overwrites launch evidence', 'parent', '$state.launchDiagnosticsFailureEvidence = Convert-ExceptionEvidence $_.Exception', '$state.failureEvidence = Convert-ExceptionEvidence $_.Exception'],
  ['diagnostic exception skips real launch', 'parent', 'catch { $state.launchDiagnosticsFailureEvidence = Convert-ExceptionEvidence $_.Exception }', 'catch { throw }'],
  ['diagnostic persistence failure skips real launch', 'parent', 'Save-OwnerAttempt $state\n    }\n    catch { $state.launchDiagnosticsFailureEvidence = Convert-ExceptionEvidence $_.Exception }', '}\n    catch { $state.launchDiagnosticsFailureEvidence = Convert-ExceptionEvidence $_.Exception }\n    Save-OwnerAttempt $state'],
  ['launch uses different working directory than evidence', 'parent', '$lease.Start($executable,$worker,$configurationPath,$workingDirectory)', '$lease.Start($executable,$worker,$configurationPath,$PSScriptRoot)'],
]) test(`read-only diagnostics reject ${name}`, () => {
  assert.ok(sources[file].includes(before), name);
  assert.throws(() => auditReadOnlyDiagnostics({ ...sources, [file]: sources[file].replace(before, after) }), { name: 'AssertionError' });
});

const diagnosticStages = [
  ['Select explicit formal or candidate Windows directory source', null],
  ['Checkout exact Windows directory producer source', 'windows_directory_source'],
  ['Resolve exact checked-out kernel toolchain', 'windows_directory_checkout'],
  ['Setup exact kernel Go toolchain', 'windows_directory_go'],
  ['Build and run native Windows directory producer and consumer acceptance', 'windows_directory_go_setup'],
];
function checkDiagnosticScheduling(workflow) {
  assert.match(workflow, /- name: Build\n\s+id: workspace_build\n\s+run: cargo build --workspace --verbose/);
  assert.match(workflow, /- name: Test\n\s+id: workspace_tests/);
  assert.match(workflow, /run: cargo test --workspace --no-fail-fast/);
  const testBlock = workflow.split('      - name: Test\n')[1].split('\n      - ')[0];
  assert.doesNotMatch(testBlock, /continue-on-error/);
  assert.deepEqual([...testBlock.matchAll(/^        if: (.+)$/gm)].map((m) => m[1]),
    ["needs.cheap_format.outputs.app_native_compile_only != 'true'"]);
  for (const [name, dependency] of diagnosticStages) {
    const block = workflow.split(`      - name: ${name}\n`)[1]?.split('      - name:')[0];
    assert.ok(block, name);
    const expression = block.match(/if: >-\s*\$\{\{([\s\S]*?)\}\}/)?.[1];
    assert.ok(expression, name);
    assert.doesNotMatch(block, /continue-on-error/);
    // Evaluate the actual checked-in condition over the complete status/event
    // matrix. This is a workflow-expression contract check, not a native run.
    const evaluate = new Function('runner', 'github', 'inputs', 'steps', 'needs', 'success', 'cancelled', `return (${expression});`);
    for (const os of ['Windows', 'Linux', 'macOS'])
      for (const event of ['workflow_dispatch', 'pull_request', 'push'])
        for (const mode of ['candidate', 'formal'])
          for (const admittedCandidate of ['', 'a'.repeat(40)])
            for (const compileOnly of ['', 'true', 'false', 'unknown'])
            for (const succeeded of [false, true])
              for (const wasCancelled of [false, true])
                for (const build of ['success', 'failure', 'skipped'])
                  for (const tests of ['success', 'failure', 'skipped'])
                    for (const preceding of ['success', 'failure', 'skipped']) {
                      if (succeeded && wasCancelled) continue;
                      const steps = { workspace_build: { outcome: build }, workspace_tests: { outcome: tests } };
                      if (dependency) steps[dependency] = { outcome: preceding };
                      const expected = compileOnly !== 'true' && os === 'Windows' && (!dependency || preceding === 'success')
                        && (succeeded || (!wasCancelled && event === 'workflow_dispatch' && (mode === 'candidate' || admittedCandidate !== '')
                          && build === 'success' && tests === 'failure'));
                      assert.equal(evaluate({ os }, { event_name: event }, { windows_directory_mode: mode }, steps,
                        { cheap_format: { outputs: { native_dns_candidate: admittedCandidate, app_native_compile_only: compileOnly } } },
                        () => succeeded, () => wasCancelled), expected, `${name}: ${[os,event,mode,admittedCandidate,succeeded,wasCancelled,build,tests,preceding]}`);
                    }
  }
  for (const name of ['Approved batch SYSTEM producer — fresh', 'Approved batch SYSTEM producer — prebuilt-user']) {
    // Approved PRs and the exact opt-in final candidate share the existing SYSTEM route.
    const block = workflow.split(`      - name: ${name}\n`)[1];
    assert.ok(block, name);
    const condition = block.split('        shell:')[0];
    assert.match(condition, /github.event_name == 'pull_request'/);
    assert.doesNotMatch(condition, /!cancelled\(\)|failure\(\)/);
    assert.match(condition, /inputs\.windows_system_candidate == github\.sha/);
  }
}
test('manual candidate diagnostics survive only Test failure without reducing PR gates', () => checkDiagnosticScheduling(sources.workflow));
test('workspace Test scope stops at the actual next step even when its name changes', () => {
  const boundary = '      - name: Run exact controlled App lifecycle hot-switch and stats tests';
  assert.ok(sources.workflow.includes(boundary));
  checkDiagnosticScheduling(sources.workflow.replace(boundary,
    '      - name: Unrelated following conditional step\n        if: false\n        run: echo fixture\n' + boundary));
  assert.throws(() => checkDiagnosticScheduling(sources.workflow.replace('id: workspace_tests',
    'id: workspace_tests\n        if: success()')), { name: 'AssertionError' });
});
for (const [name, before, after] of [
  ['PR failure bypass', "github.event_name == 'workflow_dispatch' && (inputs.windows_directory_mode == 'candidate'", "github.event_name == 'pull_request' && (inputs.windows_directory_mode == 'candidate'"],
  ['cancelled run bypass', '!cancelled()', 'true'],
  ['build failure bypass', "steps.workspace_build.outcome == 'success'", 'true'],
  ['formal validation bypass', "inputs.windows_directory_mode == 'candidate'", 'true'],
  ['admitted candidate route removed', "needs.cheap_format.outputs.native_dns_candidate != ''", 'false'],
  ['unadmitted candidate bypass', "needs.cheap_format.outputs.native_dns_candidate != ''", 'true'],
  ['failed checkout bypass', "&& steps.windows_directory_checkout.outcome == 'success'", ''],
  ['failed Go setup bypass', "&& steps.windows_directory_go_setup.outcome == 'success'", ''],
  ['test failure masked', 'id: workspace_tests', 'id: workspace_tests\n        continue-on-error: true'],
]) test(`diagnostic scheduling rejects ${name}`, () => {
  assert.ok(sources.workflow.includes(before));
  assert.throws(() => checkDiagnosticScheduling(sources.workflow.replaceAll(before, after)));
});

// This is a bounded hypothesis test of the documented NULL-desktop branch,
// not proof that desktop permissions caused the observed access denial.
function auditExplicitDesktop(native) {
  const preflight = native.split('internal static string VerifiedExistingDesktop(')[1]?.split('internal static object CurrentUserObject(')[0];
  assert.ok(preflight);
  assert.match(native, /"currentProcessSessionId", Native\.Observe\(\(\) => Native\.CurrentProcessSessionId\(\)\)/);
  ordered(preflight, 'GetProcessWindowStation()', 'GetThreadDesktop(GetCurrentThreadId())',
    'Check(station != IntPtr.Zero && desktop != IntPtr.Zero',
    'if (!String.Equals((string)UserObjectInfo(station, 2), "WinSta0", StringComparison.Ordinal)',
    '|| !String.Equals((string)UserObjectInfo(desktop, 2), "Default", StringComparison.Ordinal))',
    'throw new InvalidOperationException("Diagnostic runner desktop',
    'int currentSession = CurrentProcessSessionId();',
    'if (currentSession < 0 || SessionId(original) != (uint)currentSession || SessionId(token) != (uint)currentSession)',
    'throw new InvalidOperationException("Diagnostic runner/token session',
    'return @"winsta0\\default";');
  assert.doesNotMatch(preflight, /catch|Observe\(|Set\w+\(|CloseHandle\(/);
  assert.match(native, /return new ChildProcess\(duplicate, Original, executable, worker, config, directory, Native\.VerifiedExistingDesktop\(original, duplicate\), LaunchPrivileges\);/);
  assert.match(native, /internal ChildProcess\(IntPtr token, TokenSnapshot original, string executable, string worker, string config, string directory, string verifiedDesktop, LaunchPrivilegeEvidence privileges\)/);
  ordered(native, 'startup.cb = Marshal.SizeOf(startup)', 'startup.lpDesktop = verifiedDesktop;', 'Native.CreateProcessAsUserW(');
  assert.equal([...native.matchAll(/startup\.lpDesktop\s*=/g)].length, 1);
}
test('explicit desktop requires observed names and all three session identities before creation', () => auditExplicitDesktop(sources.native));
for (const [name, before, after] of [
  ['wrong station', '"WinSta0", StringComparison.Ordinal', '"Other", StringComparison.Ordinal'],
  ['wrong desktop', '"Default", StringComparison.Ordinal', '"Other", StringComparison.Ordinal'],
  ['missing handle validation', 'Check(station != IntPtr.Zero && desktop != IntPtr.Zero', 'Check(true'],
  ['missing process session', 'int currentSession = CurrentProcessSessionId();', 'int currentSession = 2;'],
  ['missing original session', 'SessionId(original) != (uint)currentSession || ', ''],
  ['missing private session', ' || SessionId(token) != (uint)currentSession', ''],
  ['preflight bypass', 'Native.VerifiedExistingDesktop(original, duplicate)', 'null'],
  ['NULL desktop', 'startup.lpDesktop = verifiedDesktop;', 'startup.lpDesktop = null;'],
  ['caller-selected desktop', 'return @"winsta0\\default";', 'return Environment.GetEnvironmentVariable("DESKTOP");'],
  ['name failure swallowed', 'IntPtr station = GetProcessWindowStation()', 'try { } catch { } IntPtr station = GetProcessWindowStation()'],
]) test(`explicit desktop rejects ${name}`, () => {
  assert.ok(sources.native.includes(before), name);
  assert.throws(() => auditExplicitDesktop(sources.native.replace(before, after)), { name: 'AssertionError' });
});

function auditAsUser({ native, parent }) {
  assert.doesNotMatch(native, /CreateProcessWithToken|CreateProcessWithLogon|CreateRestrictedToken|AdjustTokenPrivileges|LsaAddAccountRights/);
  assert.equal([...native.matchAll(/Native.CreateProcessAsUserW\(/g)].length, 1);
  assert.match(native, /CreateProcessAsUserW\(IntPtr token, string application, StringBuilder command, IntPtr processAttributes, IntPtr threadAttributes, bool inheritHandles, uint creationFlags,/);
  assert.match(native, /Native.CreateProcessAsUserW\(token, executable, command, IntPtr.Zero, IntPtr.Zero, false, Native.CREATE_SUSPENDED \| Native.CREATE_UNICODE_ENVIRONMENT, environment, directory, ref startup, out child\)/);
  ordered(native, 'privileges.Before = Native.CurrentPrivileges();', 'bool created = Native.CreateProcessAsUserW(',
    'int createError = Marshal.GetLastWin32Error();',
    'if (created) { process = child.hProcess; thread = child.hThread; ProcessId = child.dwProcessId; }',
    'Exception createFailure = created ? null : new Win32Exception(createError,',
    'privileges.After = Native.CurrentPrivileges();',
    'Native.AssertPrivilegesRestored(privileges.Before, privileges.After);', 'privileges.Restored = true;',
    'if (createFailure != null) throw new AggregateException(', 'if (createFailure != null) throw createFailure;',
    'Native.AssignProcessToJobObject(job, process)');
  assert.match(native, /new AggregateException\("Acceptance creation and caller privilege readback both failed", createFailure, privilegeError\)/);
  const compare = native.split('internal static void AssertPrivilegesRestored(')[1]?.split('internal static string Sid(')[0];
  assert.match(compare, /expected.Add\(value.Luid, value\)/);
  assert.match(compare, /before.Length != after.Length/);
  assert.match(compare, /!expected.TryGetValue\(value.Luid, out prior\) \|\| prior.Name != value.Name/);
  assert.match(compare, /expected.Remove\(value.Luid\)/);
  assert.match(compare, /expected.Count != 0/);
  assert.match(compare, /\(prior.Attributes & ~0x80000000u\) != \(value.Attributes & ~0x80000000u\)/);
  assert.match(native, /Luid = unchecked\(\(uint\)luid.HighPart\).ToString\("x8"\) \+ ":" \+ luid.LowPart.ToString\("x8"\)/);
  assert.doesNotMatch(compare, /catch|AdjustToken|SetToken/);
  assert.match(native, /Attributes = attributes,/);
  assert.match(parent, /finally \{\s*if \(\$null -ne \$lease\) \{ \$state.launchPrivileges = \$lease.LaunchPrivileges \}/);
  assert.ok(parent.indexOf('$state.launchPrivileges = $lease.LaunchPrivileges') < parent.indexOf('$lease.Dispose()'));
}
test('AsUser preserves suspended ownership and verifies exact caller privilege restoration on either result', () => auditAsUser(sources));
for (const [name, file, before, after] of [
  ['handle inheritance', 'native', 'command, IntPtr.Zero, IntPtr.Zero, false, Native.CREATE_SUSPENDED', 'command, IntPtr.Zero, IntPtr.Zero, true, Native.CREATE_SUSPENDED'],
  ['no suspension', 'native', 'false, Native.CREATE_SUSPENDED | Native.CREATE_UNICODE_ENVIRONMENT', 'false, Native.CREATE_UNICODE_ENVIRONMENT'],
  ['fallback API', 'native', 'if (createFailure != null) throw createFailure;', 'if (createFailure != null) CreateProcessWithTokenW();'],
  ['restricted-token substitution', 'native', 'privileges.Before = Native.CurrentPrivileges();', 'CreateRestrictedToken(); privileges.Before = Native.CurrentPrivileges();'],
  ['self-enabled privilege', 'native', 'privileges.Before = Native.CurrentPrivileges();', 'AdjustTokenPrivileges(); privileges.Before = Native.CurrentPrivileges();'],
  ['lost error before readback', 'native', 'int createError = Marshal.GetLastWin32Error();', 'int createError = 0;'],
  ['unowned successful process on readback failure', 'native', 'if (created) { process = child.hProcess; thread = child.hThread; ProcessId = child.dwProcessId; }', 'if (false) { process = child.hProcess; thread = child.hThread; ProcessId = child.dwProcessId; }'],
  ['after snapshot omitted', 'native', 'privileges.After = Native.CurrentPrivileges();', 'privileges.After = privileges.Before;'],
  ['raw attributes discarded', 'native', 'Attributes = attributes,', 'Attributes = attributes & 2,'],
  ['changed attributes accepted', 'native', '(prior.Attributes & ~0x80000000u) != (value.Attributes & ~0x80000000u)', 'false'],
  ['changed privilege name accepted', 'native', 'prior.Name != value.Name', 'false'],
  ['LUID identity replaced by name', 'native', 'expected.Add(value.Luid, value)', 'expected.Add(value.Name, value)'],
  ['unknown permission bits ignored', 'native', '(prior.Attributes & ~0x80000000u) != (value.Attributes & ~0x80000000u)', '(prior.Attributes & 7u) != (value.Attributes & 7u)'],
  ['changed count accepted', 'native', 'before.Length != after.Length', 'false'],
  ['duplicate names accepted', 'native', 'expected.Remove(value.Luid);', ''],
  ['readback check bypass', 'native', 'Native.AssertPrivilegesRestored(privileges.Before, privileges.After);', ''],
  ['API failure swallowed', 'native', 'if (createFailure != null) throw createFailure;', ''],
  ['combined failures lose original code', 'native', 'createFailure, privilegeError);', 'privilegeError);'],
  ['failed launch loses privilege evidence', 'parent', '$state.launchPrivileges = $lease.LaunchPrivileges', '$state.launchPrivileges = $null'],
]) test(`AsUser rejects ${name}`, () => {
  assert.ok(sources[file].includes(before), name);
  assert.throws(() => auditAsUser({ ...sources, [file]: sources[file].replace(before, after) }), { name: 'AssertionError' });
});

// Evaluate only the actual extracted mask expression; this is not C# execution.
test('privilege restoration mask ignores usage-only evidence and preserves every permission/unknown bit', () => {
  const match = sources.native.match(/\(prior.Attributes & (~0x[0-9a-f]+u)\) != \(value.Attributes & \1\)/i);
  assert.ok(match);
  const mask = ~Number(match[1].slice(1, -1));
  const equivalent = (a, b) => (a & mask) === (b & mask);
  assert.equal(equivalent(3, 0x80000003), true);
  for (const bit of [1, 2, 4, 8, 0x40000000]) assert.equal(equivalent(0, bit), false);
});
