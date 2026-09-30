import assert from 'node:assert/strict';
import test from 'node:test';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { ALL_PACKAGE_PLATFORMS, classifyImpact } from './classify-ci-impact.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts/assert-pc-runtime-release.mjs');
const command = 'node scripts/assert-pc-runtime-release.mjs';
const clearanceName = 'Require PC runtime release clearance';
const packageYaml = readFileSync(join(root, '.github/workflows/package.yml'), 'utf8');
const riskYaml = readFileSync(join(root, '.github/workflows/release-risk.yml'), 'utf8');

function job(yaml, name) {
  const lines = yaml.split('\n');
  const start = lines.findIndex((line) => line === `  ${name}:`);
  assert.ok(start >= 0, `missing job: ${name}`);
  const end = lines.findIndex((line, index) => index > start && /^  [a-zA-Z_][\w-]*:$/.test(line));
  return lines.slice(start, end < 0 ? lines.length : end).join('\n');
}

function step(body, name) {
  const lines = body.split('\n');
  const start = lines.findIndex((line) => line === `      - name: ${name}`);
  assert.ok(start >= 0, `missing step: ${name}`);
  const end = lines.findIndex((line, index) => index > start && /^      - (?:name:|uses:)/.test(line));
  return { start, text: lines.slice(start, end < 0 ? lines.length : end).join('\n') };
}

function requireBlockingStep(body) {
  assert.doesNotMatch(body, /^    continue-on-error:/m, 'release job must propagate failures');
  const clearance = step(body, clearanceName);
  assert.doesNotMatch(clearance.text, /^        (?:if|continue-on-error|env):/m);
  assert.match(clearance.text, /^        run: node scripts\/assert-pc-runtime-release\.mjs$/m);
  const checkout = body.split('\n').findIndex((line) => /uses: actions\/checkout@/.test(line));
  assert.ok(checkout >= 0 && checkout < clearance.start, 'clearance must use checked out policy');
  const firstWrite = body.split('\n').findIndex((line) => /^\s+gh release (?:create|upload|edit) /.test(line));
  assert.ok(firstWrite > clearance.start, 'clearance must precede every release write');
}

function requirePackagePolicy(yaml) {
  requireBlockingStep(job(yaml, 'release_desktop'));
  const release = job(yaml, 'release');
  requireBlockingStep(release);
  const promote = step(release, 'Promote release to public').text;
  assert.doesNotMatch(promote, /^        (?:if|continue-on-error):/m);
  assert.match(promote, /^          set -euo pipefail\n          node scripts\/assert-pc-runtime-release\.mjs$/m);
  assert.ok(promote.indexOf(command) < promote.indexOf('gh release edit '));
  const candidate = job(yaml, 'package');
  requireCandidateSources(candidate);
  assert.doesNotMatch(candidate, /assert-pc-runtime-release|pc_runtime_release_policy/);
  assert.ok(candidate.includes('controlled validation candidates'));
  assert.ok(candidate.includes('uses: actions/upload-artifact@'));
}

function requireCandidateSources(candidate) {
  assert.match(candidate, /^    needs: \[setup, desktop_core\]$/m);
  assert.match(candidate, /^    if: >-\n      always\(\) && needs\.setup\.result == 'success'\n      && \(needs\.desktop_core\.result == 'success'\n        \|\| \(needs\.desktop_core\.result == 'skipped' && inputs\.core_bundle_artifact != ''\)\)$/m);
}

function requireRiskPolicy(yaml) {
  const policy = job(yaml, 'pc_runtime_release_policy');
  assert.doesNotMatch(policy, /^    (?:if|needs|continue-on-error):/m);
  const verify = step(policy, 'Verify unconditional PC publication block').text;
  assert.doesNotMatch(verify, /^        (?:if|continue-on-error):/m);
  assert.match(verify, /^        run: node --test scripts\/assert-pc-runtime-release\.test\.mjs$/m);
  const gate = job(yaml, 'gate');
  assert.match(gate, /^    needs: \[[^\n]*pc_runtime_release_policy[^\n]*\]$/m);
  assert.ok(gate.includes('PC_RUNTIME_POLICY_RESULT: ${{ needs.pc_runtime_release_policy.result }}'));
  assert.ok(gate.includes('[ "$PC_RUNTIME_POLICY_RESULT" = success ] || {'));
  assert.match(gate, /^    if: always\(\)$/m);
  const events = yaml.split('jobs:')[0];
  assert.doesNotMatch(events, /^\s+(?:paths|paths-ignore):/m);
}

function runBlock(args = [], env = {}) {
  return spawnSync(process.execPath, [script, ...args], {
    cwd: root,
    encoding: 'utf8',
    env: { ...process.env, ...env },
  });
}

