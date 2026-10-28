// This executes only the finite mock producer suite, never Go or native cases.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

test('R fixture producer executes its finite pure rejection and cleanup witnesses', () => {
  const result = spawnSync('python3', ['-B', 'tools/pc-acceptance/build-r-native-fixture.test.py', '-v'], {
    cwd: root,
    env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1', POLARIS_NO_KERNEL_RUN: '1' },
    encoding: 'utf8',
    timeout: 60_000,
    maxBuffer: 1024 * 1024,
  });
  const output = `${result.stdout ?? ''}\n${result.stderr ?? ''}`;
  const diagnostics = `${result.error ?? ''}\n${output}`;
  assert.ifError(result.error);
  assert.equal(result.signal, null, diagnostics);
  assert.equal(result.status, 0, diagnostics);
  const summaries = [...output.matchAll(/^Ran (\d+) tests in [\d.]+s$/gm)];
  assert.equal(summaries.length, 1, diagnostics);
  const count = Number(summaries[0][1]);
  assert.ok(count >= 25, `Only ${count} tests ran; expected at least 25`);
  assert.equal([...output.matchAll(/^test_\S+ \([^\r\n]+\) \.\.\. ok$/gm)].length, count, diagnostics);
  assert.match(output, /^OK$/m, diagnostics);
});
