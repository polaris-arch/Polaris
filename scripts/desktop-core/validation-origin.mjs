import { hasTemporaryDesktop, loadTemporaryDesktopSelection, selectedCandidateContext } from './native-dns-candidate.mjs';
// Narrow technical-validation bridge. Original receipts are never rewritten.
// Hashes bind reviewed bytes; API transport records are not signed provenance.
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { hasDesktopRelease, loadReleaseContract } from './release-consumer.mjs';
import { candidateContext } from './native-dns-candidate.mjs';
import { formalGateContext, loadFormalGateSelection } from './native-dns-gates.mjs';
import { canonical, digest, requireGraph, DESKTOP_TARGETS, verifyHash, validateSourcePins, validateSourceManifest, desktopSourceManifestPath, sourceOwnedReplacements, producerContext } from './source-graph.mjs';

export const VALIDATION_SCHEMA = 'polaris-desktop-validation-bundle-v2';
export const ORIGIN_POLICY = 'scripts/desktop-core/validation-origin.json';
const read = (path) => JSON.parse(readFileSync(path, 'utf8'));
const equal = (a, b) => canonical(a) === canonical(b);
const sha = (s) => /^[a-f0-9]{64}$/.test(s ?? '');
const commit = (s) => /^[a-f0-9]{40}$/.test(s ?? '');
const positive = (s) => /^[1-9][0-9]*$/.test(String(s ?? ''));

export function validationContext(candidate, env = process.env, root = process.cwd(), run) {
  const formalRequested = Object.hasOwn(env, 'POLARIS_NATIVE_DNS_FORMAL');
  if (!formalRequested && Object.hasOwn(env, 'POLARIS_NATIVE_DNS_CANDIDATE')) {
    const context = candidateContext(candidate, env, root);
    return { runId: context.runId, attempt: context.attempt };
  }
  const manifest = existsSync(join(root, 'src-tauri/core-manifest.json')) ? read(join(root, 'src-tauri/core-manifest.json')) : null;
  if (!formalRequested && manifest && hasTemporaryDesktop(manifest)) {
    const context = selectedCandidateContext(root, env);
    requireGraph(context.mode === 'temporary-default' && context.candidate === candidate
      && context.attempt && env.POLARIS_NO_KERNEL_RUN !== '1', 'Committed default native proof requires its actual App attempt');
    return context.attempt;
  }
  if (formalRequested || (manifest && loadFormalGateSelection(root, manifest))) {
    // Only the committed strict contract can admit formal proof. The optional
    // Git reader is a unit-test seam; the CLI never accepts an alternate reader.
    const context = formalGateContext(root, env, run);
    requireGraph(context.candidate === candidate, 'Formal native proof App candidate differs');
    return { runId: context.runId, attempt: context.attempt };
  }
  const historicalRef = env.GITHUB_REF === 'refs/heads/collab/fk02-native-payload-validation-20261010';
  // The cumulative S5/UI candidates consume the strict published contract. They do
  // not inherit the historical source/origin exemption on another branch.
  const publishedRef = ['refs/heads/collab/s5-android-signal-20261010', 'refs/heads/collab/ui01-status-20261010'].includes(env.GITHUB_REF)
    && validationSourceMode(root) === 'published-release-v1';
  requireGraph(env.POLARIS_NATIVE_PAYLOAD_VALIDATION === '1' && env.GITHUB_ACTIONS === 'true'
    && env.POLARIS_NO_KERNEL_RUN !== '1' && env.GITHUB_EVENT_NAME === 'workflow_dispatch'
    && env.GITHUB_REPOSITORY === 'polaris-arch/Polaris'
    && (historicalRef || publishedRef)
    && env.GITHUB_SHA === candidate && commit(candidate)
    && env.GITHUB_WORKFLOW_REF === `${env.GITHUB_REPOSITORY}/.github/workflows/release-risk.yml@${env.GITHUB_REF}`
    && positive(env.GITHUB_RUN_ID) && positive(env.GITHUB_RUN_ATTEMPT),
  'Validation origin requires exact non-tag manual Risk candidate/run context');
  return { runId: String(env.GITHUB_RUN_ID), attempt: String(env.GITHUB_RUN_ATTEMPT) };
}