test('PC runtime script blocks publishing and labels candidates for controlled validation', () => {
  const result = runBlock();
  assert.equal(result.status, 1);
  assert.match(result.stderr, /PC_RUNTIME_RELEASE_BLOCKED/);
  assert.match(result.stderr, /sing-tun PC runtime Start\/PostStart, internal rollback, stale cleanup/);
  assert.match(result.stderr, /controlled validation/);
});

test('arguments, version changes, skip flags and claimed receipts cannot grant clearance', () => {
  for (const [args, env] of [
    [['--allow', '--skip'], {}],
    [['--version', '999.999.999'], {}],
    [[], { POLARIS_SKIP_PC_RUNTIME_RELEASE_GATE: '1' }],
    [[], { POLARIS_ALLOW_PC_RUNTIME_RELEASE: '1', PC_RUNTIME_RELEASE_READY: 'true' }],
    [[], { POLARIS_NO_KERNEL_RUN: '1', POLARIS_SKIP_QUALITY_GATES: '1' }],
    [[], { HELPER_STOP_ACK: 'stopped', PC_OWNER_STATE: 'NoOwner', GITHUB_REF_NAME: 'v999.999.999' }],
  ]) {
    const result = runBlock(args, env);
    assert.equal(result.status, 1, JSON.stringify({ args, env }));
    assert.match(result.stderr, /PC_RUNTIME_RELEASE_BLOCKED/);
  }
});

test('missing release block script fails closed', () => {
  const missing = join(root, 'scripts/__missing_pc_runtime_release_gate__.mjs');
  assert.equal(existsSync(missing), false);
  const result = spawnSync(process.execPath, [missing], { cwd: root, encoding: 'utf8' });
  assert.notEqual(result.status, 0);
});

test('desktop draft, uploads and final public promotion all require unconditional clearance', () => {
  requirePackagePolicy(packageYaml);
});

test('removing, skipping or swallowing a release clearance fails the policy', () => {
  const runLine = `        run: ${command}`;
  const mutants = [
    packageYaml.replace(runLine, ''),
    packageYaml.replace(runLine, `        # run: ${command}`),
    packageYaml.replace(runLine, `${runLine} || true`),
    packageYaml.replace(runLine, `        if: inputs.skip_quality_gates != true\n${runLine}`),
    packageYaml.replace(runLine, `        continue-on-error: true\n${runLine}`),
    packageYaml.replace(runLine, `        run: test "$SKIP" = 1 || ${command}`),
    packageYaml.replace('  release_desktop:\n', '  release_desktop:\n    continue-on-error: true\n'),
    packageYaml.replace(`          ${command}\n`, ''),
  ];
  for (const mutant of mutants) assert.throws(() => requirePackagePolicy(mutant));
});

test('candidate matrix remains buildable independently of publication block', () => {
  const candidate = job(packageYaml, 'package');
  requireCandidateSources(candidate);
  assert.doesNotMatch(candidate, /assert-pc-runtime-release|pc_runtime_release_policy/);
  assert.ok(step(candidate, 'Mark desktop artifacts as controlled validation candidates').text.includes('GITHUB_STEP_SUMMARY'));
});

test('Release Risk runs policy independently of path classification and requires it in final gate', () => {
  requireRiskPolicy(riskYaml);
});

test('path-selected or optional policy checks cannot make Release Risk green', () => {
  for (const mutant of [
    riskYaml.replace('  pc_runtime_release_policy:\n', '  pc_runtime_release_policy:\n    if: needs.classify.outputs.has_package == \'true\'\n'),
    riskYaml.replace('  pc_runtime_release_policy:\n', '  pc_runtime_release_policy:\n    needs: classify\n'),
    riskYaml.replace('        run: node --test scripts/assert-pc-runtime-release.test.mjs', '        run: node --test scripts/assert-pc-runtime-release.test.mjs || true'),
    riskYaml.replace('[ "$PC_RUNTIME_POLICY_RESULT" = success ] || {', '[ "$PC_RUNTIME_POLICY_RESULT" != failure ] || {'),
    riskYaml.replace(', pc_runtime_release_policy]', ']'),
  ]) assert.throws(() => requireRiskPolicy(mutant));
});

test('both policy files select package impact and their tests are mandatory in Node gate', () => {
  for (const path of ['scripts/assert-pc-runtime-release.mjs', 'scripts/assert-pc-runtime-release.test.mjs']) {
    assert.deepEqual(classifyImpact([path]).platforms, [...ALL_PACKAGE_PLATFORMS]);
    assert.equal(classifyImpact([path]).hasPackage, true);
  }
  const nodeGate = readFileSync(join(root, 'scripts/gate-node-test.sh'), 'utf8');
  const required = nodeGate.split('required_tests=(')[1].split('\n)')[0];
  assert.ok(required.includes('scripts/assert-pc-runtime-release.test.mjs'));
});
