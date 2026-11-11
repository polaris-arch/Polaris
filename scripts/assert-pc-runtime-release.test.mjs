import assert from 'node:assert/strict';
import test from 'node:test';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { ALL_PACKAGE_PLATFORMS, classifyImpact } from './classify-ci-impact.mjs';
import { createHash } from 'node:crypto';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts/assert-pc-runtime-release.mjs');
const command = 'node scripts/assert-pc-runtime-release.mjs';
const clearanceName = 'Require selected formal release source policy';
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
  assert.ok(candidate.includes('selected source provenance'));
  assert.ok(candidate.includes('does not authorize publication or establish native/device acceptance'));
  assert.ok(candidate.includes('uses: actions/upload-artifact@'));
}

function requireCandidateSources(candidate) {
  assert.match(candidate, /^    needs: setup$/m);
  // `!cancelled()` (2026-10-07) lets a superseded run stop its in-flight
  // installer legs, whereas `always()` kept them running to the end.
  assert.doesNotMatch(candidate, /^      always\(\)/m, 'installer legs must stay cancellable');
  assert.match(candidate, /^    if: >-\n      \(!cancelled\(\) && needs\.setup\.result == 'success'\)$/m);
}

function requireRiskPolicy(yaml) {
  const policy = job(yaml, 'pc_runtime_release_policy');
  assert.doesNotMatch(policy, /^    (?:if|needs|continue-on-error):/m);
  const verify = step(policy, 'Verify fail-closed source-first release policy').text;
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

function runPolicy(args = [], env = {}, policyScript = script) {
  return spawnSync(process.execPath, [policyScript, ...args], {
    cwd: root,
    encoding: 'utf8',
    env: { ...process.env, ...env },
  });
}

const manifestPath = 'src-tauri/core-manifest.json';
function requireUnobserved(policy, state = 'FORMAL_CONSUMER_SOURCE_ELIGIBLE') {
  assert.equal(policy.policyState, state);
  assert.equal(policy.publicationRequirements, 'existing-signed-release-dag');
  assert.equal(policy.nativeAcceptance, 'NotObserved');
  assert.equal(policy.deviceAcceptance, 'NotObserved');
}

test('arguments cannot bypass source checks or grant native clearance', () => {
  for (const args of [['--allow', '--skip'], ['--version', '999.999.999']]) {
    const result = runPolicy(args);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /PC_RUNTIME_RELEASE_BLOCKED.*no arguments/);
  }
});

test('missing release policy script fails closed', () => {
  const missing = join(root, 'scripts/__missing_pc_runtime_release_gate__.mjs');
  assert.equal(existsSync(missing), false);
  const result = spawnSync(process.execPath, [missing], { cwd: root, encoding: 'utf8' });
  assert.notEqual(result.status, 0);
});