// Package's Bash setup and the Python native/final proof use the same admission
// function as the Node assembler. The contract is read from actual source bytes.
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  requireGraph(process.argv.length === 3, 'Validation context requires exactly one candidate');
  console.log(JSON.stringify(validationContext(process.argv[2])));
}

export function validationSourceMode(root) {
  const manifest = read(join(root, 'src-tauri/core-manifest.json'));
  if (hasTemporaryDesktop(manifest)) { loadTemporaryDesktopSelection(root, manifest); return 'temporary-desktop-default-v1'; }
  if (hasDesktopRelease(manifest)) { loadReleaseContract(root, manifest); return 'published-release-v1'; }
  const spec = validateSourcePins(manifest, 'linux', false);
  const sourcePath = join(root, desktopSourceManifestPath(spec));
  verifyHash(sourcePath, spec.sourceManifestSha256);
  validateSourceManifest(read(sourcePath), spec);
  return sourceOwnedReplacements(spec) ? 'source-owned-all-current-v1' : 'historical-d-origin-v1';
}

export function desktopProducerPlan(root, nativeValidation) {
  requireGraph(typeof nativeValidation === 'boolean', 'Explicit native validation selection required');
  const mode = validationSourceMode(root);
  const legacyOrigins = nativeValidation && mode === 'historical-d-origin-v1';
  const matrix = [
    { platform: 'linux', os: 'ubuntu-22.04', host: 'linux', arch: 'x64' },
    { platform: 'win', os: 'windows-2022', host: 'win32', arch: 'x64' },
    { platform: 'mac-x64', os: 'macos-15-intel', host: 'darwin', arch: 'x64' },
    { platform: 'mac-arm64', os: 'macos-15', host: 'darwin', arch: 'arm64' },
  ];
  return { mode, release: ['published-release-v1', 'temporary-desktop-default-v1'].includes(mode), legacyOrigins, matrix: legacyOrigins ? matrix.filter((row) => row.platform === 'win') : matrix };
}

export function validateCurrentProducer(receipt, candidate, env = process.env) {
  const context = producerContext(candidate, env);
  const { fingerprint, ...facts } = receipt;
  requireGraph(receipt.schema === 'polaris-desktop-core-v1' && receipt.candidate === candidate
    && fingerprint === digest(canonical(facts)) && equal(receipt.producerContext, context),
  'Current native producer context/fingerprint differs');
  return context;
}

export function loadOriginPolicy(root) {
  const bytes = readFileSync(join(root, ORIGIN_POLICY)), policy = JSON.parse(bytes);
  requireGraph(policy.schema === 'polaris-desktop-validation-origin-v1'
    && policy.repository === 'polaris-arch/Polaris' && policy.repositoryId === 1347028220
    && positive(policy.runId) && policy.attempt === 1 && commit(policy.workflowHead) && commit(policy.candidate)
    && equal(Object.keys(policy.platforms ?? {}).sort(), ['linux', 'mac-arm64', 'mac-x64']), 'Invalid fixed validation origin policy');
  for (const [key, pin] of Object.entries(policy.platforms)) {
    requireGraph(Number.isSafeInteger(pin.artifactId) && pin.artifactId > 0
      && pin.artifactName === `desktop-core-producer-${policy.candidate}-${policy.runId}-${policy.attempt}-${key}`
      && ['archiveSha256', 'binarySha256', 'receiptSha256', 'sourceFingerprint', 'platformInputFingerprint', 'buildInfoSha256'].every((k) => sha(pin[k]))
      && pin.buildID === `polaris-desktop-source-v1-${pin.platformInputFingerprint}`,
    'Incomplete origin bytes/input pins');
  }
  return { policy, policySha256: digest(bytes) };
}

