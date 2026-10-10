// Static contracts and synthetic OS/path fixtures only. No real kernel/loader.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
const root = join(dirname(fileURLToPath(import.meta.url)), '..');
test('native/final payload contracts and no-local-execution rejection', () => {
  execFileSync(process.platform === 'win32' ? 'python' : 'python3', ['-B', 'scripts/native-payload-proof.test.py'], {
    cwd: root, env: { ...process.env, POLARIS_NO_KERNEL_RUN: '1' }, stdio: 'pipe', timeout: 30000 });
});
test('validation-only native and actual final-payload steps preserve production release policy', () => {
  const risk = readFileSync(join(root, '.github/workflows/release-risk.yml'), 'utf8');
  const pkg = readFileSync(join(root, '.github/workflows/package.yml'), 'utf8');
  const producer = readFileSync(join(root, '.github/workflows/desktop-core.yml'), 'utf8');
  assert.match(risk, /options: \[ordinary, native-payload\]/);
  assert.match(risk, /native_payload:[\s\S]*?run_kernel_gates: true[\s\S]*?light_build: false/);
  assert.match(risk, /NATIVE_VALIDATION_RESULT.*needs\.native_payload\.result/);
  assert.match(producer, /Retrieve reviewed unchanged native origins/);
  assert.match(producer, /--assemble --validation-origin/);
  assert.match(pkg, /Verify actual installed NSIS and final portable payloads/);
  assert.match(pkg, /--kind nsis[\s\S]*?--kind zip/);
  assert.match(pkg, /--kind deb[\s\S]*?--kind AppImage/);
  assert.match(pkg, /--test core_build_matrix/);
  assert.match(pkg, /startsWith\(github.ref, 'refs\/tags\/v'\)/);
  const final = readFileSync(join(root, 'scripts/final-package-receipt.py'), 'utf8');
  assert.doesNotMatch(final, /subprocess\.(?:run|Popen)\([^\n]*(?:uninstall|runas|cleaner)/i);
});
