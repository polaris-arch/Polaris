import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { ALL_PACKAGE_PLATFORMS, classifyImpact } from './classify-ci-impact.mjs';
import { createHash } from 'node:crypto';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts/assert-pc-runtime-release.mjs');
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
    'scripts/desktop-core/release-consumer.mjs', 'scripts/desktop-core/upstream-release.mjs', 'scripts/lib/polaris-version.json',
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
