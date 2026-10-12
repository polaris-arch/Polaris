import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

test('formal mobile input/output/cache/recovery source controls run without native producers', () => {
  const result = spawnSync('python3', ['-B', '-m', 'unittest', 'discover', '-s', 'scripts/mobile-core', '-p', 'test_*.py'], {
    cwd: root, env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1', POLARIS_NO_KERNEL_RUN: '1' },
    encoding: 'utf8', timeout: 60_000, maxBuffer: 8 * 1024 * 1024,
  });
  assert.equal(result.status, 0, `${result.error ?? ''}\n${result.stdout}\n${result.stderr}`);
  assert.match(result.stderr, /Ran \d+ tests/);
});
