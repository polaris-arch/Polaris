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
  assert.doesNotMatch(candidate, /^      always\(\)/m, 'installer legs must stay cancellable');
  assert.match(candidate, /^    if: >-\n      !cancelled\(\) && needs\.setup\.result == 'success'\n      && \(needs\.desktop_core\.result == 'success'\n        \|\| \(needs\.desktop_core\.result == 'skipped' && inputs\.core_bundle_artifact != ''\)\)$/m);
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
const inputPaths = [manifestPath, sourcePath, 'scripts/core-source-provision.py',
  ...source.patches.map((patch) => `scripts/libbox-patches/${patch.file}`),
  ...source.dependencyPatches.map((dep) => `scripts/libbox-patches/${dep.patchFile}`),
  'scripts/core-patches/windows-dns-refresh.patch'];
const inputs = new Map(inputPaths.map((path) => [path, readFileSync(join(root, path))]));
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
function requireUnobserved(policy) {
  assert.equal(policy.policyState, 'SOURCE_FIRST_RELEASE_ELIGIBLE');
  assert.equal(policy.publicationRequirements, 'existing-signed-release-dag');
  assert.equal(policy.nativeAcceptance, 'NotObserved');
  assert.equal(policy.deviceAcceptance, 'NotObserved');
  assert.equal(policy.Exact, 'Unknown');
  assert.equal(policy.NoOwner, 'Unknown');
  assert.equal(policy.managed, 'NotGranted');
}

test('only reviewed source inputs are eligible, without native or ownership clearance', () => {
  const result = runPolicy();
  assert.equal(result.status, 0, result.stderr);
  const policy = JSON.parse(result.stdout);
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
    return fixtureRead(new Map([[sourcePath, bytes], [manifestPath, Buffer.from(JSON.stringify(manifest))]]));
  };
  const patchBytes = (file) => readFileSync(join(root, 'scripts/libbox-patches', file));
  // Unchanged rebinding reaches the reviewed-input digest, so the helper is live.
  assert.throws(() => assertSourceFirstRelease(withSource((changed) => { changed.goVersion = '1.25.6'; })),
    /Source inputs differ from the reviewed release policy/);
  assert.deepEqual([...OWNERSHIP_CHAIN_PATCHES].sort(), ['nftables-owned-transactions.patch',
    'sing-tun-owned-native.patch', 'tun-owner-consumer.patch']);
  assert.throws(() => assertSourceFirstRelease(withSource((changed) => {
    changed.patches.push({ file: 'tun-owner-consumer.patch', sha256: sha(patchBytes('tun-owner-consumer.patch')) });
  })), /Ownership chain policy: retired patch listed/);
  // The retained files stay on disk; each name is refused in the ordered list.
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
      'scripts/desktop-core/source-graph.mjs'];
    for (const path of fixturePaths) {
      mkdirSync(dirname(join(fixture, path)), { recursive: true });
      writeFileSync(join(fixture, path), readFileSync(join(root, path)));
    }
    writeFileSync(join(fixture, 'scripts/core-source-provision.py'), 'changed-source');
    for (const env of [
      { POLARIS_SKIP_PC_RUNTIME_RELEASE_GATE: '1' },
      { POLARIS_ALLOW_PC_RUNTIME_RELEASE: '1', PC_RUNTIME_RELEASE_READY: 'true' },
      { POLARIS_NO_KERNEL_RUN: '1', POLARIS_SKIP_QUALITY_GATES: '1' },
      { HELPER_STOP_ACK: 'stopped', PC_OWNER_STATE: 'NoOwner', GITHUB_REF_NAME: 'v999.999.999' },
    ]) {
      const good = runPolicy([], env);
      assert.equal(good.status, 0, good.stderr);
      requireUnobserved(JSON.parse(good.stdout));
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
