import assert from 'node:assert/strict';
import test from 'node:test';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { ALL_PACKAGE_PLATFORMS, classifyImpact } from './classify-ci-impact.mjs';
import { createHash } from 'node:crypto';
import { assertOwnershipChainAbsent, assertSourceFirstRelease, OWNERSHIP_CHAIN_PATCHES } from './assert-pc-runtime-release.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts/assert-pc-runtime-release.mjs');
const command = 'node scripts/assert-pc-runtime-release.mjs';
const clearanceName = 'Require reviewed source-first release policy';
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
  assert.ok(candidate.includes('reviewed source-first release policy'));
  assert.ok(candidate.includes('Native/device acceptance is NotObserved'));
  assert.ok(candidate.includes('Exact/NoOwner remain Unknown and managed status is NotGranted'));
  assert.ok(candidate.includes('uses: actions/upload-artifact@'));
}

function requireCandidateSources(candidate) {
  assert.match(candidate, /^    needs: \[setup, desktop_core\]$/m);
  // The source allowlist is unchanged and still matched in full. Only the status
  // function differs: `!cancelled()` (2026-10-07) lets a superseded run stop its
  // in-flight installer legs, whereas `always()` kept them running to the end.
  // Both disable default skip propagation, which is what this allowlist relies on.
  // The authorization-only preflight must not enter this installer matrix.
  assert.doesNotMatch(candidate, /^      always\(\)/m, 'installer legs must stay cancellable');
  assert.match(candidate, /^    if: >-\n      inputs\.linux_auth_preflight_only != true && \(!cancelled\(\) && needs\.setup\.result == 'success'\n      && \(needs\.desktop_core\.result == 'success'\n        \|\| \(needs\.desktop_core\.result == 'skipped' && inputs\.core_bundle_artifact != ''\)\)\)$/m);
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

const sourcePath = 'scripts/libbox-patches/source-manifest.json';
const manifestPath = 'src-tauri/core-manifest.json';
const source = JSON.parse(readFileSync(join(root, sourcePath)));
// Deliberately inert bytes: policy depends on the retired names, not old implementation
// or review transcripts. Matching hashes must not make these inputs admissible.
const retiredOwnershipFixtures = new Map(OWNERSHIP_CHAIN_PATCHES.map((file) => [
  `scripts/libbox-patches/${file}`, Buffer.from(`inert retired ownership input: ${file}\n`),
]));
const inputPaths = [manifestPath, sourcePath, 'scripts/core-source-provision.py',
  ...source.patches.map((patch) => `scripts/libbox-patches/${patch.file}`),
  ...source.dependencyPatches.map((dep) => `scripts/libbox-patches/${dep.patchFile}`),
  'scripts/core-patches/windows-dns-refresh.patch'];
// Exact legacy manifest from accepted 36230f41, before D data activation.
const legacyManifestBytes = readFileSync(join(root, 'scripts/desktop-core/fixtures/legacy-core-manifest.json'));
const inputs = new Map(inputPaths.map((path) => [path, path === manifestPath ? legacyManifestBytes : readFileSync(join(root, path))]));
function fixtureRead(changes = new Map()) {
  return (path) => {
    const bytes = changes.has(path) ? changes.get(path) : inputs.get(path);
    assert.ok(bytes !== undefined, `missing source input: ${path}`);
    return bytes;
  };
}
function manifestRead(mutate) {
  const manifest = JSON.parse(inputs.get(manifestPath));
  mutate(manifest);
  return fixtureRead(new Map([[manifestPath, Buffer.from(JSON.stringify(manifest))]]));
}
function requireUnobserved(policy, state = 'SOURCE_FIRST_RELEASE_ELIGIBLE') {
  assert.equal(policy.policyState, state);
  assert.equal(policy.publicationRequirements, 'existing-signed-release-dag');
  assert.equal(policy.nativeAcceptance, 'NotObserved');
  assert.equal(policy.deviceAcceptance, 'NotObserved');
  assert.equal(policy.Exact, 'Unknown');
  assert.equal(policy.NoOwner, 'Unknown');
  assert.equal(policy.managed, 'NotGranted');
}

test('only reviewed source inputs are eligible, without native or ownership clearance', () => {
  const policy = assertSourceFirstRelease(fixtureRead());
  requireUnobserved(policy);
  assert.equal(policy.reviewedCandidate, '123259cb4ee0eef484368e34d8ea7211d39964b6');
  assert.deepEqual(assertSourceFirstRelease(fixtureRead()), policy);
  for (const path of inputPaths) {
    assert.throws(() => assertSourceFirstRelease(fixtureRead(new Map([[path, undefined]]))), /missing source input/);
    assert.throws(() => assertSourceFirstRelease(fixtureRead(new Map([[path,
      Buffer.concat([inputs.get(path), Buffer.from('\nchanged-source')])]]))), undefined, path);
  }
  for (const mutate of [
    (manifest) => { manifest.sourceBuild.sourceManifestSha256 = 'a'.repeat(64); },
    (manifest) => { manifest.sourceBuild.provisionerSha256 = 'a'.repeat(64); },
    (manifest) => { manifest.sourceBuild.sourceReceiptFingerprint = 'a'.repeat(64); },
    (manifest) => { manifest.sourceBuild.moduleGraphSha256 = 'a'.repeat(64); },
    (manifest) => { manifest.sourceBuild.mainGoModSha256 = 'a'.repeat(64); },
    (manifest) => { manifest.sourceBuild.mainGoSumSha256 = 'a'.repeat(64); },
    (manifest) => { manifest.sourceBuild.patchedSourceTree = 'a'.repeat(40); },
    (manifest) => { manifest.sourceBuild.platforms.win.buildTree = 'a'.repeat(40); },
    (manifest) => { manifest.sourceBuild.transportPins['golang.org/x/sys'] = 'v0.48.0'; },
    (manifest) => { delete manifest.sourceBuild.platforms['mac-arm64']; },
    ...['linux', 'win', 'mac-x64', 'mac-arm64'].map((key) => (manifest) => {
      manifest.sourceBuild.platforms[key].patchedModules.allowedAbsent.push('example.com/absent');
    }),
    (manifest) => { manifest.windowsBuild.patchSha256 = 'a'.repeat(64); },
    (manifest) => {
      manifest.sourceBuild.version = manifest.sourceBuild.version.replace(/\d+$/, (suffix) => String(Number(suffix) + 1));
    },
    (manifest) => { manifest.clearance = 'approved'; },
  ]) assert.throws(() => assertSourceFirstRelease(manifestRead(mutate)));

  // These are historical output pins, not review authority for newly produced
  // binaries. Actual all-four source bundle consumption remains mandatory.
  requireUnobserved(assertSourceFirstRelease(manifestRead((manifest) => {
    manifest.windowsBuild.binarySha256 = null;
    for (const platform of Object.values(manifest.sourceBuild.platforms)) platform.binarySha256 = null;
  })));
});

test('ownership chain inputs are rejected by name, not as a digest drift', () => {
  const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
  // Rebind the manifest to the changed source so no file digest differs.
  const withSource = (mutateSource, mutateManifest = () => {}) => {
    const changed = JSON.parse(inputs.get(sourcePath));
    mutateSource(changed);
    const bytes = Buffer.from(JSON.stringify(changed));
    const manifest = JSON.parse(inputs.get(manifestPath));
    manifest.sourceBuild.sourceManifestSha256 = sha(bytes);
    mutateManifest(manifest);
    return fixtureRead(new Map([...retiredOwnershipFixtures,
      [sourcePath, bytes], [manifestPath, Buffer.from(JSON.stringify(manifest))]]));
  };
  const patchBytes = (file) => retiredOwnershipFixtures.get(`scripts/libbox-patches/${file}`);
  // Unchanged rebinding reaches the reviewed-input digest, so the helper is live.
  assert.throws(() => assertSourceFirstRelease(withSource((changed) => { changed.goVersion = '1.25.6'; })),
    /Source inputs differ from the reviewed release policy/);
  assert.deepEqual([...OWNERSHIP_CHAIN_PATCHES].sort(), ['nftables-owned-transactions.patch',
    'sing-tun-owned-native.patch', 'tun-owner-consumer.patch']);
  assert.throws(() => assertSourceFirstRelease(withSource((changed) => {
    changed.patches.push({ file: 'tun-owner-consumer.patch', sha256: sha(patchBytes('tun-owner-consumer.patch')) });
  })), /Ownership chain policy: retired patch listed/);
  // Every name is refused even when the synthetic bytes and listed digest agree.
  for (const file of OWNERSHIP_CHAIN_PATCHES) {
    assert.throws(() => assertSourceFirstRelease(withSource((changed) => {
      changed.patches.push({ file, sha256: sha(patchBytes(file)) });
    })), /Ownership chain policy: retired patch listed/, file);
  }
  const module = 'example.com/patched';
  const declare = (manifest) => {
    manifest.sourceBuild.dependencyModules.push(module);
    for (const platform of Object.values(manifest.sourceBuild.platforms)) platform.patchedModules.requiredLinked.push(module);
  };
  const dependency = { name: 'patched', module, sourceURL: 'https://github.com/example/patched.git',
    upstreamVersion: 'v1.0.0', upstreamCommit: '1'.repeat(40), patchFile: 'sing-tun-owned-native.patch',
    patchSha256: sha(patchBytes('sing-tun-owned-native.patch')), patchedTree: '2'.repeat(40) };
  for (const read of [
    withSource((changed) => { changed.dependencyPatches.push(dependency); }, declare),
    withSource((changed) => { changed.dependencyPatches.push(dependency); }),
    manifestRead(declare),
  ]) assert.throws(() => assertSourceFirstRelease(read), /Ownership chain policy: dependency patches must be empty/);
  assert.throws(() => assertSourceFirstRelease(manifestRead((manifest) => {
    manifest.sourceBuild.graphScope = 'declared-patched-modules';
  })), /Ownership chain policy: graph scope must be core-source-only/);

  // Each clause alone, on otherwise accepted inputs.
  const spec = () => JSON.parse(inputs.get(manifestPath)).sourceBuild;
  const accepted = () => JSON.parse(inputs.get(sourcePath));
  assertOwnershipChainAbsent(spec(), accepted());
  for (const field of ['requiredLinked', 'allowedAbsent']) {
    for (const key of ['linux', 'win', 'mac-x64', 'mac-arm64']) {
      const changed = spec();
      changed.platforms[key].patchedModules[field].push(module);
      assert.throws(() => assertOwnershipChainAbsent(changed, accepted()),
        /Ownership chain policy: platform patched modules must be empty/, `${key} ${field}`);
    }
  }
  for (const mutate of [(changed) => { delete changed.dependencyPatches; },
    (changed) => { changed.dependencyPatches = null; }]) {
    const changed = accepted();
    mutate(changed);
    assert.throws(() => assertOwnershipChainAbsent(spec(), changed), /dependency patches must be empty/);
  }
});

test('current committed mobile source graph retains the empty ownership policy', () => {
  const manifest = JSON.parse(readFileSync(join(root, manifestPath)));
  const currentSource = JSON.parse(readFileSync(join(root, sourcePath)));
  assertOwnershipChainAbsent(manifest.sourceBuild, currentSource);
  assert.deepEqual(currentSource.dependencyPatches, []);
  assert.deepEqual(manifest.sourceBuild.dependencyModules, []);
  assert.equal(manifest.sourceBuild.graphScope, 'core-source-only');
  for (const platform of Object.values(manifest.sourceBuild.platforms)) {
    assert.deepEqual(platform.patchedModules, { requiredLinked: [], allowedAbsent: [] });
  }
  for (const file of OWNERSHIP_CHAIN_PATCHES) {
    const changed = structuredClone(currentSource);
    changed.patches.push({ file, sha256: createHash('sha256')
      .update(retiredOwnershipFixtures.get(`scripts/libbox-patches/${file}`)).digest('hex') });
    assert.throws(() => assertOwnershipChainAbsent(manifest.sourceBuild, changed),
      /Ownership chain policy: retired patch listed/, file);
  }
});

test('retired ownership inputs reject before artifact reads with self-consistent synthetic hashes', () => {
  const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
  for (const file of OWNERSHIP_CHAIN_PATCHES) {
    const artifactPath = `scripts/libbox-patches/${file}`;
    const changed = JSON.parse(inputs.get(sourcePath));
    changed.patches.push({ file, sha256: sha(retiredOwnershipFixtures.get(artifactPath)) });
    const bytes = Buffer.from(JSON.stringify(changed));
    const manifest = JSON.parse(inputs.get(manifestPath));
    manifest.sourceBuild.sourceManifestSha256 = sha(bytes);
    const read = fixtureRead(new Map([...retiredOwnershipFixtures,
      [sourcePath, bytes], [manifestPath, Buffer.from(JSON.stringify(manifest))]]));
    const observedReads = [];
    assert.throws(() => assertSourceFirstRelease((path) => {
      observedReads.push(path);
      return read(path);
    }), /Ownership chain policy: retired patch listed/, file);
    assert.deepEqual(observedReads, [manifestPath, sourcePath],
      'a forbidden name must be rejected before loading any artifact or reporting digest drift');
  }
});

test('arguments, skip flags and claimed receipts cannot bypass source checks or grant native clearance', () => {
  for (const args of [['--allow', '--skip'], ['--version', '999.999.999']]) {
    const result = runPolicy(args);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /PC_RUNTIME_RELEASE_BLOCKED.*no arguments/);
  }
  const fixture = mkdtempSync(join(tmpdir(), 'polaris-source-release-policy-'));
  try {
    // Exercise the actual CLI with a changed source input under every claimed
    // clearance. Only copies in this private fixture are modified.
    const fixturePaths = [...inputPaths, 'scripts/assert-pc-runtime-release.mjs',
      'scripts/desktop-core/source-graph.mjs', 'scripts/desktop-core/release-consumer.mjs',
      // release-consumer statically imports the upstream adapter even for legacy inputs.
      'scripts/desktop-core/upstream-release.mjs', 'scripts/lib/polaris-version.json', 'scripts/desktop-core/native-dns-candidate.mjs',
      'scripts/lib/core-identity.mjs',
      'scripts/lib/cronet-payload.mjs', 'scripts/lib/cronet-contract.mjs'];
    for (const path of fixturePaths) {
      mkdirSync(dirname(join(fixture, path)), { recursive: true });
      writeFileSync(join(fixture, path), inputs.get(path) ?? readFileSync(join(root, path)));
    }
    writeFileSync(join(fixture, 'scripts/core-source-provision.py'), 'changed-source');
    for (const env of [
      { POLARIS_SKIP_PC_RUNTIME_RELEASE_GATE: '1' },
      { POLARIS_ALLOW_PC_RUNTIME_RELEASE: '1', PC_RUNTIME_RELEASE_READY: 'true' },
      { POLARIS_NO_KERNEL_RUN: '1', POLARIS_SKIP_QUALITY_GATES: '1' },
      { HELPER_STOP_ACK: 'stopped', PC_OWNER_STATE: 'NoOwner', GITHUB_REF_NAME: 'v999.999.999' },
    ]) {
      writeFileSync(join(fixture, 'scripts/core-source-provision.py'), inputs.get('scripts/core-source-provision.py'));
      const good = runPolicy([], env, join(fixture, 'scripts/assert-pc-runtime-release.mjs'));
      assert.equal(good.status, 0, good.stderr);
      requireUnobserved(JSON.parse(good.stdout));
      writeFileSync(join(fixture, 'scripts/core-source-provision.py'), 'changed-source');
      const bad = runPolicy([], env, join(fixture, 'scripts/assert-pc-runtime-release.mjs'));
      assert.equal(bad.status, 1, JSON.stringify(env));
      assert.match(bad.stderr, /PC_RUNTIME_RELEASE_BLOCKED.*core-source-provision\.py/);
    }
  } finally {
    rmSync(fixture, { recursive: true, force: true });
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
    ['inputs.linux_auth_preflight_only != true && ', ''],
    ['inputs.linux_auth_preflight_only != true', 'inputs.linux_auth_preflight_only == true'],
    ['inputs.linux_auth_preflight_only != true && ', 'inputs.linux_auth_preflight_only != true || '],
    ['!cancelled()', 'always()'],
    ["needs.setup.result == 'success'", "needs.setup.result != 'failure'"],
    ["needs.desktop_core.result == 'success'", "needs.desktop_core.result != 'failure'"],
    ["needs.desktop_core.result == 'skipped' && inputs.core_bundle_artifact != ''",
      "needs.desktop_core.result == 'skipped' || inputs.core_bundle_artifact != ''"],
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

test('both policy files select package impact and their tests are mandatory in Node gate', () => {
  for (const path of ['scripts/assert-pc-runtime-release.mjs', 'scripts/assert-pc-runtime-release.test.mjs']) {
    assert.deepEqual(classifyImpact([path]).platforms, [...ALL_PACKAGE_PLATFORMS]);
    assert.equal(classifyImpact([path]).hasPackage, true);
  }
  const nodeGate = readFileSync(join(root, 'scripts/gate-node-test.sh'), 'utf8');
  const required = nodeGate.split('required_tests=(')[1].split('\n)')[0];
  assert.ok(required.includes('scripts/assert-pc-runtime-release.test.mjs'));
});


function desktopPolicyFixture() {
  const manifest = JSON.parse(inputs.get(manifestPath));
  const desktop = structuredClone(manifest.sourceBuild);
  desktop.sourceMode = 'fork-commit-v1'; desktop.upstreamVersion = '2.0.0-alpha.11'; desktop.version = '2.0.0-alpha.11.polaris.1';
  desktop.mainGoModSha256 = 'a'.repeat(64); desktop.mainGoSumSha256 = 'b'.repeat(64);
  desktop.buildTree = desktop.patchedSourceTree;
  for (const platform of Object.values(desktop.platforms)) platform.buildTree = desktop.buildTree;
  const source = { schema: 'polaris-desktop-fork-source-v1', role: 'desktop',
    sourceURL: 'https://github.com/polaris-arch/polaris-box', sourceCommit: 'a'.repeat(40), sourceTree: desktop.buildTree,
    sourceTag: 'polaris-v2.0.0-alpha.11-1', sourceTagObject: 'b'.repeat(40), upstreamTag: 'v2.0.0-alpha.11',
    upstreamCommit: 'c'.repeat(40), goVersion: '1.25.5', patches: [], dependencyPatches: [] };
  const bytes = JSON.stringify(source);
  desktop.sourceManifestSha256 = createHash('sha256').update(bytes).digest('hex');
  manifest.desktopSourceBuild = desktop; delete manifest.windowsBuild;
  const changes = new Map([['scripts/desktop-core/source-manifest.json', bytes],
    ['scripts/libbox-ios-patches/source-manifest.json', readFileSync(join(root, 'scripts/libbox-ios-patches/source-manifest.json'))],
    [manifestPath, JSON.stringify(manifest)]]);
  return { manifest, source, changes };
}

test('new D source closure selects D bytes and remains publication-blocked pending actual reviewed pins', () => {
  const f = desktopPolicyFixture();
  assert.throws(() => assertSourceFirstRelease(fixtureRead(f.changes)), /exact D release input closure not recorded/);
  f.changes.set('scripts/desktop-core/source-manifest.json', JSON.stringify({ ...f.source, role: 'mobile' }));
  assert.throws(() => assertSourceFirstRelease(fixtureRead(f.changes)), /desktop source SHA-256/);
  for (const invalid of [null, [], {}, f.manifest.sourceBuild]) {
    f.manifest.desktopSourceBuild = invalid;
    f.changes.set(manifestPath, JSON.stringify(f.manifest));
    assert.throws(() => assertSourceFirstRelease(fixtureRead(f.changes)), /desktopSourceBuild|not frozen/);
  }
});

test('D source admission retains the complete mobile graph, source and provider byte bindings', () => {
  for (const field of ['bundledCoreVersion', 'sourceBuild']) {
    const f = desktopPolicyFixture();
    if (field === 'bundledCoreVersion') f.manifest.bundledCoreVersion = '9.9.9';
    else f.manifest.sourceBuild.transportPins['example.invalid/unreviewed'] = 'v1.0.0';
    f.changes.set(manifestPath, JSON.stringify(f.manifest));
    assert.throws(() => assertSourceFirstRelease(fixtureRead(f.changes)), /not frozen|Mobile source inputs/);
  }
  const patch = source.patches[0];
  for (const path of [sourcePath, `scripts/libbox-patches/${patch.file}`, 'scripts/libbox-ios-patches/source-manifest.json']) {
    const f = desktopPolicyFixture(); f.changes.set(path, Buffer.from('unreviewed changed bytes'));
    assert.throws(() => assertSourceFirstRelease(fixtureRead(f.changes)), /Reviewed mobile source SHA-256/);
  }
  const f = desktopPolicyFixture(); f.changes.set('scripts/core-source-provision.py', Buffer.from('changed provider'));
  assert.throws(() => assertSourceFirstRelease(fixtureRead(f.changes)), /desktop provisioner SHA-256/);
});


test('actual production CLI admits only its selected reviewed source without claimed runtime clearance', () => {
  const manifest = JSON.parse(readFileSync(join(root, manifestPath)));
  for (const env of [{}, { POLARIS_SKIP_PC_RUNTIME_RELEASE_GATE: '1' },
    { POLARIS_ALLOW_PC_RUNTIME_RELEASE: '1', PC_RUNTIME_RELEASE_READY: 'true' },
    { HELPER_STOP_ACK: 'stopped', PC_OWNER_STATE: 'NoOwner', GITHUB_REF_NAME: 'v999.999.999' }]) {
    const result = runPolicy([], env);
    if (Object.hasOwn(manifest, 'desktopRelease') && !Object.hasOwn(manifest, 'desktopTemporary')) {
      assert.equal(result.status, 0, result.stderr);
      const policy = JSON.parse(result.stdout);
      requireUnobserved(policy, 'FORMAL_CONSUMER_SOURCE_ELIGIBLE');
      assert.equal(policy.reviewedContractSha256, manifest.desktopRelease.contractSha256);
      assert.equal(policy.sourceCommit, manifest.desktopRelease.sourceCommit);
    } else if (Object.hasOwn(manifest, 'desktopSourceBuild')) {
      assert.equal(result.status, 1, result.stderr);
      assert.match(result.stderr, /PC_RUNTIME_RELEASE_BLOCKED: (?:Temporary desktop default is not formal App release approval; packaged acceptance remains unobserved|Published release consumption does not inherit source-first formal App release approval|Desktop fork source review must be renewed: exact D release input closure not recorded|DNS source-owned release input closure\/signatures\/output pins not recorded)/);
    } else {
      assert.equal(result.status, 0, result.stderr);
      requireUnobserved(JSON.parse(result.stdout));
    }
  }
});

test('source-owned schema readiness cannot authorize source-first DNS publication', () => {
  const source = readFileSync(join(root, 'scripts/assert-pc-runtime-release.mjs'), 'utf8');
  assert.match(source, /requireGraph\(!sourceOwnedReplacements\(spec\),\s*'DNS source-owned release input closure\/signatures\/output pins not recorded'\)/);
  const manifest = JSON.parse(readFileSync(join(root, manifestPath)));
  assert.equal(Object.hasOwn(manifest.desktopSourceBuild, 'sourceOwnedReplacements'), true);
  assert.equal(JSON.parse(readFileSync(join(root, 'scripts/desktop-core/source-manifest.json'))).sourceCommit,
    'aafc521b745e0d1a8f50a6a3f41169433f1c2194');
  // The old source-owned input alone still cannot use the newly reviewed
  // published consumer admission. Explicitly remove the active selection.
  delete manifest.desktopRelease;
  assert.throws(() => assertSourceFirstRelease(path => path === manifestPath
    ? Buffer.from(JSON.stringify(manifest)) : readFileSync(join(root, path))),
  /DNS source-owned release input closure\/signatures\/output pins not recorded/);
});


test('temporary desktop defaults cannot inherit formal publication approval or hide malformed selections', () => {
  for (const selection of [null, false, {}, { schema: 'polaris-app-temporary-desktop-selection-v1', publicationEligible: true }]) {
    assert.throws(() => assertSourceFirstRelease(manifestRead((manifest) => {
      manifest.desktopTemporary = selection;
    })), /Temporary desktop default is not formal App release approval/);
  }
  const actual = JSON.parse(readFileSync(join(root, manifestPath)));
  if (Object.hasOwn(actual, 'desktopTemporary')) assert.throws(() => assertSourceFirstRelease(), /Temporary desktop default/);
});


function formalPolicyFixture(t) {
  const directory = mkdtempSync(join(tmpdir(), 'formal-source-publication-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const paths = ['scripts/assert-pc-runtime-release.mjs', 'scripts/desktop-core/source-graph.mjs',
    'scripts/desktop-core/release-consumer.mjs', 'scripts/desktop-core/upstream-release.mjs', 'scripts/lib/polaris-version.json',
    'scripts/desktop-core/native-dns-candidate.mjs', 'scripts/lib/core-identity.mjs',
    'scripts/lib/cronet-payload.mjs', 'scripts/lib/cronet-contract.mjs', manifestPath,
    'scripts/desktop-core/release-contract.json', 'scripts/desktop-core/release-asset-index.json'];
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

test('formal publication source admission uses the exact strict selected contract and committed index', t => {
  const f = formalPolicyFixture(t), manifest = JSON.parse(f.files.get(manifestPath));
  const result = f.run({});
  assert.equal(result.status, 0, result.stderr);
  const policy = JSON.parse(result.stdout);
  requireUnobserved(policy, 'FORMAL_CONSUMER_SOURCE_ELIGIBLE');
  assert.equal(policy.reviewedContractSha256, manifest.desktopRelease.contractSha256);
  for (const key of ['sourceCommit', 'sourceTree', 'tagObject', 'releaseId']) assert.equal(policy[key], manifest.desktopRelease[key]);
  assert.equal(Object.hasOwn(policy, 'reviewedSourceIncrement'), false, 'historical ownership proof is not transferred');
  assert.equal(Object.hasOwn(policy, 'AppAccepted'), false, 'no new acceptance receipt is manufactured');
  for (const path of ['scripts/desktop-core/release-contract.json', 'scripts/desktop-core/release-asset-index.json']) {
    f.reset(); rmSync(join(f.directory, path));
    assert.equal(f.run({}).status, 1, path);
    f.reset(); writeFileSync(join(f.directory, path), Buffer.concat([f.files.get(path), Buffer.from(' ')]));
    assert.equal(f.run({}).status, 1, path);
  }
});

test('formal source gate rejects malformed, mixed, aliased and historically relabeled selections', t => {
  const f = formalPolicyFixture(t);
  const base = JSON.parse(f.files.get(manifestPath));
  for (const mutate of [
    m => { m.desktopRelease = null; }, m => { m.desktopRelease = {}; },
    m => { m.desktopTemporary = null; }, m => { m.desktopTemporary = m.historicalDesktopTemporary; },
    m => { m.desktopRelease = m.historicalDesktopRelease; },
    m => { m.desktopRelease.contractPath = 'scripts/desktop-core/pending-release-contract.json'; },
    m => { m.desktopRelease.contractSha256 = '0'.repeat(64); },
    m => { m.desktopRelease.sourceCommit = m.historicalDesktopTemporary.sourceCommit; },
    m => { m.desktopRelease.sourceTree = '0'.repeat(40); },
    m => { m.desktopRelease.tagObject = '0'.repeat(40); },
    m => { m.desktopRelease.version = '1.15.0-alpha.12'; },
    m => { m.desktopRelease.librarySha256.linux = '0'.repeat(64); },
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

test('structurally valid rebound formal contracts still require source review renewal', t => {
  const f = formalPolicyFixture(t), contractPath = 'scripts/desktop-core/release-contract.json';
  const hash = bytes => createHash('sha256').update(bytes).digest('hex');
  for (const producerChange of [false, true]) {
    f.reset(); const manifest = JSON.parse(f.files.get(manifestPath));
    const contract = JSON.parse(f.files.get(contractPath));
    if (producerChange) {
      const indexPath = 'scripts/desktop-core/release-asset-index.json';
      const index = JSON.parse(f.files.get(indexPath));
      contract.producer.runAttempt += 1; index.producer.runAttempt += 1;
      const indexRaw = Buffer.from(JSON.stringify(index) + '\n');
      contract.assetIndex.bytes = indexRaw.length; contract.assetIndex.sha256 = hash(indexRaw);
      writeFileSync(join(f.directory, indexPath), indexRaw);
    }
    const raw = Buffer.from(JSON.stringify(contract) + '\n');
    manifest.desktopRelease.contractSha256 = hash(raw);
    writeFileSync(join(f.directory, contractPath), raw);
    writeFileSync(join(f.directory, manifestPath), JSON.stringify(manifest));
    const result = f.run({ POLARIS_SKIP_QUALITY_GATES: '1', POLARIS_NO_KERNEL_RUN: '1' });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /Formal consumer source review must be renewed: exact reviewed contract differs/);
  }
});