test('desktop draft, uploads and final public promotion all require the source-first policy', () => {
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

test('candidate matrix preserves strict source consumption and states unobserved acceptance', () => {
  const candidate = job(packageYaml, 'package');
  requireCandidateSources(candidate);
  const condition = candidate.match(/^    if: >-\n(?:      .*\n)+/m)?.[0];
  assert.ok(condition, 'candidate condition must be present for negative controls');
  for (const [before, after] of [
    ['!cancelled()', 'always()'],
    ["needs.setup.result == 'success'", "needs.setup.result != 'failure'"],
  ]) {
    const changed = condition.replace(before, after);
    assert.notEqual(changed, condition, `mutation must exercise ${before}`);
    assert.throws(() => requireCandidateSources(candidate.replace(condition, changed)));
  }
  assert.doesNotMatch(candidate, /assert-pc-runtime-release|pc_runtime_release_policy/);
  const summary = step(candidate, 'Describe source-first desktop artifacts').text;
  assert.ok(summary.includes('GITHUB_STEP_SUMMARY'));
  assert.ok(summary.includes('Publication still requires the complete signed Release DAG'));
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

test('both policy files select package impact', () => {
  for (const path of ['scripts/assert-pc-runtime-release.mjs', 'scripts/assert-pc-runtime-release.test.mjs']) {
    assert.deepEqual(classifyImpact([path]).platforms, [...ALL_PACKAGE_PLATFORMS]);
    assert.equal(classifyImpact([path]).hasPackage, true);
  }
});


test('actual production CLI admits only the selected formal contract without runtime clearance', () => {
  const manifest = JSON.parse(readFileSync(join(root, manifestPath)));
  const contract = JSON.parse(readFileSync(join(root, manifest.desktopRelease.contractPath)));
  for (const env of [{}, { POLARIS_SKIP_PC_RUNTIME_RELEASE_GATE: '1' },
    { POLARIS_ALLOW_PC_RUNTIME_RELEASE: '1', PC_RUNTIME_RELEASE_READY: 'true' },
    { HELPER_STOP_ACK: 'stopped', PC_OWNER_STATE: 'NoOwner', GITHUB_REF_NAME: 'v999.999.999' }]) {
    const result = runPolicy([], env);
    assert.equal(result.status, 0, result.stderr);
    const policy = JSON.parse(result.stdout);
    requireUnobserved(policy, 'FORMAL_CONSUMER_SOURCE_ELIGIBLE');
    assert.equal(policy.contractSha256, manifest.desktopRelease.contractSha256);
    assert.equal(policy.sourceCommit, contract.sourceCommit);
  }
});


function formalPolicyFixture(t) {
  const directory = mkdtempSync(join(tmpdir(), 'formal-source-publication-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const paths = ['scripts/assert-pc-runtime-release.mjs', 'scripts/desktop-core/source-graph.mjs',
    'scripts/desktop-core/release-consumer.mjs', 'scripts/desktop-core/upstream-release.mjs', 'scripts/desktop-core/recovery.mjs', 'scripts/lib/polaris-version.json',
    'scripts/desktop-core/local-development.mjs',
    'scripts/lib/core-identity.mjs', 'scripts/lib/cronet-payload.mjs', manifestPath,
    'scripts/desktop-core/release-contract.json'];
  const files = new Map(paths.map(path => [path, readFileSync(join(root, path))]));
  const reset = () => { for (const [path, bytes] of files) {
    mkdirSync(dirname(join(directory, path)), { recursive: true });
    writeFileSync(join(directory, path), bytes);
  } };
  reset();
  return { directory, files, reset, run: env => runPolicy([], env, join(directory, 'scripts/assert-pc-runtime-release.mjs')) };
}

test('copied release adapter requires its shared grammar with no missing-file fallback', t => {
  const f = formalPolicyFixture(t);
  rmSync(join(f.directory, 'scripts/lib/polaris-version.json'));
  const result = f.run({});
  assert.equal(result.status, 1);
  assert.match(result.stderr, /ENOENT/);
  assert.match(result.stderr, /polaris-version\.json/);
});

test('formal publication source admission uses the exact strict selected contract', t => {
  const f = formalPolicyFixture(t), manifest = JSON.parse(f.files.get(manifestPath));
  const contract = JSON.parse(f.files.get('scripts/desktop-core/release-contract.json'));
  const result = f.run({});
  assert.equal(result.status, 0, result.stderr);
  const policy = JSON.parse(result.stdout);
  requireUnobserved(policy, 'FORMAL_CONSUMER_SOURCE_ELIGIBLE');
  assert.equal(policy.contractSha256, manifest.desktopRelease.contractSha256);
  for (const key of ['sourceCommit', 'sourceTree', 'tagObject', 'releaseId']) assert.equal(policy[key], contract[key]);
  assert.equal(Object.hasOwn(policy, 'reviewedCandidate'), false, 'internal review identity is not a publication input');
  assert.equal(Object.hasOwn(policy, 'AppAccepted'), false, 'no new acceptance receipt is manufactured');
  const path = 'scripts/desktop-core/release-contract.json';
  f.reset(); rmSync(join(f.directory, path));
  assert.equal(f.run({}).status, 1, path);
  f.reset(); writeFileSync(join(f.directory, path), Buffer.concat([f.files.get(path), Buffer.from(' ')]));
  assert.equal(f.run({}).status, 1, path);
});

test('formal source gate rejects malformed, aliased and extended selections', t => {
  const f = formalPolicyFixture(t);
  const base = JSON.parse(f.files.get(manifestPath));
  for (const mutate of [
    m => { m.desktopRelease = null; }, m => { m.desktopRelease = {}; },
    m => { m.desktopRelease.contractPath = 'scripts/desktop-core/pending-release-contract.json'; },
    m => { m.desktopRelease.contractSha256 = '0'.repeat(64); },
    m => { m.desktopRelease.sourceCommit = '0'.repeat(40); },
    m => { m.desktopRelease.librarySha256 = { linux: '0'.repeat(64) }; },
    m => { m.desktopRelease.AppAccepted = true; },
  ]) {
    f.reset(); const changed = structuredClone(base); mutate(changed);
    writeFileSync(join(f.directory, manifestPath), JSON.stringify(changed));
    const result = f.run({ POLARIS_ALLOW_PC_RUNTIME_RELEASE: '1', PC_RUNTIME_RELEASE_READY: 'true',
      POLARIS_SKIP_PC_RUNTIME_RELEASE_GATE: '1', HELPER_STOP_ACK: 'stopped', PC_OWNER_STATE: 'NoOwner' });
    assert.equal(result.status, 1, `${mutate}: ${result.stderr}`);
    assert.match(result.stderr, /PC_RUNTIME_RELEASE_BLOCKED/);
  }
});

test('formal contract selection is the exact selected byte hash', t => {
  const f = formalPolicyFixture(t), contractPath = 'scripts/desktop-core/release-contract.json';
  const hash = bytes => createHash('sha256').update(bytes).digest('hex');
  const manifest = JSON.parse(f.files.get(manifestPath));
  const contract = JSON.parse(f.files.get(contractPath));
  // Formatting is not an independent approval identity: the selected byte hash is authoritative.
  const raw = Buffer.from(JSON.stringify(contract) + '\n');
  manifest.desktopRelease.contractSha256 = hash(raw);
  writeFileSync(join(f.directory, contractPath), raw);
  writeFileSync(join(f.directory, manifestPath), JSON.stringify(manifest));
  assert.equal(f.run({}).status, 0);
});
