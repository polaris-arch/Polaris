import assert from 'node:assert/strict';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { linkSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('./acceptance.mjs', import.meta.url));
const APP = 'a'.repeat(40), KERNEL = 'b'.repeat(40), ATTEMPT = 'c'.repeat(32);
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
const run = (...args) => spawnSync(process.execPath, [script, ...args], { encoding: 'utf8' });

// One passed build-and-test attempt with its manifests, binaries and receipt.
function evidence(t) {
  const dir = mkdtempSync(join(tmpdir(), 'windows-directory-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const write = (name, value) => {
    writeFileSync(join(dir, name), typeof value === 'string' ? value : `${JSON.stringify(value)}\n`);
    return join(dir, name);
  };
  const binary = (name) => ({ path: write(name, `binary ${name}`), sha256: sha256(`binary ${name}`) });
  const kernel = { kernelSource: KERNEL, validationMode: 'formal', repository: 'polaris-arch/sing-box-polaris' };
  write('app-artifact.json', { ...binary('app.exe'), appSource: APP, validationMode: 'formal' });
  write('kernel-artifact.json', { ...binary('tailscale-tests.exe'), ...kernel });
  write('activation-artifact.json', { ...binary('directory-activation-tests.exe'), ...kernel, tests: [
    'TestWindowsDirectorySourceContract', 'TestWindowsDirectoryReservedNamespaceWholeConfig',
    'TestWindowsDirectoryActivationAndOriginalBinding'] });
  const receipt = write('validation-receipt.json', { attemptId: ATTEMPT, validationMode: 'formal', kernelSource: KERNEL,
    appSource: APP, repository: 'polaris-arch/sing-box-polaris', result: 'passed', scope: 'directory-protocol-tests-only' });
  write('validation-attempt.json', { attemptId: ATTEMPT, result: 'passed', receipt: 'validation-receipt.json',
    receiptSHA256: sha256(readFileSync(receipt)) });
  return dir;
}
const verify = (dir) => run('verify', '--evidence-directory', dir, '--expected-app-source', APP,
  '--expected-kernel-source', KERNEL, '--validation-mode', 'formal', '--previous-attempt-id=');

test('verify accepts the current passed attempt and reports its bound identities', (t) => {
  const dir = evidence(t);
  const result = verify(dir);
  assert.equal(result.status, 0, result.stderr);
  const verified = JSON.parse(result.stdout);
  assert.equal(verified.attemptId, ATTEMPT);
  assert.equal(verified.receiptSHA256, sha256(readFileSync(join(dir, 'validation-receipt.json'))));
  assert.equal(verified.activationManifestSHA256, sha256(readFileSync(join(dir, 'activation-artifact.json'))));
  assert.equal(verified.systemNonce, null);
});

test('verify rejects a binary changed after its manifest was written', (t) => {
  const dir = evidence(t);
  writeFileSync(join(dir, 'app.exe'), 'replaced');
  const result = verify(dir);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Current binary hash mismatch: app-artifact.json/);
});

test('verify rejects a hard-linked activation test binary', (t) => {
  const dir = evidence(t);
  linkSync(join(dir, 'directory-activation-tests.exe'), join(dir, 'second-name.exe'));
  const result = verify(dir);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Invalid activation test binary/);
});

test('read-artifact returns the verified manifest for the SYSTEM bootstrap', (t) => {
  const dir = evidence(t);
  const result = run('read-artifact', '--manifest', join(dir, 'kernel-artifact.json'), '--source-field', 'kernelSource',
    '--expected-source', KERNEL, '--validation-mode', 'formal');
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), JSON.parse(readFileSync(join(dir, 'kernel-artifact.json'), 'utf8')));
});
