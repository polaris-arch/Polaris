// Formal input to the existing four loopback tests. Not a producer or release authority.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, closeSync, constants, copyFileSync, fstatSync, lstatSync, mkdirSync, openSync, readSync, realpathSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { loadFormalGateSelection } from './desktop-core/native-dns-gates.mjs';
import { portableCanonical, verifyPackagedRelease } from './desktop-core/release-consumer.mjs';
import { contractPublicAssets, validatePublishedUpstreamReadback } from './desktop-core/upstream-release.mjs';
export const FORMAL_PROFILE = 'linux-amd64-formal-release';
export const FORMAL_SCHEMA = 'polaris-runtime-loopback-formal-input-v1';
export const FORMAL_TEST_SCHEMA = 'polaris-runtime-loopback-formal-test-v1';
const demand = (ok, message) => { if (!ok) throw Error(message); };
const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
const same = (a, b, message) => demand(portableCanonical(a) === portableCanonical(b), message);
const hex = (value, length) => typeof value === 'string' && new RegExp(`^[0-9a-f]{${length}}$`).test(value);
const JSON_LIMIT = 8 * 1024 * 1024;
const names = Object.freeze({ manifest: 'manifest.json', contract: 'contract.json', assetIndex: 'asset-index.json',
  consumption: 'consumption.json', readback: 'public-readback.json', installation: 'installation.json' });
