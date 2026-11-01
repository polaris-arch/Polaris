import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import test from 'node:test';
const root = fileURLToPath(new URL('../', import.meta.url));

test('complete Windows carrier fixtures: actual ZIP and inert NSIS registration', () => {
  const result = spawnSync(process.platform === 'win32' ? 'python' : 'python3', ['-B', '-X', 'utf8', 'scripts/windows-package-integrity.test.py', '-v'],
    { cwd: root, encoding: 'utf8', timeout: 60000, maxBuffer: 8 * 1024 * 1024 });
  process.stdout.write(result.stdout ?? ''); process.stderr.write(result.stderr ?? '');
  assert.ifError(result.error); assert.equal(result.signal, null); assert.equal(result.status, 0);
});

test('ordinary Windows publishing consumes the complete static gate after one build', () => {
  const source = readFileSync(new URL('../.github/workflows/package.yml', import.meta.url), 'utf8');
  const freeze = source.indexOf('python scripts/windows-package-integrity.py prepare');
  const build = source.indexOf('      - name: Build installers');
  const produce = source.indexOf('python scripts/windows-package-integrity.py produce');
  const verify = source.indexOf('python scripts/windows-package-integrity.py assemble');
  const assets = source.indexOf('run: node scripts/verify-packaging.mjs assets --label windows --dir dist-win');
  assert.ok(freeze >= 0 && freeze < build && build < produce && produce < verify && verify < assets);
  const step = source.slice(source.lastIndexOf('      - name:', verify), source.indexOf('      - name:', verify));
  assert.match(step, /if: runner\.os == 'Windows'/);
  assert.doesNotMatch(step, /native_validation|continue-on-error/);
  assert.match(step, /if \(\$LASTEXITCODE -ne 0\).*throw/);
  assert.doesNotMatch(source, /Select-Object -First 1[\s\S]{0,250}dist-win|Compress-Archive -Path "\$staging\/\*"/);
  assert.match(source, /Verify actual installed NSIS and final portable payloads[\s\S]*?if: inputs\.native_validation == true/);
  // Run the actual mandatory confs consumer block with a controlled helper
  // response. Python tests separately audit source/workflow and actual ZIP bytes;
  // this closes their routing into the existing full confs entry, not a new gate.
  const verifier = readFileSync(new URL('./verify-packaging.mjs', import.meta.url), 'utf8');
  const start = verifier.indexOf('  // ── ⑥ Windows publication configuration.');
  const end = verifier.indexOf('  // 与 conf 那条 note', start);
  assert.ok(start >= 0 && end > start);
  const check = Function('workflow', 'execFileSync', 'join', 'ROOT', 'LICENSE_BASENAMES', 'fail', verifier.slice(start, end));
  const valid = { schema: 'polaris-windows-package-config-v1', legalFiles: ['LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md'],
    platform: 'win', resources: ['core-manifest.json'], formats: ['nsis', 'zip'] };
  const invoke = response => {
    const failures = []; let calls = 0;
    check(source, (_exe, args) => {
      calls++;
      assert.deepEqual(args, [join(root, 'scripts/windows-package-integrity.py'), 'contract']);
      if (response instanceof Error) throw response;
      return JSON.stringify(response);
    }, join, root, valid.legalFiles, message => failures.push(message));
    assert.equal(calls, 1); return failures;
  };
  assert.deepEqual(invoke(valid), []);
  for (const name of valid.legalFiles) assert.equal(invoke({ ...valid, legalFiles: valid.legalFiles.filter(item => item !== name) }).length, 1);
  for (const response of [new Error('actual helper contract failed'), { ...valid, platform: 'foreign' },
    { ...valid, formats: ['nsis'] }, { ...valid, resources: null }]) assert.equal(invoke(response).length, 1);

});

test('mobile version diagnostics consume the validated formal contract in both normal Android jobs', () => {
  const source = readFileSync(new URL('../.github/workflows/android.yml', import.meta.url), 'utf8');
  const steps = [...source.matchAll(/      - name: Resolve bundled core version\n([\s\S]*?)(?=      - name:)/g)];
  assert.equal(steps.length, 2);
  for (const [, step] of steps) {
    assert.match(step, /from fork_input import load; print\(load\(\)\['version'\]\)/);
    assert.doesNotMatch(step, /bundledCoreVersion|alpha\.12\.6/);
    assert.match(step, /hot_consumer_candidate/);
    assert.match(step, /version=\$v/);
  }
});
