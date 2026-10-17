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
  assert.match(native, /if \(Normalized == null\) throw new InvalidOperationException/);
  assert.match(native, /if \(!success\) throw new Win32Exception/);
  assert.match(native, /child.TokenType != 1 \|\| child.TokenId == original.TokenId \|\| child.User != original.User \|\| child.Owner != child.User/);
  assert.match(native, /before.TokenId != after.TokenId \|\| before.User != after.User \|\| before.Owner != after.Owner/);
  ordered(native, 'Native.CreateProcessWithTokenW(', 'Native.CREATE_SUSPENDED | Native.CREATE_UNICODE_ENVIRONMENT', 'Native.AssignProcessToJobObject(job, process)', 'Native.OpenProcessToken(process, Native.TOKEN_QUERY', 'Token = Native.Snapshot(childToken)', 'Native.AssertPrivate(original, Token)');
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
  assert.match(native, /if \(error != 0\) throw new Win32Exception/);
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
  ['constructor cleanup failure falsely confirmed with null child', 'parent', 'try { if ($null -ne $child) { $child.Dispose(); $state.cleanupConfirmed = $true } }', 'try { if ($null -ne $child) { $child.Dispose() }; $state.cleanupConfirmed = $true }'],
  ['cleanup success assumed before launch', 'parent', 'cleanupConfirmed=$false', 'cleanupConfirmed=$true'],
  ['CI continues after failed owner gate', 'workflow', "if ($LASTEXITCODE -ne 0) { throw 'Private TokenOwner acceptance failed' }", '# missing immediate gate'],
  ['CI bypasses private owner entrypoint', 'workflow', './scripts/windows-directory/Run-WithUserTokenOwner.ps1 @args', './scripts/windows-directory/Build-And-Test.ps1 @args'],
  ['original token mutation right', 'native', 'Native.TOKEN_QUERY | Native.TOKEN_DUPLICATE, out original', 'Native.TOKEN_QUERY | Native.TOKEN_DUPLICATE | Native.TOKEN_ADJUST_DEFAULT, out original'],
  ['setter aimed at shared original token', 'native', 'SetTokenInformation(duplicate, Native.TokenOwner', 'SetTokenInformation(original, Native.TokenOwner'],
  ['duplicate owner API failure ignored', 'native', 'if (!success) throw new Win32Exception', 'if (false) throw new Win32Exception'],
  ['closed token accepted', 'native', 'if (duplicate == IntPtr.Zero) throw new ObjectDisposedException("PrivateToken")', 'if (false) throw new ObjectDisposedException("PrivateToken")', true],
  ['no normalized-token precondition', 'native', 'if (Normalized == null) throw', 'if (false) throw'],
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
  ['probe owner read failure ignored', 'native', 'if (error != 0) throw new Win32Exception', 'if (false) throw new Win32Exception'],
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