export function verifyOriginTransport(root, transport) {
  const { policy } = loadOriginPolicy(root);
  const run = transport?.run;
  requireGraph(transport?.repository === policy.repository && run?.id === policy.runId
    && run.run_attempt === policy.attempt && run.head_sha === policy.workflowHead
    && run.status === 'completed' && run.conclusion === 'success' && run.event === 'workflow_dispatch'
    && run.repository?.id === policy.repositoryId && run.head_repository?.id === policy.repositoryId
    && equal(Object.keys(transport.artifacts ?? {}).sort(), Object.keys(policy.platforms).sort()), 'Original successful native run identity differs');
  for (const [key, pin] of Object.entries(policy.platforms)) {
    const item = transport.artifacts[key], api = item?.api;
    requireGraph(api?.id === pin.artifactId && api.name === pin.artifactName && api.expired === false
      && api.digest === `sha256:${pin.archiveSha256}` && item.archiveSha256 === pin.archiveSha256
      && api.workflow_run?.id === policy.runId && api.workflow_run.head_sha === policy.workflowHead
      && api.workflow_run.repository_id === policy.repositoryId && api.workflow_run.head_repository_id === policy.repositoryId,
    `Exact origin artifact transport differs: ${key}`);
  }
  return policy;
}

export function validateBundleOrigin(root, inventory, candidate, key, receipt, env = process.env) {
  if (inventory.schema !== VALIDATION_SCHEMA) {
    requireGraph(inventory.mode === undefined, 'Unsupported bundle mode');
    if (validationSourceMode(root) === 'source-owned-all-current-v1') validateCurrentProducer(receipt, candidate, env);
    requireGraph(inventory.schema === 'polaris-desktop-bundle-v1' && receipt.candidate === candidate,
      'Producer candidate differs');
    return;
  }
  const context = validationContext(candidate, env, root);
  if (inventory.mode === 'source-owned-all-current-v1') {
    requireGraph(validationSourceMode(root) === inventory.mode && inventory.validationOnly === true
      && inventory.candidate === candidate && equal(inventory.transport, context)
      && equal(Object.keys(inventory.origins ?? {}).sort(), Object.keys(DESKTOP_TARGETS).sort())
      && inventory.policySha256 === undefined && inventory.originTransport === undefined,
    'All-current validation envelope differs');
    const manifest = read(join(root, 'src-tauri/core-manifest.json'));
    const spec = validateSourcePins(manifest, key, false);
    const owned = sourceOwnedReplacements(spec);
    requireGraph(inventory.sourceManifestSha256 === spec.sourceManifestSha256
      && inventory.sourceOwnedReplacementsSha256 === digest(canonical(owned))
      && equal(receipt.sourceOwnedReplacements, owned) && receipt.platform === key,
    'All-current source-owned identity differs');
    const producer = validateCurrentProducer(receipt, candidate, env);
    requireGraph(equal(inventory.origins[key], { kind: 'current-native', ...producer }),
      'All-current origin differs from actual producer context');
    return;
  }
  requireGraph(validationSourceMode(root) === 'historical-d-origin-v1'
    && inventory.mode === undefined && !Object.hasOwn(receipt, 'producerContext')
    && !Object.hasOwn(receipt, 'sourceOwnedReplacements'), 'New source cannot reuse historical origins');
  const { policy, policySha256 } = loadOriginPolicy(root);
  requireGraph(inventory.validationOnly === true && inventory.candidate === candidate
    && inventory.policySha256 === policySha256 && equal(inventory.transport, context)
    && equal(Object.keys(inventory.origins ?? {}).sort(), Object.keys(DESKTOP_TARGETS).sort()), 'Validation consumer envelope differs');
  verifyOriginTransport(root, inventory.originTransport);
  const origin = inventory.origins[key];
  if (key === 'win') {
    requireGraph(equal(origin, { kind: 'current-native', candidate, ...context })
      && receipt.candidate === candidate, 'Windows must be freshly produced for current candidate');
    return;
  }
  const pin = policy.platforms[key];
  requireGraph(equal(origin, { kind: 'reviewed-native-origin', candidate: policy.candidate,
    runId: String(policy.runId), attempt: String(policy.attempt), artifactId: pin.artifactId })
    && receipt.candidate === policy.candidate && receipt.binarySha256 === pin.binarySha256
    && inventory.platforms[key].receiptSha256 === pin.receiptSha256
    && receipt.sourceFingerprint === pin.sourceFingerprint
    && receipt.platformInputFingerprint === pin.platformInputFingerprint && receipt.buildID === pin.buildID
    && receipt.buildInfoSha256 === pin.buildInfoSha256
    && equal(receipt.macCodeSignature ?? null, pin.macCodeSignature), 'Reused original receipt/input/signature pins differ');
}

