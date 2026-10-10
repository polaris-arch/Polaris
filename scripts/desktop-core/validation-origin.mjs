// Narrow technical-validation bridge. Original receipts are never rewritten.
// Hashes bind reviewed bytes; API transport records are not signed provenance.
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { canonical, digest, requireGraph, DESKTOP_TARGETS, verifyHash } from './source-graph.mjs';

export const VALIDATION_SCHEMA = 'polaris-desktop-validation-bundle-v2';
export const ORIGIN_POLICY = 'scripts/desktop-core/validation-origin.json';
const read = (path) => JSON.parse(readFileSync(path, 'utf8'));
const equal = (a, b) => canonical(a) === canonical(b);
const sha = (s) => /^[a-f0-9]{64}$/.test(s ?? '');
const commit = (s) => /^[a-f0-9]{40}$/.test(s ?? '');
const positive = (s) => /^[1-9][0-9]*$/.test(String(s ?? ''));

export function validationContext(candidate, env = process.env) {
  requireGraph(env.POLARIS_NATIVE_PAYLOAD_VALIDATION === '1' && env.GITHUB_ACTIONS === 'true'
    && env.POLARIS_NO_KERNEL_RUN !== '1' && env.GITHUB_EVENT_NAME === 'workflow_dispatch'
    && env.GITHUB_REPOSITORY === 'polaris-arch/Polaris'
    && env.GITHUB_REF === 'refs/heads/collab/fk02-native-payload-validation-20261010'
    && env.GITHUB_SHA === candidate && commit(candidate)
    && env.GITHUB_WORKFLOW_REF === `${env.GITHUB_REPOSITORY}/.github/workflows/release-risk.yml@${env.GITHUB_REF}`
    && positive(env.GITHUB_RUN_ID) && positive(env.GITHUB_RUN_ATTEMPT),
  'Validation origin requires exact non-tag manual Risk candidate/run context');
  return { runId: String(env.GITHUB_RUN_ID), attempt: String(env.GITHUB_RUN_ATTEMPT) };
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
    requireGraph(inventory.schema === 'polaris-desktop-bundle-v1' && receipt.candidate === candidate,
      'Producer candidate differs');
    return;
  }
  const context = validationContext(candidate, env), { policy, policySha256 } = loadOriginPolicy(root);
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
  const context = validationContext(candidate, env), { policy, policySha256 } = loadOriginPolicy(root);
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
