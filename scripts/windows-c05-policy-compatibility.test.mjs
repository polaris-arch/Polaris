// Real 5.1 policy boundary, separate from native transaction receipts. No policy
// override, inline script loading, certificate installation, Unblock or PS7.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdtempSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, win32 } from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
function host() {
  assert.ok(process.env.POLARIS_C05_NATIVE_LOG, 'fresh native system-directory observation required');
  const log = readFileSync(process.env.POLARIS_C05_NATIVE_LOG, 'utf8');
  const rows = [...log.matchAll(/^POLARIS_C05_SYSTEM_PS=(.+)$/gm)];
  assert.equal(rows.length, 1, 'one native GetSystemDirectoryW observation');
  const executable = JSON.parse(rows[0][1]);
  assert.ok(win32.isAbsolute(executable));
  assert.ok(executable.endsWith('\\WindowsPowerShell\\v1.0\\powershell.exe'));
  return executable;
}
function child(executable, args, extra = {}) {
  const env = { ...process.env, ...extra };
  delete env.PSModulePath;
  delete env.PSExecutionPolicyPreference;
  const output = spawnSync(executable, ['-NoProfile', '-NonInteractive', ...args], { encoding: 'utf8', env, timeout: 15000 });
  assert.ifError(output.error);
  return output;
}
function restricted(executable) {
  // This inline command only reads policy. It never dot-sources/loads a script.
  const query = child(executable, ['-Command', 'Get-ExecutionPolicy']);
  assert.equal(query.status, 0, query.stderr);
  assert.equal(query.stdout.trim(), 'Restricted', 'PREREQUISITE_PENDING: a naturally Restricted 5.1 identity is required; do not change policy to satisfy this test');
}
test('natural Restricted rejects an actual -File before payload entry', { skip: process.platform !== 'win32' }, () => {
  const executable = host(); restricted(executable);
  const dir = mkdtempSync(join(tmpdir(), 'polaris-c05-policy-'));
  try {
    const script = join(dir, 'inert.ps1'), marker = join(dir, 'entered');
    writeFileSync(script, '\ufeff[IO.File]::WriteAllText($env:POLARIS_C05_ENTRY, "entered")\r\n', 'utf8');
    const output = child(executable, ['-File', script], { POLARIS_C05_ENTRY: marker });
    assert.notEqual(output.status, 0);
    assert.match(output.stderr,/PSSecurityException|UnauthorizedAccess/,'actual policy denial required');
    assert.equal(existsSync(marker), false, 'policy refusal cannot be relabeled fixture execution');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('natural Restricted also rejects an already valid signed inert -File', { skip: process.platform !== 'win32' }, () => {
  const executable = host(); restricted(executable);
  const path = process.env.POLARIS_C05_SIGNED_INERT_FIXTURE;
  const expected = process.env.POLARIS_C05_SIGNED_INERT_SHA256;
  assert.ok(path && win32.isAbsolute(path) && /^[a-f0-9]{64}$/.test(expected ?? ''),
    'PREREQUISITE_PENDING: exact already-signed inert fixture/hash required; never install a certificate or sign dynamically');
  assert.equal(createHash('sha256').update(readFileSync(path)).digest('hex'), expected);
  const signature = child(executable, ['-Command', '(Get-AuthenticodeSignature -LiteralPath $env:POLARIS_C05_SIGNED_INERT_FIXTURE).Status']);
  assert.equal(signature.status, 0, signature.stderr); assert.equal(signature.stdout.trim(), 'Valid');
  const output = child(executable, ['-File', path]);
  assert.notEqual(output.status, 0, 'a valid signature does not unlock Restricted');
  assert.match(output.stderr,/PSSecurityException|UnauthorizedAccess/,'signed fixture must be denied by policy before entry');
});
