// Source-only predicates: these suites use bytes and mocked producers, never Apple tools or Apps.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { classifyImpact } from './classify-ci-impact.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

function runSuite(script, args, summary, minimum, optimized = false) {
  const result = spawnSync('python3', [...(optimized ? ['-O', '-B'] : ['-B']), script, ...args], {
    cwd: root,
    env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1', POLARIS_NO_KERNEL_RUN: '1' },
    encoding: 'utf8',
    timeout: 60_000,
    maxBuffer: 16 * 1024 * 1024,
  });
  const diagnostics = `${result.error ?? ''}\n${result.stdout ?? ''}\n${result.stderr ?? ''}`;
  assert.equal(result.status, 0, diagnostics);
  const matched = summary.exec(result.stdout);
  assert.ok(matched, `Missing executed-suite summary: ${diagnostics}`);
  assert.ok(Number(matched[1]) >= minimum, `Only ${matched[1]} cases ran; expected at least ${minimum}`);
}

test('Apple source admission keeps its existing pure predicates in the host CI gate', () => {
  runSuite('scripts/ios-libbox-verify.test.py', ['--preflight-only'],
    /^(\d+) source-admission\/producer\/structure unit cases passed; no Framework or App built\.$/m, 477);
  const impact = classifyImpact([
    'scripts/ios-libbox.py', 'scripts/ios-libbox-verify.test.py',
    'scripts/apple-carrier.py', 'scripts/apple-carrier.test.py',
    'scripts/apple-source-inputs.py', 'scripts/apple-source-inputs.test.py',
    'scripts/apple-source-unit.test.mjs',
  ]);
  assert.equal(impact.kernel, false);
  assert.equal(impact.android, false);
  assert.equal(impact.hasPackage, false);
  assert.deepEqual(impact.platforms, []);
  assert.deepEqual(impact.unregisteredScopes, []);
});

test('Apple C2 source observations exercise finite pure tool, graph and header rejection gates', () => {
  runSuite('scripts/apple-source-inputs.test.py', [],
    /^(\d+) Apple source input unit cases passed; no compiler, Framework or App built\.$/m, 186);
});

test('Apple C2 source predicates retain every pure counterexample with Python optimization', () => {
  runSuite('scripts/apple-source-inputs.test.py', [],
    /^(\d+) Apple source input unit cases passed; no compiler, Framework or App built\.$/m, 186, true);
});

test('Apple carrier predicates execute nonempty pure format and provenance counterexamples', () => {
  runSuite('scripts/apple-carrier.test.py', [],
    /^(\d+) Apple carrier unit cases passed; no Framework or App built\.$/m, 170);
});