export function writeValidationBundle(root, directory, candidate, env = process.env) {
  const context = validationContext(candidate, env, root);
  if (validationSourceMode(root) === 'source-owned-all-current-v1') {
    const manifest = read(join(root, 'src-tauri/core-manifest.json'));
    const spec = validateSourcePins(manifest, 'linux', false), owned = sourceOwnedReplacements(spec);
    const platforms = {}, origins = {};
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      const file = join(directory, key, key === 'win' ? 'sing-box.exe' : 'sing-box');
      const receiptFile = `${file}.source-receipt.json`, receipt = read(receiptFile);
      verifyHash(file, receipt.binarySha256);
      platforms[key] = { binarySha256: receipt.binarySha256, receiptSha256: digest(readFileSync(receiptFile)) };
      origins[key] = { kind: 'current-native', ...validateCurrentProducer(receipt, candidate, env) };
    }
    const inventory = { schema: VALIDATION_SCHEMA, mode: 'source-owned-all-current-v1',
      validationOnly: true, candidate, transport: context, sourceManifestSha256: spec.sourceManifestSha256,
      sourceOwnedReplacementsSha256: digest(canonical(owned)), platforms, origins };
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      const file = join(directory, key, key === 'win' ? 'sing-box.exe' : 'sing-box');
      validateBundleOrigin(root, inventory, candidate, key, read(`${file}.source-receipt.json`), env);
    }
    writeFileSync(join(directory, 'bundle.json'), `${JSON.stringify(inventory, null, 2)}\n`);
    return inventory;
  }
  const { policy, policySha256 } = loadOriginPolicy(root);
  const originTransport = read(join(directory, 'origin-transport.json'));
  verifyOriginTransport(root, originTransport);
  const platforms = {}, origins = {};
  for (const key of Object.keys(DESKTOP_TARGETS)) {
    const file = join(directory, key, key === 'win' ? 'sing-box.exe' : 'sing-box');
    const receiptFile = `${file}.source-receipt.json`, receipt = read(receiptFile);
    verifyHash(file, receipt.binarySha256);
    platforms[key] = { binarySha256: receipt.binarySha256, receiptSha256: digest(readFileSync(receiptFile)) };
    origins[key] = key === 'win' ? { kind: 'current-native', candidate, ...context }
      : { kind: 'reviewed-native-origin', candidate: policy.candidate, runId: String(policy.runId),
        attempt: String(policy.attempt), artifactId: policy.platforms[key].artifactId };
  }
  const inventory = { schema: VALIDATION_SCHEMA, validationOnly: true, candidate, ...{ transport: context },
    policySha256, platforms, origins, originTransport };
  for (const key of Object.keys(DESKTOP_TARGETS)) {
    const file = join(directory, key, key === 'win' ? 'sing-box.exe' : 'sing-box');
    validateBundleOrigin(root, inventory, candidate, key, read(`${file}.source-receipt.json`), env);
  }
  writeFileSync(join(directory, 'bundle.json'), `${JSON.stringify(inventory, null, 2)}\n`);
  return inventory;
}