export function loopbackInput(inputs) {
  const mode = inputs.runtime_config_loopback_input === undefined ? 'diagnostic-source' : inputs.runtime_config_loopback_input;
  demand(['diagnostic-source', 'formal-release'].includes(mode), 'Exact loopback input choice required');
  return mode;
}
function readFormal(path, limit = 512 * 1024 * 1024, maxLinks = 1n) {
  demand(resolve(path) === path && realpathSync(path) === path, 'Canonical formal input required');
  const stamp = (value) => ['dev', 'ino', 'size', 'mtimeNs', 'ctimeNs', 'mode', 'nlink', 'uid'].map((key) => String(value[key])).join(':');
  const initial = lstatSync(path, { bigint: true });
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const before = fstatSync(fd, { bigint: true });
    demand(stamp(initial) === stamp(before) && before.isFile() && before.nlink >= 1n && before.nlink <= maxLinks && before.uid === BigInt(process.getuid())
      && (before.mode & 0o7022n) === 0n && before.size > 0n && before.size <= BigInt(limit), 'Bounded owned regular formal input required');
    const chunks = []; let count = 0;
    while (count <= limit) {
      const chunk = Buffer.alloc(Math.min(64 * 1024, limit + 1 - count));
      const size = readSync(fd, chunk, 0, chunk.length, null);
      if (size === 0) break;
      count += size; demand(count <= limit, 'Formal input exceeded read bound'); chunks.push(chunk.subarray(0, size));
    }
    const after = fstatSync(fd, { bigint: true }), current = lstatSync(path, { bigint: true });
    demand(stamp(before) === stamp(after) && stamp(before) === stamp(current) && realpathSync(path) === path
      && BigInt(count) === before.size, 'Formal input changed or redirected during bounded read');
    const bytes = Buffer.concat(chunks, count);
    return { pin: { path, bytes: count, sha256: sha(bytes), mode: Number(before.mode & 0o7777n) }, bytes };
  } finally { closeSync(fd); }
}
export const formalFile = (path, limit) => readFormal(path, limit).pin;
const json = (path) => JSON.parse(readFormal(path, JSON_LIMIT).bytes);
export function formalSelection(root) {
  const manifest = json(join(root, 'src-tauri/core-manifest.json'));
  const contract = loadFormalGateSelection(root, manifest);
  demand(contract?.repository === 'polaris-arch/sing-box-polaris', 'Only the committed formal Polaris kernel is admissible');
  return { manifest, contract };
}
function paths(root, contract) {
  return { manifest: join(root, 'src-tauri/core-manifest.json'), contract: join(root, 'scripts/desktop-core/release-contract.json'),
    assetIndex: join(root, 'resources/.release-receipts', contract.assetIndex.name),
    consumption: join(root, 'resources/.release-receipts/linux.json'), readback: join(root, 'resources/.release-receipts/public-readback.json'),
    installation: join(root, 'resources/linux/core-manifest.json') };
}
export function selectFormalHelper(messages, metadata, root, target) {
  demand(resolve(metadata.target_directory) === target, 'Formal helper target differs');
  const packages = metadata.packages.filter((p) => p.name === 'polaris-helper' && resolve(p.manifest_path) === join(root, 'crates/helper/Cargo.toml'));
  demand(packages.length === 1, 'Unique current helper package required');
  const targets = packages[0].targets.filter((t) => t.name === 'polaris-helper' && portableCanonical(t.kind) === '["bin"]');
  demand(targets.length === 1, 'Unique current helper binary target required');
  const artifacts = messages.filter((m) => m.reason === 'compiler-artifact' && m.package_id === packages[0].id
    && m.profile?.test === false && m.executable && m.target?.name === targets[0].name
    && portableCanonical(m.target.kind) === '["bin"]' && resolve(m.target.src_path) === resolve(targets[0].src_path));
  demand(artifacts.length === 1 && artifacts[0].executable === join(target, 'debug/polaris-helper'), 'Unique private helper build output required');
  return artifacts[0].executable;
}
export async function prepareFormalCarrier(root, target, metadata, execute, env) {
  const built = await execute('formal-helper-build', 'cargo',
    ['build', '--locked', '-p', 'polaris-helper', '--bin', 'polaris-helper', '--message-format=json'], root, env);
  const messages = built.stdout.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
  const source = selectFormalHelper(messages, metadata, root, target);
  // Cargo links this exact private debug output to its deps artifact. Only this
  // build-source read allows two links; all installed/formal inputs stay single-link.
  const observed = readFormal(source, undefined, 2n), before = observed.pin;
  const directory = join(root, 'resources/linux'), destination = join(directory, 'polaris-helper');
  writeFileSync(destination, observed.bytes, { flag: 'wx', mode: before.mode }); chmodSync(destination, before.mode);
  same(readFormal(source, undefined, 2n).pin, before, 'Helper build output changed during copy');
  same({ ...formalFile(destination), path: source }, before, 'Copied helper bytes/mode differ');
  await execute('formal-identity-prepare', 'node', ['scripts/prepare-core-identity.mjs', '--platform=linux', `--directory=${directory}`], root, env);
  return { built: before, installed: formalFile(destination), installation: formalFile(join(directory, 'core-manifest.json'), JSON_LIMIT) };
}
// Pure binding only: the caller must first run the existing full verifier.
// Unit tests use synthetic fixtures, never acceptance receipts.
export function validateFormalIdentity(binding, manifest, contract, receipt, readback, installation, context) {
  demand(binding?.schema === FORMAL_SCHEMA && binding.profile === FORMAL_PROFILE && binding.inputMode === 'formal-release'
    && binding.sourceVerified === true && binding.publicationEligible === false && binding.implementationManifestDigest === null,
  'Distinct formal HOT input binding required');
  demand(!Object.hasOwn(manifest, 'desktopTemporary') && manifest.desktopRelease?.contractPath === 'scripts/desktop-core/release-contract.json'
    && contract.schema === 'polaris-upstream-native-release-consumer-v1' && contract.repository === 'polaris-arch/sing-box-polaris',
  'Committed canonical Polaris formal selection required');
  for (const key of ['appCommit', 'appTree', 'kernelCommit', 'kernelTree']) demand(hex(binding[key], 40), `Exact formal ${key} required`);
  demand(binding.appCommit === context.candidate && binding.appTree === context.appTree, 'Verified App source identity differs');
  demand(hex(binding.protoSha256, 64) && binding.kernelCommit === contract.sourceCommit && binding.kernelTree === contract.sourceTree,
    'Formal source/protocol attribution differs');
  same(binding.release, Object.fromEntries(['repository', 'version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId']
    .map((key) => [key, contract[key]])), 'Formal release identity differs');
  for (const key of ['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId'])
    same(manifest.desktopRelease[key], contract[key], `Formal selection differs: ${key}`);
  demand(binding.inputs.contract.sha256 === manifest.desktopRelease.contractSha256
    && binding.inputs.assetIndex.sha256 === contract.assetIndex.sha256 && binding.inputs.assetIndex.bytes === contract.assetIndex.bytes,
  'Formal contract/index binding differs');
  const { fingerprint, schema, ...facts } = receipt;
  demand(schema === 'polaris-app-release-consumption-v1' && fingerprint === sha(portableCanonical(facts))
    && fingerprint === binding.consumptionFingerprint && receipt.platform === 'linux'
    && receipt.signaturePolicy === 'preserve measured original bytes; ad-hoc/not-signed observations do not assert trust' && receipt.macCodeSignature === null
    && receipt.actualAppAcceptance === 'pending native load, gvisor/mixed, feature and final carrier gates'
    && receipt.contractSha256 === manifest.desktopRelease.contractSha256 && receipt.assetIndexSha256 === contract.assetIndex.sha256
    && receipt.adapter?.schema === 'polaris-app-release-adapter-v1'
    && receipt.adapter?.facts?.candidate === binding.appCommit
    && receipt.adapter.fingerprint === sha(portableCanonical(receipt.adapter.facts)), 'Original formal consumption attribution differs');
  const row = contract.platforms.linux;
  same(receipt.producer, row, 'Original formal producer differs'); same(receipt.releaseAsset, row.releaseAsset, 'Original formal release asset differs');
  demand(readback.schema === 'polaris-app-release-readback-v1' && readback.contractSha256 === receipt.contractSha256,
    'Original formal public readback differs');
  same(readback.before, readback.after, 'Formal public readback changed'); same(readback.before, receipt.publicReadback, 'Formal public snapshot differs');
  const snapshot = readback.before;
  for (const [key, field] of [['repositoryId', 'repositoryId'], ['releaseId', 'releaseId'], ['tag', 'tagObject'], ['sourceCommit', 'sourceCommit'], ['sourceTree', 'sourceTree']])
    same(snapshot[key], contract[field], `Compiled public identity differs: ${key}`);
  validatePublishedUpstreamReadback(snapshot, contract);
  const pins = contractPublicAssets(contract);
  demand(Array.isArray(snapshot.assets) && snapshot.assets.length === pins.length, 'Exact public asset set required');
  for (const pin of pins) {
    const rows = snapshot.assets.filter((row) => row.name === pin.name);
    demand(rows.length === 1, 'Unique public asset required');
    const row = rows[0];
    same({ name: row.name, id: row.id, bytes: row.bytes, url: row.url },
      { name: pin.name, id: pin.assetId, bytes: pin.bytes, url: pin.url }, 'Compiled public asset identity differs');
    demand(row.digest === null || row.digest === `sha256:${pin.sha256}`, 'Compiled public asset digest differs');
  }

  for (const key of ['binary', 'sidecar']) {
    const expected = row[key === 'binary' ? 'core' : 'sidecar'];
    demand(expected && binding[key]?.bytes === expected.bytes && binding[key]?.sha256 === expected.sha256, `Actual formal ${key} differs`);
  }
  same(binding.build, { goVersion: row.goVersion, buildID: row.buildID, tags: row.tags }, 'Formal build facts differ');
  demand(binding.naiveIncluded === row.tags.split(',').includes('with_naive_outbound'), 'Formal Naive availability differs from admitted build');
  demand(installation.schema === 'polaris-core-manifest-v1' && hex(installation.id, 64)
    && installation.id === binding.installedCoreId && installation.core?.helperSha256 === binding.helper.sha256,
  'Actual formal installation/helper binding differs');
  return binding;
}
function inspect(root, context, evidence, run) {
  const { manifest, contract } = formalSelection(root), binary = join(root, 'resources/linux/sing-box');
  const receipt = verifyPackagedRelease(root, manifest, 'linux', binary, run, 'linux');
  const rawBuildInfo = String(run('go', ['version', '-m', binary], { cwd: root, encoding: 'utf8', timeout: 60_000,
    maxBuffer: JSON_LIMIT, env: { ...process.env, GOTOOLCHAIN: 'local', GOWORK: 'off' } }));
  const originals = paths(root, contract), snapshots = Object.fromEntries(Object.entries(originals).map(([key, path]) => [key, readFormal(path, JSON_LIMIT)]));
  const inputs = Object.fromEntries(Object.entries(snapshots).map(([key, observed]) => [key, observed.pin]));
  const parsed = (key) => JSON.parse(snapshots[key].bytes);
  same(parsed('manifest'), manifest, 'Formal manifest changed during verification');
  same(parsed('contract'), contract, 'Formal contract changed during verification');
  same(parsed('consumption'), receipt, 'Original consumption changed during verification');
  const row = contract.platforms.linux, installation = parsed('installation');
  const binding = { schema: FORMAL_SCHEMA, inputMode: 'formal-release', profile: FORMAL_PROFILE,
    appCommit: context.candidate, appTree: context.appTree, kernelCommit: contract.sourceCommit, kernelTree: contract.sourceTree,
    protoSha256: context.protoSha256, runId: context.runId, runAttempt: context.runAttempt,
    release: Object.fromEntries(['repository', 'version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId'].map((key) => [key, contract[key]])),
    inputs, consumptionFingerprint: receipt.fingerprint, installedCoreId: installation.id,
    binary: formalFile(binary), sidecar: formalFile(join(root, 'resources/linux/libcronet.so')), helper: formalFile(join(root, 'resources/linux/polaris-helper')),
    build: { goVersion: row.goVersion, buildID: row.buildID, tags: row.tags },
    observedBuildInfo: { path: join(evidence, 'formal-build-info.txt'), bytes: Buffer.byteLength(rawBuildInfo), sha256: sha(rawBuildInfo), mode: 0o600 },
    implementationManifestDigest: null, naiveIncluded: row.tags.split(',').includes('with_naive_outbound'), sourceVerified: true, publicationEligible: false };
  validateFormalIdentity(binding, manifest, contract, receipt, parsed('readback'), installation, context);
  return { binding, rawBuildInfo };
}
export function createFormalInput(root, context, evidence, run = execFileSync) {
  const { binding, rawBuildInfo } = inspect(root, context, evidence, run);
  writeFileSync(binding.observedBuildInfo.path, rawBuildInfo, { flag: 'wx', mode: 0o600 });
  same(formalFile(binding.observedBuildInfo.path, JSON_LIMIT), binding.observedBuildInfo, 'Recorded original BuildInfo differs');
  const directory = join(evidence, 'formal-inputs'); mkdirSync(directory, { mode: 0o700 }); binding.retainedInputs = {};
  for (const [key, name] of Object.entries(names)) {
    const original = binding.inputs[key], destination = join(directory, name);
    copyFileSync(original.path, destination, constants.COPYFILE_EXCL); chmodSync(destination, 0o600);
    const copy = formalFile(destination, JSON_LIMIT);
    demand(copy.bytes === original.bytes && copy.sha256 === original.sha256, 'Retained original formal bytes differ'); binding.retainedInputs[key] = copy;
  }
  verifyFormalFiles(binding); return binding;
}
export function verifyFormalFiles(binding) {
  for (const pin of [...Object.values(binding.inputs), ...Object.values(binding.retainedInputs), binding.binary, binding.sidecar, binding.helper])
    same(formalFile(pin.path), pin, 'Formal input bytes/mode changed');
  const actual = formalFile(binding.observedBuildInfo.path, JSON_LIMIT);
  same(actual, binding.observedBuildInfo, 'Original observed BuildInfo changed');
}
export function verifyFormalInput(root, binding, evidence, run = execFileSync) {
  verifyFormalFiles(binding);
  const context = { candidate: binding.appCommit, appTree: binding.appTree, protoSha256: binding.protoSha256, runId: binding.runId, runAttempt: binding.runAttempt };
  const observed = inspect(root, context, evidence, run).binding, { retainedInputs, ...original } = binding;
  same(observed, original, 'Formal input identity changed after execution');
  run('node', ['scripts/prepare-core-identity.mjs', '--platform=linux', `--directory=${join(root, 'resources/linux')}`, '--mode=verify'],
    { cwd: root, encoding: 'utf8', timeout: 5 * 60_000, maxBuffer: JSON_LIMIT });
  verifyFormalFiles(binding); return observed;
}
