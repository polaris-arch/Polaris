import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

test('formal mobile input/output/cache/recovery source controls run without native producers', () => {
  assert.ok(existsSync(join(root, 'scripts/mobile-core/test_apple_merge_reuse.py')),
    'the exact-tree Apple reuse negative-control suite must not disappear');
  const result = spawnSync('python3', ['-B', '-m', 'unittest', 'discover', '-s', 'scripts/mobile-core', '-p', 'test_*.py'], {
    cwd: root, env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1', POLARIS_NO_KERNEL_RUN: '1' },
    encoding: 'utf8', timeout: 60_000, maxBuffer: 8 * 1024 * 1024,
  });
  assert.equal(result.status, 0, `${result.error ?? ''}\n${result.stdout}\n${result.stderr}`);
  const count = result.stderr.match(/Ran (\d+) tests/);
  assert.ok(count && Number(count[1]) >= 299, 'all existing mobile controls plus the 36 merge-reuse controls must execute');
});
