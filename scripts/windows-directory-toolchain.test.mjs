import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
test('Windows directory compiler selection reuses source parsing and preserves formal producer facts', () => {
  const result = spawnSync(process.platform === 'win32' ? 'python' : 'python3',
    ['-B', 'scripts/windows-directory/test-toolchain.py'], {
      cwd: root, encoding: 'utf8', timeout: 30_000,
      env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1' },
    });
  assert.equal(result.status, 0, `${result.error ?? ''}\n${result.stdout}\n${result.stderr}`);
  assert.match(result.stderr, /Ran 7 tests/);
  const ci = readFileSync(join(root, '.github/workflows/ci.yml'), 'utf8');
  assert.equal(ci.split('python scripts/windows-directory/resolve-toolchain.py').length - 1, 2);
  assert.doesNotMatch(ci, /Expected one exact (?:kernel )?source toolchain pin/);
  const producer = readFileSync(join(root, 'scripts/windows-directory/Build-And-Test.ps1'), 'utf8');
  assert.match(producer, /--mode \$mode --source \$source --actual-go \$actualGo/);
  assert.match(producer, /toolchain-evidence.json/);
  assert.match(ci, /GOTOOLCHAIN: local/);
});
