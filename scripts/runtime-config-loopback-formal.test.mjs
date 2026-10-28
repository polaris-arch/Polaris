import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { chmodSync, existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { portableCanonical } from './desktop-core/release-consumer.mjs';
import { FORMAL_PROFILE, FORMAL_SCHEMA, FORMAL_TEST_SCHEMA, loopbackInput, formalFile, selectFormalHelper,
  validateFormalIdentity, verifyFormalFiles, prepareFormalCarrier } from './runtime-config-loopback-formal.mjs';
import { TESTS, validatePreparedSource, validateTestEvidence, withResultEvidence } from './runtime-config-loopback.mjs';
const sha = (value) => createHash('sha256').update(value).digest('hex');
const root = fileURLToPath(new URL('../', import.meta.url));
// Deliberately synthetic unit-test objects. No final tag, Release, binary or receipt is asserted.
function fixture() {
  const contract = { schema: 'polaris-upstream-native-release-consumer-v1', repository: 'polaris-arch/sing-box-polaris', repositoryId: 42, materialTransport: 'committed-index-github-source-v1',
    sourceCommit: 'b'.repeat(40), sourceTree: 'c'.repeat(40), version: 'fixture', tag: 'fixture-tag', tagObject: 'd'.repeat(40), releaseId: 42,
    assetIndex: { sha256: '1'.repeat(64), bytes: 7 }, platforms: { linux: {
      core: { bytes: 11, sha256: '2'.repeat(64) }, sidecar: { bytes: 13, sha256: '3'.repeat(64) },
      tags: 'with_gvisor,with_naive_outbound', goVersion: 'fixture-go', buildID: 'fixture-build', releaseAsset: { name: 'fixture-asset', assetId: 7, bytes: 31, url: 'https://fixture.invalid/asset', sha256: '7'.repeat(64) } } } };
  const release = Object.fromEntries(['repository', 'version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId'].map((key) => [key, contract[key]]));
  const manifest = { desktopRelease: { ...release, contractPath: 'scripts/desktop-core/release-contract.json', contractSha256: '4'.repeat(64) } };
  const snapshot = { repositoryId: 42, releaseId: contract.releaseId, tag: contract.tagObject, sourceCommit: contract.sourceCommit, sourceTree: contract.sourceTree, sourceGitlinks: [],
    assets: [{ name: 'fixture-asset', id: 7, bytes: 31, url: 'https://fixture.invalid/asset', digest: 'sha256:' + '7'.repeat(64) }] };
  const context = { candidate: 'a'.repeat(40), appTree: 'f'.repeat(40) };
  const receipt = { schema: 'polaris-app-release-consumption-v1', platform: 'linux',
    signaturePolicy: 'preserve measured original bytes; ad-hoc/not-signed observations do not assert trust', macCodeSignature: null,
    actualAppAcceptance: 'pending native load, gvisor/mixed, feature and final carrier gates', contractSha256: '4'.repeat(64), assetIndexSha256: '1'.repeat(64),
    adapter: { schema: 'polaris-app-release-adapter-v1', facts: { candidate: 'a'.repeat(40) } }, producer: structuredClone(contract.platforms.linux), releaseAsset: { name: 'fixture-asset', assetId: 7, bytes: 31, url: 'https://fixture.invalid/asset', sha256: '7'.repeat(64) }, publicReadback: snapshot };
  receipt.adapter.fingerprint = sha(portableCanonical(receipt.adapter.facts));
  const repair = () => { const { fingerprint, schema, ...facts } = receipt; receipt.fingerprint = sha(portableCanonical(facts)); };
  repair();
  const readback = { schema: 'polaris-app-release-readback-v1', contractSha256: '4'.repeat(64), before: snapshot, after: structuredClone(snapshot) };
  const installation = { schema: 'polaris-core-manifest-v1', id: '5'.repeat(64), core: { helperSha256: '6'.repeat(64) } };
  const binding = { schema: FORMAL_SCHEMA, profile: FORMAL_PROFILE, inputMode: 'formal-release', sourceVerified: true, publicationEligible: false,
    implementationManifestDigest: null, appCommit: 'a'.repeat(40), appTree: 'f'.repeat(40), kernelCommit: contract.sourceCommit, kernelTree: contract.sourceTree,
    protoSha256: '0'.repeat(64), release, inputs: { contract: { sha256: '4'.repeat(64) }, assetIndex: { sha256: '1'.repeat(64), bytes: 7 } },
    consumptionFingerprint: receipt.fingerprint, binary: structuredClone(contract.platforms.linux.core), sidecar: structuredClone(contract.platforms.linux.sidecar),
    build: { goVersion: 'fixture-go', buildID: 'fixture-build', tags: 'with_gvisor,with_naive_outbound' }, naiveIncluded: true,
    helper: { sha256: '6'.repeat(64) }, installedCoreId: installation.id };
  return { binding, manifest, contract, receipt, readback, installation, repair,
    check: () => validateFormalIdentity(binding, manifest, contract, receipt, readback, installation, context) };
}
test('explicit mode is strict, with only absent undefined defaulting to diagnostic', () => {
  assert.equal(loopbackInput({}), 'diagnostic-source');
  for (const mode of ['diagnostic-source', 'formal-release']) assert.equal(loopbackInput({ runtime_config_loopback_input: mode }), mode);
  for (const mode of ['', null, false, 0, {}, [], 'formal', 'diagnostic-source ', 'DIAGNOSTIC-SOURCE', 'FORMAL-RELEASE', 'unknown'])
    assert.throws(() => loopbackInput({ runtime_config_loopback_input: mode }));
});
test('formal binding uses original receipt and measured feature facts, not diagnostic relabelling', () => {
  const value = fixture(); assert.equal(value.check(), value.binding);
  value.contract.platforms.linux.tags = 'with_gvisor'; value.receipt.producer.tags = 'with_gvisor'; value.binding.build.tags = 'with_gvisor';
  value.binding.naiveIncluded = false; value.repair(); value.binding.consumptionFingerprint = value.receipt.fingerprint;
  assert.equal(value.check(), value.binding);
});
const mutations = {
  schema: (x) => { x.binding.schema = 'polaris-runtime-loopback-candidate-v1'; },
  profile: (x) => { x.binding.profile = 'linux-amd64-purego-no-naive'; },
  mode: (x) => { x.binding.inputMode = 'diagnostic-source'; },
  sourceVerified: (x) => { x.binding.sourceVerified = false; },
  publication: (x) => { x.binding.publicationEligible = true; },
  fabricatedManifest: (x) => { x.binding.implementationManifestDigest = 'e'.repeat(64); },
  app: (x) => { x.binding.appCommit = 'f'.repeat(40); },
  appTree: (x) => { x.binding.appTree = 'e'.repeat(40); },
  source: (x) => { x.binding.kernelCommit = 'f'.repeat(40); },
  sourceTree: (x) => { x.binding.kernelTree = 'f'.repeat(40); },
  proto: (x) => { x.binding.protoSha256 = ''; },
  contract: (x) => { x.binding.inputs.contract.sha256 = 'f'.repeat(64); },
  indexHash: (x) => { x.binding.inputs.assetIndex.sha256 = 'f'.repeat(64); },
  indexBytes: (x) => { x.binding.inputs.assetIndex.bytes++; },
  temporary: (x) => { x.manifest.desktopTemporary = {}; },
  pending: (x) => { x.manifest.desktopRelease.contractPath = 'scripts/desktop-core/pending-release-contract.json'; },
  foreignKernel: (x) => { x.contract.repository = 'another/kernel'; },
  historicalSchema: (x) => { x.contract.schema = 'polaris-box-release-consumer-contract-v1'; },
  adapterSchema: (x) => { x.receipt.adapter.schema = 'wrong'; x.repair(); x.binding.consumptionFingerprint = x.receipt.fingerprint; },
  adapterFingerprint: (x) => { x.receipt.adapter.fingerprint = '0'.repeat(64); x.repair(); x.binding.consumptionFingerprint = x.receipt.fingerprint; },
  receiptSchema: (x) => { x.receipt.schema = 'polaris-runtime-loopback-candidate-v1'; },
  signaturePolicy: (x) => { x.receipt.signaturePolicy = 'trusted'; x.repair(); x.binding.consumptionFingerprint = x.receipt.fingerprint; },
  macCodeSignature: (x) => { x.receipt.macCodeSignature = { trustValidated: true }; x.repair(); x.binding.consumptionFingerprint = x.receipt.fingerprint; },
  relabelledAcceptance: (x) => { x.receipt.actualAppAcceptance = 'accepted'; x.repair(); x.binding.consumptionFingerprint = x.receipt.fingerprint; },
  receiptFingerprint: (x) => { x.receipt.fingerprint = 'f'.repeat(64); },
  bindingFingerprint: (x) => { x.binding.consumptionFingerprint = 'f'.repeat(64); },
  producer: (x) => { x.receipt.producer.core.sha256 = 'f'.repeat(64); x.repair(); x.binding.consumptionFingerprint = x.receipt.fingerprint; },
  publicAfter: (x) => { x.readback.after = { fixture: false }; },
  publicBinding: (x) => { x.readback.contractSha256 = 'f'.repeat(64); },
  helper: (x) => { x.binding.helper.sha256 = 'f'.repeat(64); },
  installationId: (x) => { x.binding.installedCoreId = 'f'.repeat(64); },
  naive: (x) => { x.binding.naiveIncluded = false; },
};
for (const key of ['signaturePolicy', 'macCodeSignature', 'actualAppAcceptance'])
  mutations['missing-' + key] = (x) => { delete x.receipt[key]; x.repair(); x.binding.consumptionFingerprint = x.receipt.fingerprint; };
for (const key of ['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId', 'repository'])
  mutations[`release-${key}`] = (x) => { x.binding.release[key] = 'wrong'; };
for (const key of ['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId'])
  mutations[`selection-${key}`] = (x) => { x.manifest.desktopRelease[key] = 'wrong'; };
for (const payload of ['binary', 'sidecar']) for (const key of ['bytes', 'sha256'])
  mutations[`${payload}-${key}`] = (x) => { x.binding[payload][key] = key === 'bytes' ? 999 : 'f'.repeat(64); };
for (const key of ['tags', 'goVersion', 'buildID']) mutations[`build-${key}`] = (x) => { x.binding.build[key] = 'wrong'; };
for (const [name, mutate] of Object.entries(mutations)) test(`formal binding rejects ${name}`, () => { const x = fixture(); mutate(x); assert.throws(x.check); });

test('formal filesystem pins reject redirect, hardlink, unsafe mode, empty, oversize, byte and mode drift', () => {
  const directory = mkdtempSync(join(tmpdir(), 'formal-hot-unit-'));
  try {
    const path = join(directory, 'input'); writeFileSync(path, 'fixture', { mode: 0o600 }); const pin = formalFile(path);
    const binding = { inputs: { input: pin }, retainedInputs: {}, binary: pin, sidecar: pin, helper: pin, observedBuildInfo: pin };
    assert.doesNotThrow(() => verifyFormalFiles(binding));
    const { mode, ...missingMode } = pin;
    assert.throws(() => verifyFormalFiles({ ...binding, helper: missingMode }));
    writeFileSync(path, 'mutated'); assert.throws(() => verifyFormalFiles(binding)); writeFileSync(path, 'fixture');
    chmodSync(path, 0o644); assert.throws(() => verifyFormalFiles(binding)); chmodSync(path, 0o600);
    assert.throws(() => formalFile(path, 2));
    symlinkSync(path, join(directory, 'link')); assert.throws(() => formalFile(join(directory, 'link')));
    linkSync(path, join(directory, 'hard')); assert.throws(() => formalFile(path)); rmSync(join(directory, 'hard'));
    chmodSync(path, 0o622); assert.throws(() => formalFile(path)); chmodSync(path, 0o600);
    writeFileSync(path, ''); assert.throws(() => formalFile(path));
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
test('helper selection is exact current package/bin and private Cargo output, never a runnable test', () => {
  const root = '/fixture/app', target = '/fixture/private-target';
  const binary = { name: 'polaris-helper', kind: ['bin'], src_path: root + '/crates/helper/src/main.rs' };
  const metadata = { target_directory: target, packages: [{ id: 'fixture-id', name: 'polaris-helper', manifest_path: root + '/crates/helper/Cargo.toml', targets: [binary] }] };
  const artifact = { reason: 'compiler-artifact', package_id: 'fixture-id', target: binary, profile: { test: false }, executable: target + '/debug/polaris-helper' };
  assert.equal(selectFormalHelper([artifact], metadata, root, target), artifact.executable);
  for (const change of [{ executable: '/other/helper' }, { package_id: 'other' }, { profile: { test: true } }, { target: { ...binary, kind: ['lib'] } }])
    assert.throws(() => selectFormalHelper([{ ...artifact, ...change }], metadata, root, target));
  assert.throws(() => selectFormalHelper([artifact, artifact], metadata, root, target));
  assert.throws(() => selectFormalHelper([], metadata, root, target));
});
test('formal evidence is distinct, binding-specific, capability-enabled and owned-child terminal', () => {
  const { binding } = fixture(), digest = '9'.repeat(64);
  const value = { schema: FORMAL_TEST_SCHEMA, test: TESTS[0].split('::').at(-1), passed: true, profile: FORMAL_PROFILE,
    publicationEligible: false, appCommit: binding.appCommit, kernelCommit: binding.kernelCommit, kernelTree: binding.kernelTree,
    coreSha256: binding.binary.sha256, inputSha256: digest, contractSha256: binding.inputs.contract.sha256, consumptionFingerprint: binding.consumptionFingerprint,
    events: [{ phase: 'started', pid: 42, capabilities: { enabled: true, contractVersion: 1, implementationManifestDigest: '8'.repeat(64) } },
      { phase: 'stopped', pid: 42, ownedChildReaped: true }] };
  assert.equal(validateTestEvidence(value, TESTS[0], binding, digest), '8'.repeat(64));
  for (const mutation of [{ schema: 'polaris-runtime-loopback-test-v1' }, { inputSha256: '0'.repeat(64) }, { contractSha256: '0'.repeat(64) },
    { consumptionFingerprint: '0'.repeat(64) }, { events: [] }, { events: [value.events[0]] }, { events: [value.events[0], { ...value.events[1], ownedChildReaped: false }] },
    { events: [value.events[1], value.events[0]] },
    ...[undefined, null, 0, -1, 1.5].map((pid) => ({ events: value.events.map((row) => ({ ...row, pid })) })),
    { events: [{ ...value.events[0], capabilities: { ...value.events[0].capabilities, enabled: false } }, value.events[1]] }])
    assert.throws(() => validateTestEvidence({ ...value, ...mutation }, TESTS[0], binding, digest));
});
test('formal post-verification failure cannot replace the original failure or produce a pass', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'formal-finalizer-unit-')), result = { status: 'failed' };
  try {
    await assert.rejects(withResultEvidence(directory, result, async () => { throw Error('original'); }, async () => { throw Error('after'); }), /original/);
    const saved = JSON.parse(readFileSync(join(directory, 'result.json')));
    assert.equal(saved.status, 'failed'); assert.equal(saved.failure, 'original'); assert.equal(saved.finalVerificationFailure, 'after');
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
test('formal wiring retains existing full verifier, actual carrier identity and original evidence', () => {
  const module = readFileSync(join(root, 'scripts/runtime-config-loopback-formal.mjs'), 'utf8');
  const runner = readFileSync(join(root, 'scripts/runtime-config-loopback.mjs'), 'utf8');
  const rust = readFileSync(join(root, 'src-tauri/src/runtime/proxy/tests/runtime_config_real_core.rs'), 'utf8');
  assert.match(module, /verifyPackagedRelease\(root, manifest, 'linux', binary, run, 'linux'\)/);
  assert.match(module, /COPYFILE_EXCL/); assert.match(module, /--mode=verify/);
  assert.match(runner, /await consumeRelease\(ROOT, formal.manifest, \['linux'\]\)/);
  assert.match(runner, /formalInputAfter = verifyFormalInput/);
  assert.match(runner, /\.\.\.\(!formal \? \{ POLARIS_SINGBOX_PATH: binary \} : \{\}\)/);
  assert.match(rust, /selected\.helper_sha256\.as_deref\(\)/);
  assert.match(rust, /formal_bundle\.as_ref\(\)\.unwrap\(\)\.is_current\(\)/);
  assert.match(rust, /option_env!\("GITHUB_SHA"\)/);
  assert.match(rust, /resolve_core_binary\(\)[\s\S]*?canonicalize\(\)/);
  for (const source of [runner, module]) assert.doesNotMatch(source, /sudo |netns |gh release|--no-sandbox/);
});


test('prepared App tree is anchored to measured source, not a format-valid admission declaration', () => {
  const source = { commit: 'a'.repeat(40), tree: 'b'.repeat(40) };
  const admission = { appCommit: source.commit, appTree: source.tree, appSource: source };
  assert.equal(validatePreparedSource(admission, source), source.tree);
  assert.throws(() => validatePreparedSource({ ...admission, appTree: 'c'.repeat(40) }, source));
});
for (const key of ['repositoryId', 'releaseId', 'tag', 'sourceCommit', 'sourceTree', 'assets'])
  test(`self-consistent forged readback cannot replace compiled public ${key}`, () => {
    const x = fixture();
    x.readback.before[key] = key === 'assets' ? [] : 'wrong';
    x.readback.after = structuredClone(x.readback.before);
    x.receipt.publicReadback = structuredClone(x.readback.before);
    x.repair(); x.binding.consumptionFingerprint = x.receipt.fingerprint;
    assert.throws(x.check);
  });


test('formal readers retain bounded fd identity and parse/hash one receipt observation', () => {
  const source = readFileSync(join(root, 'scripts/runtime-config-loopback-formal.mjs'), 'utf8');
  const rust = readFileSync(join(root, 'src-tauri/src/runtime/proxy/tests/runtime_config_real_core.rs'), 'utf8');
  assert.match(source, /openSync\(path, constants.O_RDONLY \| constants.O_NOFOLLOW \| constants.O_NONBLOCK\)/);
  assert.match(source, /fstatSync\(fd, \{ bigint: true \}\)/);
  assert.match(source, /Math.min\(64 \* 1024, limit \+ 1 - count\)/);
  assert.match(source, /finally \{ closeSync\(fd\); \}/);
  assert.match(rust, /nix::libc::O_NOFOLLOW \| nix::libc::O_NONBLOCK/);
  assert.match(rust, /take\(limit.checked_add\(1\).unwrap\(\)\)/);
  assert.match(rust, /let raw_receipt = formal_bytes\(&receipt_path, 1024 \* 1024\)\.0;/);
  assert.doesNotMatch(rust, /let raw_receipt = if/);
  assert.match(rust, /serde_json::from_slice\(&raw_receipt\)/);
  assert.match(rust, /consumption\["signaturePolicy"\]/);
  assert.match(rust, /consumption\.get\("macCodeSignature"\), Some\(&Value::Null\)/);
  assert.match(rust, /consumption\["actualAppAcceptance"\]/);
  assert.match(rust, /formal.then\(\|\| polaris_updater::sha256_hex\(&raw_receipt\)\)/);
  assert.match(rust, /consumption\["fingerprint"\],\s*formal_fingerprint\(&facts\)/);
  assert.match(rust, /formal_public_snapshot\(&readback\["before"\], &contract\)/);
  assert.match(rust, /admission.verify_binary\(\);\n        \}\n        rt.inject_real_core_for_test/);
  assert.match(rust, /admission.verify_binary\(\);\n        \}\n        let status = tokio::time::timeout/);
});

for (const links of [1, 2, 3]) test(`only Cargo-selected helper source allows up to two links (${links}) and installs a single-link byte copy`, async () => {
  // Synthetic inert bytes and mocked command observations; no helper executes.
  const root = mkdtempSync(join(tmpdir(), 'formal-helper-unit-')), target = join(root, 'private-target');
  try {
    mkdirSync(join(target, 'debug/deps'), { recursive: true }); mkdirSync(join(root, 'resources/linux'), { recursive: true });
    const source = join(target, 'debug/polaris-helper'), destination = join(root, 'resources/linux/polaris-helper');
    writeFileSync(source, 'inert helper fixture', { mode: 0o755 });
    for (let n = 1; n < links; n++) linkSync(source, join(target, `debug/deps/fixture-${n}`));
    const binary = { name: 'polaris-helper', kind: ['bin'], src_path: root + '/crates/helper/src/main.rs' };
    const metadata = { target_directory: target, packages: [{ id: 'fixture-id', name: 'polaris-helper', manifest_path: root + '/crates/helper/Cargo.toml', targets: [binary] }] };
    const artifact = { reason: 'compiler-artifact', package_id: 'fixture-id', target: binary, profile: { test: false }, executable: source };
    const calls = [];
    const execute = async (label, command, args) => {
      calls.push({ label, command, args });
      if (label === 'formal-helper-build') return { stdout: JSON.stringify(artifact) + '\n' };
      assert.equal(label, 'formal-identity-prepare'); assert.equal(command, 'node');
      writeFileSync(join(root, 'resources/linux/core-manifest.json'), 'inert installation fixture', { mode: 0o600 });
    };
    if (links > 2) {
      await assert.rejects(prepareFormalCarrier(root, target, metadata, execute, {}), /Bounded owned regular/);
      assert.equal(existsSync(destination), false); assert.equal(calls.length, 1);
    } else {
      const prepared = await prepareFormalCarrier(root, target, metadata, execute, {});
      assert.equal(readFileSync(destination, 'utf8'), 'inert helper fixture');
      assert.equal(statSync(destination).nlink, 1); assert.notEqual(statSync(destination).ino, statSync(source).ino);
      assert.equal(prepared.installed.sha256, prepared.built.sha256); assert.equal(prepared.installed.mode, 0o755);
      assert.equal(calls.length, 2); assert.equal(calls[0].command, 'cargo');
      assert.deepEqual(calls[0].args, ['build', '--locked', '-p', 'polaris-helper', '--bin', 'polaris-helper', '--message-format=json']);
      await assert.rejects(prepareFormalCarrier(root, target, metadata, execute, {}), /EEXIST/);
      assert.equal(readFileSync(destination, 'utf8'), 'inert helper fixture');
      linkSync(destination, join(root, 'installed-alias')); assert.throws(() => formalFile(destination), /Bounded owned regular/);
      if (links === 2) assert.throws(() => formalFile(source), /Bounded owned regular/);
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});
test('the helper link exception stays private and copies bounded observed bytes exclusively', () => {
  const source = readFileSync(join(root, 'scripts/runtime-config-loopback-formal.mjs'), 'utf8');
  assert.match(source, /function readFormal\(path, limit = 512 \* 1024 \* 1024, maxLinks = 1n\)/);
  assert.match(source, /export const formalFile = \(path, limit\) => readFormal\(path, limit\)\.pin/);
  assert.equal((source.match(/readFormal\(source, undefined, 2n\)/g) ?? []).length, 2);
  assert.match(source, /writeFileSync\(destination, observed.bytes, \{ flag: 'wx', mode: before.mode \}\)/);
  assert.doesNotMatch(source, /copyFileSync\(source, destination/);
});

test('every formal binding/core verification callsite retains bounded fd reads; diagnostic readers stay unchanged', () => {
  const runner = readFileSync(join(root, 'scripts/runtime-config-loopback.mjs'), 'utf8');
  const rust = readFileSync(join(root, 'src-tauri/src/runtime/proxy/tests/runtime_config_real_core.rs'), 'utf8');
  assert.match(runner, /result.formalInputFile = formalFile\(receiptPath, 1024 \* 1024\)/);
  assert.match(runner, /const pinCore = formal \? formalFile : filePin;/);
  const shared = runner.split('    const pinCore = formal ? formalFile : filePin;')[1].split('    result.appSourceAfterTests')[0];
  assert.equal((shared.match(/pinCore\(binary\)\.sha256/g) ?? []).length, 4);
  assert.doesNotMatch(shared, /filePin\(binary\)|filePin\(receiptPath\)/);
  assert.match(shared, /JSON.stringify\(formalFile\(receiptPath, 1024 \* 1024\)\) === JSON.stringify\(result.formalInputFile\)/);
  assert.match(runner, /JSON.stringify\(formalFile\(result.formalInputFile.path, 1024 \* 1024\)\) === JSON.stringify\(result.formalInputFile\)/);
  const verification = rust.split('    fn verify_binary(&self) {')[1].split('    fn capabilities(')[0];
  assert.match(verification, /let bytes = if self.binding_sha256.is_some\(\) \{\s*formal_bytes\(&self.binary, 256 \* 1024 \* 1024\)\.0\s*\} else \{\s*regular\(&self.binary, 256 \* 1024 \* 1024\)\s*\};/);
});
