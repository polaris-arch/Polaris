import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { validateReleaseContract } from './desktop-core/release-consumer.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));

test('native DNS source binding controls are offline and make no runtime acceptance claim', () => {
  const result = spawnSync(process.platform === 'win32' ? 'python' : 'python3',
    ['-B', 'scripts/desktop-core/test-native-dns-source.py', '-v'], {
      cwd: root, encoding: 'utf8', timeout: 30_000, maxBuffer: 1024 * 1024,
      env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1', POLARIS_NO_KERNEL_RUN: '1' },
    });
  assert.equal(result.status, 0, `${result.error ?? ''}\n${result.stdout}\n${result.stderr}`);
  assert.match(result.stderr, /Ran 11 tests/);
});

test('inactive staging source binding cannot enter the formal release consumer', () => {
  const binding = JSON.parse(readFileSync(new URL('./desktop-core/native-dns-source-binding.json', import.meta.url)));
  assert.throws(() => validateReleaseContract(binding));
  assert.equal(binding.active, false);
  assert.equal(binding.AppAccepted, false);
  assert.equal(binding.publicationEligible, false);
  assert.equal(binding.tagObject, null);
});
