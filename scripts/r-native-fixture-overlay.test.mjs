// Source/patch predicates only; this runner does not execute Go or native cases.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

test('R fixed FD overlay executes its nonempty pure patch and source counterexamples', () => {
  const result = spawnSync('python3', ['-B', 'tools/pc-acceptance/test-r-native-fixture-overlay.py', '-v'], {
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
  assert.equal(summaries.length, 1, `Missing or repeated executed-suite summary: ${diagnostics}`);
  const count = Number(summaries[0][1]);
  assert.ok(count >= 7, `Only ${count} tests ran; expected at least 7`);
  assert.equal([...output.matchAll(/^test_\S+ \([^\r\n]+\) \.\.\. ok$/gm)].length, count,
    `Executed test count differs from successful non-skipped cases: ${diagnostics}`);
  assert.match(output, /^OK$/m, diagnostics);
});
