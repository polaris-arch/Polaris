#!/usr/bin/env node
// Linux-only final-carrier custody. This is not a signature, an installation
// test, or permission to substitute diagnostic payloads for formal releases.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, lstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { formalGateContext } from '../desktop-core/native-dns-gates.mjs';
import { canonical } from '../desktop-core/source-graph.mjs';

export const SCHEMA = 'polaris-linux-package-acceptance-v1';
export const KINDS = ['AppImage', 'deb'];
export const CASES = ['default_dashboard', 'gui_selection', 'gui_on_off',
  'gui_manual_disconnect_preference', 'gui_auto_preserves_existing', 'gui_apply_tcp_dns',
  'gui_durability_status_retry', 'gui_rejected_preserves_old', 'rpc_contract',
  'same_home_restart', 'tray_quit', 'cleanup'];
const demand = (ok, message) => { if (!ok) throw Error(message); };
export const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
const read = (path) => JSON.parse(readFileSync(path, 'utf8'));
const same = (a, b, message) => demand(canonical(a) === canonical(b), message);
const hex = (s, n = 64) => typeof s === 'string' && new RegExp(`^[a-f0-9]{${n}}$`).test(s);
const positive = (s) => /^[1-9][0-9]*$/.test(String(s ?? '')) && Number.isSafeInteger(Number(s));
const stamp = (facts) => ({ ...facts, fingerprint: sha256(canonical(facts)) });
function unstamped(value, schema) {
  const { fingerprint, ...facts } = value ?? {};
  demand(facts.schema === schema && hex(fingerprint) && fingerprint === sha256(canonical(facts)), 'Evidence fingerprint/schema differs');
  return facts;
}
function write(path, value) {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  writeFileSync(path, JSON.stringify(value, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
}
function regular(path) {
  const absolute = resolve(path);
  for (let current = absolute;; current = dirname(current)) {
    const s = lstatSync(current); demand(!s.isSymbolicLink(), 'Linked carrier/evidence path');
    demand(current === absolute ? s.isFile() : s.isDirectory(), 'Non-regular carrier/evidence path');
    if (dirname(current) === current) break;
  }
  return absolute;
}
export function selection(env = process.env) {
  demand(env.GITHUB_WORKFLOW_REF === `polaris-arch/Polaris/.github/workflows/package.yml@${env.GITHUB_REF}`,
    'Direct genuine Package caller required');
  const tag = /^refs\/tags\/v[0-9][A-Za-z0-9.+-]*$/.test(env.GITHUB_REF ?? '');
  const manual = env.GITHUB_EVENT_NAME === 'workflow_dispatch' && env.PACKAGE_ACCEPTANCE_REQUESTED === 'true';
  demand(tag && ['push', 'workflow_dispatch'].includes(env.GITHUB_EVENT_NAME) || manual, 'Package acceptance was not selected');
  demand(['linux', 'all'].includes(env.PACKAGE_ACCEPTANCE_PLATFORM || 'all')
    && !['true', '1'].includes(env.PACKAGE_ACCEPTANCE_LIGHT)
    && !['true', '1'].includes(env.PACKAGE_ACCEPTANCE_NATIVE)
    && !['true', '1'].includes(env.PACKAGE_ACCEPTANCE_SKIP_QUALITY)
    && !env.PACKAGE_ACCEPTANCE_PLATFORMS && !env.PACKAGE_ACCEPTANCE_BUNDLE,
  'Full-weight standalone Linux package required');
  if (!tag) demand(/^refs\/heads\/[A-Za-z0-9_./-]+$/.test(env.GITHUB_REF ?? '')
    && hex(env.PACKAGE_ACCEPTANCE_CANDIDATE, 40)
    && env.PACKAGE_ACCEPTANCE_CANDIDATE === env.GITHUB_SHA,
  'Manual acceptance requires the real reviewed branch and exact explicit SHA');
  if (env.PACKAGE_ACCEPTANCE_CANDIDATE) demand(env.PACKAGE_ACCEPTANCE_CANDIDATE === env.GITHUB_SHA, 'Explicit candidate differs');
  return { release: tag, manual };
}
export function admit(root, env = process.env, run = execFileSync) {
  selection(env);
  return formalGateContext(root, env, run); // Legacy 0 AND 1 remain rejected here.
}
function sourceFacts(root, env, run) {
  const options = { cwd: root, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024, env: { ...env } };
  for (const key of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE']) delete options.env[key];
  const git = (...args) => String(run('git', args, options));
  demand(git('rev-parse', 'HEAD').trim() === env.GITHUB_SHA && !git('status', '--porcelain', '--untracked-files=no').trim(),
    'Producer checkout differs or tracked source is dirty');
  const manifest = read(join(root, 'src-tauri/core-manifest.json'));
  demand(manifest.desktopRelease?.contractPath === 'scripts/desktop-core/release-contract.json', 'Canonical formal contract required');
  for (const path of ['src-tauri/core-manifest.json', manifest.desktopRelease.contractPath])
    demand(git('show', `HEAD:${path}`) === readFileSync(join(root, path), 'utf8'), 'Producer selection is not committed bytes');
  demand(sha256(readFileSync(join(root, manifest.desktopRelease.contractPath))) === manifest.desktopRelease.contractSha256,
    'Producer contract digest differs');
  return { tree: git('rev-parse', 'HEAD^{tree}').trim(), contractSha256: manifest.desktopRelease.contractSha256 };
}
export function producerContext(root, admitted, env = process.env, run = execFileSync) {
  // Existing package job declares legacy=0. We never erase it or call it formal
  // admission: setup's clean context is the source binding, consumer re-admits.
  selection(env);
  demand(env.POLARIS_NATIVE_PAYLOAD_VALIDATION === '0' && env.POLARIS_LIGHT_BUILD === '0'
    && env.POLARIS_UPLOAD_ARTIFACTS === '1' && env.POLARIS_RUN_KERNEL_GATES === '1'
    && env.CORE_CANDIDATE === env.GITHUB_SHA
    && !Object.hasOwn(env, 'POLARIS_NATIVE_DNS_FORMAL') && !Object.hasOwn(env, 'POLARIS_NATIVE_DNS_CANDIDATE')
    && !Object.hasOwn(env, 'POLARIS_SINGBOX_PATH') && env.POLARIS_NO_KERNEL_RUN !== '1', 'Producer environment differs');
  demand(admitted?.mode === 'formal-default' && admitted.candidate === env.GITHUB_SHA
    && admitted.repository === env.GITHUB_REPOSITORY && admitted.repositoryId === env.GITHUB_REPOSITORY_ID
    && admitted.workflowRef === env.GITHUB_WORKFLOW_REF && admitted.ref === env.GITHUB_REF
    && admitted.runId === env.GITHUB_RUN_ID && admitted.attempt === env.GITHUB_RUN_ATTEMPT,
  'Producer differs from setup source identity');
  const facts = sourceFacts(root, env, run);
  demand(hex(facts.tree, 40) && facts.tree === admitted.tree && facts.contractSha256 === admitted.contractSha256,
    'Producer tree/contract differs');
  return admitted;
}
export function validateCarriers(carriers) {
  demand(Array.isArray(carriers) && carriers.length === 2, 'Both final Linux carriers required');
  same(carriers.map((r) => r.kind).sort(), KINDS, 'Carrier kinds differ');
  demand(new Set(carriers.map((r) => r.name)).size === 2, 'Duplicate carrier basename');
  for (const row of carriers) {
    demand(/^Polaris_[0-9][A-Za-z0-9.+-]*_amd64-linux\.(deb|AppImage)$/.test(row.name ?? '')
      && row.name.endsWith(`.${row.kind}`) && hex(row.sha256) && Number.isSafeInteger(row.bytes)
      && row.bytes > 0 && row.bytes <= 512 * 1024 ** 2, 'Invalid frozen carrier');
    same(Object.keys(row.members ?? {}).sort(), ['app', 'core', 'cronet'], 'Carrier member roles differ');
    const paths = [];
    for (const member of Object.values(row.members)) {
      demand(typeof member.path === 'string' && !member.path.startsWith('/')
        && member.path.split('/').every((p) => p && p !== '.' && p !== '..')
        && !member.path.includes('\\') && hex(member.sha256)
        && Number.isSafeInteger(member.bytes) && member.bytes > 0, 'Invalid carrier member identity');
      paths.push(member.path);
    }
    demand(new Set(paths).size === 3, 'Duplicate member roles');
  }
  return carriers;
}
export function verifyFreeze(freeze, context) {
  unstamped(freeze, 'polaris-linux-carrier-freeze-v1');
  same(freeze.context, context, 'Frozen source/run/attempt differs');
  demand(freeze.scope === 'producer-byte-record-not-acceptance' && freeze.producerLegacyFlag === '0', 'Freeze is not admission');
  validateCarriers(freeze.carriers);
  return freeze;
}
export function artifactName(kind, context) {
  demand(['transport', 'accepted'].includes(kind), 'Unknown evidence role');
  return `linux-package-${kind}-${context.candidate}-${context.runId}-${context.attempt}`;
}
export function verifyMetadata(api, context, expected) {
  demand(positive(expected.id) && hex(expected.sha256) && api?.id === Number(expected.id)
    && api.name === expected.name && api.digest === `sha256:${expected.sha256}` && api.expired === false
    && api.workflow_run?.id === Number(context.runId) && api.workflow_run.head_sha === context.candidate
    && api.workflow_run.repository_id === Number(context.repositoryId)
    && api.workflow_run.head_repository_id === Number(context.repositoryId)
    && Number.isSafeInteger(api.size_in_bytes) && api.size_in_bytes > 0 && api.size_in_bytes <= 1024 ** 3,
  'Exact current-run artifact ID/name/digest/head/repository differs');
  return api;
}
export function apiJson(path, env, run) {
  return JSON.parse(run('gh', ['api', path], { env, encoding: 'utf8', maxBuffer: 8 * 1024 ** 2, timeout: 60_000 }));
}
export function checkRun(api, context, env) {
  demand(api.id === Number(context.runId) && api.run_attempt === Number(context.attempt)
    && api.head_sha === context.candidate && api.repository?.id === Number(context.repositoryId)
    && api.head_repository?.id === Number(context.repositoryId) && api.event === env.GITHUB_EVENT_NAME
    && api.path === '.github/workflows/package.yml' && api.status === 'in_progress' && api.conclusion === null,
  'Actual Package run/attempt/caller differs');
}
export function download(root, expected, context, destination, env, run) {
  const api = verifyMetadata(apiJson(`/repos/${context.repository}/actions/artifacts/${expected.id}`, env, run), context, expected);
  const scratch = mkdtempSync(join(dirname(resolve(destination)), '.package-download-'));
  try {
    const zip = join(scratch, 'artifact.zip'), fd = openSync(zip, 'wx', 0o600);
    try { run('gh', ['api', `/repos/${context.repository}/actions/artifacts/${api.id}/zip`],
      { env, stdio: ['ignore', fd, 'pipe'], timeout: 300_000, maxBuffer: 1024 ** 2 }); } finally { closeSync(fd); }
    run('python3', ['-B', join(root, 'scripts/package-acceptance/transport_archive.py'), zip, destination],
      { input: JSON.stringify({ bytes: api.size_in_bytes, sha256: expected.sha256 }), encoding: 'utf8', timeout: 300_000 });
    return api;
  } finally { rmSync(scratch, { recursive: true, force: true }); }
}
function files(directory) {
  demand(lstatSync(directory).isDirectory() && !lstatSync(directory).isSymbolicLink(), 'Invalid evidence directory');
  return readdirSync(directory).flatMap((name) => {
    const path = join(directory, name), st = lstatSync(path);
    demand(!st.isSymbolicLink(), 'Linked evidence member');
    if (st.isDirectory()) return files(path);
    demand(st.isFile(), 'Special evidence member'); return [path];
  });
}
export function verifyCarrierFiles(directory, carriers, only = false) {
  validateCarriers(carriers);
  const paths = files(directory);
  if (only) demand(paths.length === 2, 'Unexpected package transport members');
  for (const row of carriers) {
    const found = paths.filter((path) => basename(path) === row.name);
    demand(found.length === 1, 'Missing/duplicate frozen package');
    const path = regular(found[0]);
    demand(lstatSync(path).size === row.bytes && sha256(readFileSync(path)) === row.sha256, 'Final package bytes drifted');
  }
}
function runtimeIdentity(value) {
  return value && hex(value.effectiveDigest) && typeof value.incarnation === 'string'
    && value.incarnation.length > 0 && Number.isSafeInteger(value.appliedRevision) && value.appliedRevision >= 0;
}
export function runtimeBytes(value) {
  demand(value && hex(value.sha256) && positive(value.bytes) && positive(value.pid) && positive(value.startTime)
    && value.stableDoubleRead === true && value.sameInodeAndMetadata === true
    && value.dashboardEnabled === true && value.dashboardDetourMatchesFinal === true
    && /^[1-9][0-9]+$/.test(value.readStartedUnixNs) && /^[1-9][0-9]+$/.test(value.readFinishedUnixNs)
    && BigInt(value.readFinishedUnixNs) >= BigInt(value.readStartedUnixNs)
    && value.originalBytesRetention === 'private ephemeral runner only; not uploaded or cross-VM archived',
  'Stable private original runtime byte proof missing');
}
function stream(value, backend) {
  demand(value?.backend === backend && positive(value.bytes) && hex(value.nonceSha256), 'Real loopback stream evidence missing');
}
export function verifyCaseEvidence(row) {
  const evidence = (key) => row.cases[key].evidence;
  const d = evidence('default_dashboard');
  demand(d.emptyHome === true && d.observed === true && d.defaultGraphics === true, 'Empty HOME default not observed');
  runtimeBytes(d.actualGeneratedRuntime);
  const s = evidence('gui_selection'); demand(s.selected === 'node-b', 'GUI selection missing'); stream(s.before, 'A'); stream(s.newConnection, 'B');
  const m = evidence('gui_manual_disconnect_preference');
  demand(m.interruptPreferenceObserved === true && m.oldBClosed === true, 'Manual disconnect policy not observed'); stream(m.gracefulOldConnection, 'A');
  const o = evidence('gui_on_off'); demand(o.selected === 'node-b' && o.newProcess === true, 'ON/OFF selection/process missing'); stream(o.restoredConnection, 'B');
  const a = evidence('gui_auto_preserves_existing');
  demand(a.nativeIntervalMinutes === 5 && a.manuallyRequestedSwitch === false && positive(a.keepaliveExchanges), 'Native Auto not observed');
  stream(a.before, 'A'); stream(a.oldAfter, 'A'); stream(a.newConnection, 'B');
  const h = evidence('gui_apply_tcp_dns');
  demand(h.appResult?.status === 'hotApplied' && h.appResult.transactionId
    && h.appResult.transactionId === h.operation?.transactionId && h.operation.durable === true
    && h.operation.persistenceMode === 'exact_configuration_file' && runtimeIdentity(h.operation.active)
    && h.answerTtl === 0, 'Real GUI HOT transaction/DNS proof missing');
  stream(h.oldBefore, 'A'); stream(h.oldAfter, 'A'); stream(h.newAfter, 'B'); runtimeBytes(h.acceptedConfig);
  const p = evidence('gui_durability_status_retry');
  demand(p.pending?.status === 'hotDurabilityPending' && p.recovered?.status === 'hotApplied'
    && p.pending.transactionId && p.pending.transactionId === p.recovered.transactionId
    && p.recovered.transactionId === p.operation?.transactionId && p.operation.durable === true
    && p.readOnlyStatusPolls === 3 && hex(p.originalSha256) && hex(p.disturbedSha256)
    && p.originalSha256 !== p.disturbedSha256 && p.terminalRetry?.durable === true
    && runtimeIdentity(p.operation.active) && p.terminalRetry.transactionId === p.pending.transactionId,
  'Same-transaction GUI durability recovery proof missing');
  same(p.operation.active, p.terminalRetry.active, 'Terminal retry changed graph'); runtimeBytes(p.acceptedConfig);
  const j = evidence('gui_rejected_preserves_old');
  demand(j.structuralResult?.status === 'restartRequired' && j.explicitRestartDeclined === true
    && j.rejectedAppResult?.status === 'rejected' && !j.rejectedAppResult.transactionId
    && j.sourceUnchanged === true && runtimeIdentity(j.acceptedIdentity), 'Real rejected/declined candidate preservation missing'); stream(j.oldConnection, 'B');
  const r = evidence('same_home_restart');
  demand(r.sameHome === true && ['node-a', 'node-b'].includes(r.selected), 'Same-HOME desired state not restored');
  runtimeBytes(r.acceptedBefore); runtimeBytes(r.restoredConfig); stream(r.restoredDataPlane, 'B');
  const c = evidence('rpc_contract');
  same(c.methods?.slice().sort(), ['GetRuntimeCapabilities', 'PlanRuntimeConfig', 'ApplyRuntimeConfig',
    'GetRuntimeConfigStatus', 'RetryRuntimePersistence', 'AcknowledgeRuntimePersistence'].sort(), 'Six real RPC methods missing');
  demand(c.source === 'independent authenticated sideclient, not GUI' && c.unauthenticatedPerMethod === 2
    && c.sameTransactionReplay === true && c.cliRegenerationAckRejected === true
    && c.wrongDigestReplayRejected === true && c.stalePlanRejected === true
    && runtimeIdentity(c.operation?.active) && c.terminalRetry?.durable === true
    && c.operation.transactionId && c.operation.transactionId === c.terminalRetry.transactionId,
  'Real authenticated RPC negative/replay evidence missing');
  same(c.noOpOperation?.active, c.before?.active, 'No-op changed graph');
  demand(runtimeIdentity(c.noOpOperation?.active), 'No-op original identity missing');
  same(c.noOpOperation.before, c.noOpOperation.active, 'No-op before/after differs');
  same(c.operation.active, c.terminalRetry.active, 'RPC terminal retry changed graph');
  same(c.operation.active, c.after?.active, 'RPC negatives changed graph');
  const q = evidence('tray_quit');
  demand(q.entry === 'native WebDriver Ctrl+Q -> original tray_quit' && q.normalProductExit === true
    && q.cleanExitMarker?.name === 'clean-exit.marker' && q.cleanExitMarker.bytes === 0
    && q.cleanExitMarker.sha256 === sha256('') && q.cleanExitMarker.createdAfterShortcut === true
    && /^[1-9][0-9]+$/.test(q.cleanExitMarker.mtimeUnixNs)
    && q.cleanExitMarker.writer === 'original desktop exit commit' && q.identities?.length === 2,
  'Original normal exit commit missing');
  same(q.identities, [row.execution.app, row.execution.core].map(({pid, startTime}) => ({pid, startTime})), 'Quit did not identify executed App/core');
  same(evidence('cleanup'), row.cleanup, 'Cleanup case differs from process supervisor');
}

export function verifyRunEvidence(report, freeze) {
  demand(report?.schema === SCHEMA && report.result === 'success', 'No successful real-package run');
  same(report.carriers?.map((r) => r.kind).sort(), KINDS, 'Missing executed carrier');
  demand(report.carriers.length === 2 && report.cleanup?.state === 'normal'
    && Array.isArray(report.cleanup.forced) && report.cleanup.forced.length === 0
    && Array.isArray(report.cleanup.remaining) && report.cleanup.remaining.length === 0,
  'Cleanup is forced, unknown or incomplete');
  for (const row of report.carriers) {
    const frozen = freeze.carriers.find((r) => r.kind === row.kind);
    demand(row.sha256 === frozen.sha256 && row.result === 'success', 'Ran different or failed carrier');
    demand(row.cleanup?.state === 'normal' && row.cleanup.forced?.length === 0
      && row.cleanup.remaining?.length === 0 && row.cleanup.unknown?.length === 0, 'Carrier cleanup is incomplete or unknown');
    same(Object.keys(row.cases ?? {}).sort(), [...CASES].sort(), 'Mandatory GUI/RPC case set differs');
    for (const [name, result] of Object.entries(row.cases)) demand(result?.status === 'passed'
      && result.evidence && typeof result.evidence === 'object' && Object.keys(result.evidence).length > 0, `Missing executed case evidence: ${name}`);
    verifyCaseEvidence(row);
    demand(Number.isInteger(row.execution?.uid) && row.execution.uid > 0 && row.execution.defaultSandbox === true
      && row.execution.originalEntry === true && row.execution.automation === 'tauri-driver 2.0.5'
      && row.execution.app?.sha256 === frozen.members.app.sha256
      && row.execution.core?.sha256 === frozen.members.core.sha256
      && row.execution.cronet?.sha256 === frozen.members.cronet.sha256
      && ['mapped-exact-packaged-library', 'not-mapped-in-socks-scenario'].includes(row.execution.cronet.mapState)
      && positive(row.execution.app?.pid) && positive(row.execution.core?.pid)
      && positive(row.execution.app?.startTime) && positive(row.execution.core?.startTime),
    'Actual default-sandbox App/core/Cronet execution identity missing');
  }
}
export function verifyAccepted(value, context) {
  unstamped(value, SCHEMA);
  const freeze = verifyFreeze(value.freeze, context);
  demand(value.result === 'success' && value.scope === 'linux-nonroot-unpacked-no-tun', 'No bounded success receipt');
  verifyRunEvidence(value.run, freeze);
  demand(hex(value.transportSha256) && value.run.transportSha256 === value.transportSha256, 'Transport proof binding missing');
  for (const row of freeze.carriers) {
    const native = value.native?.[row.kind];
    demand(hex(native?.sha256) && native.coreSha256 === row.members.core.sha256
      && native.cronetSha256 === row.members.cronet.sha256 && native.actualCoreStarted === true
      && native.actualCoreMappedSha256 === row.members.cronet.sha256 && native.capiEngineInitialized === true,
    'Original actual core/Cronet native proof binding missing');
  }
  demand(positive(value.carrierArtifact?.id) && hex(value.carrierArtifact.sha256), 'Exact original carrier artifact missing');
  return value;
}
export function verifyNativeProof(proof, row, context) {
  demand(proof?.schema === 'polaris-final-package-proof-v1' && proof.artifact?.kind === row.kind
    && proof.artifact.sha256 === row.sha256 && proof.artifact.bytes === row.bytes
    && proof.relativeCore === row.members.core.path && proof.relativeLibrary === row.members.cronet.path
    && proof.transport?.candidate === context.candidate && proof.transport.runId === context.runId
    && proof.transport.attempt === context.attempt && proof.native?.schema === 'polaris-native-payload-proof-v1'
    && proof.native.input?.core?.sha256 === row.members.core.sha256
    && proof.native.input?.library?.sha256 === row.members.cronet.sha256
    && proof.native.actualCore?.started === true
    && proof.native.actualCore.mappedLibrary?.sha256 === row.members.cronet.sha256
    && proof.native.capi?.engineInitialized === true
    && proof.native.capi.librarySha256 === row.members.cronet.sha256,
  'Original final-package actual core/Cronet proof differs');
}
export function verifyRemoteAssets(assets, carriers) {
  demand(Array.isArray(assets), 'Remote asset inventory missing');
  for (const row of carriers) {
    const actual = assets.filter((a) => a.name === row.name);
    demand(actual.length === 1 && actual[0].state === 'uploaded' && actual[0].size === row.bytes
      && actual[0].digest === `sha256:${row.sha256}`, 'Remote Linux asset differs from accepted producer bytes');
  }
}
function expectSingle(directory, name) {
  const found = files(directory); demand(found.length === 1 && basename(found[0]) === name, 'Evidence artifact inventory differs');
  return read(found[0]);
}
function main() {
  const [command, ...args] = process.argv.slice(2), root = process.cwd(), env = process.env, run = execFileSync;
  if (command === 'admit') {
    demand(args.length === 0, 'admit takes no arguments');
    const context = admit(root, env, run); console.log(JSON.stringify(context)); return;
  }
  if (command === 'freeze') {
    demand(args.length === 3, 'freeze requires deb AppImage output');
    const context = producerContext(root, JSON.parse(env.PACKAGE_ACCEPTANCE_CONTEXT || 'null'), env, run);
    const inspection = JSON.parse(run('python3', ['-B', 'scripts/package-acceptance/linux.py', 'inspect', '--deb', args[0], '--appimage', args[1]],
      { cwd: root, encoding: 'utf8', timeout: 300_000, maxBuffer: 1024 ** 2 }));
    validateCarriers(inspection.carriers);
    write(args[2], stamp({ schema: 'polaris-linux-carrier-freeze-v1', scope: 'producer-byte-record-not-acceptance',
      producerLegacyFlag: '0', context, carriers: inspection.carriers })); return;
  }
  if (command === 'seal') {
    demand(args.length === 2, 'seal requires freeze output');
    const context = producerContext(root, JSON.parse(env.PACKAGE_ACCEPTANCE_CONTEXT || 'null'), env, run);
    const freeze = verifyFreeze(read(args[0]), context);
    demand(env.PACKAGE_UPLOAD_OUTCOME === 'success', 'Carrier upload did not succeed this attempt');
    const artifact = { id: env.PACKAGE_ARTIFACT_ID, sha256: env.PACKAGE_ARTIFACT_DIGEST,
      name: `polaris-linux-${env.BUILD_LABEL}` };
    verifyMetadata(apiJson(`/repos/${context.repository}/actions/artifacts/${artifact.id}`, env, run), context, artifact);
    write(args[1], stamp({ schema: 'polaris-linux-package-transport-v1', freeze, artifact })); return;
  }
  const context = admit(root, env, run);
  checkRun(apiJson(`/repos/${context.repository}/actions/runs/${context.runId}`, env, run), context, env);
  if (command === 'fetch') {
    demand(args.length === 2, 'fetch requires evidence packages destinations');
    const response = apiJson(`/repos/${context.repository}/actions/runs/${context.runId}/artifacts?per_page=100`, env, run);
    demand(response.total_count <= 100, 'Artifact list truncated');
    const matches = response.artifacts.filter((a) => a.name === artifactName('transport', context));
    demand(matches.length === 1, 'Missing/duplicate current-attempt package index; start a new full run');
    const api = matches[0], expected = { id: String(api.id), name: api.name, sha256: api.digest?.replace(/^sha256:/, '') };
    mkdirSync(dirname(resolve(args[0])), { recursive: true });
    download(root, expected, context, args[0], env, run);
    const transport = expectSingle(args[0], 'transport.json');
    unstamped(transport, 'polaris-linux-package-transport-v1'); verifyFreeze(transport.freeze, context);
    download(root, transport.artifact, context, args[1], env, run);
    verifyCarrierFiles(args[1], transport.freeze.carriers, true); return;
  }
  if (command === 'accept') {
    demand(args.length === 4, 'accept requires transport run native-directory output');
    const transport = read(args[0]); unstamped(transport, 'polaris-linux-package-transport-v1');
    verifyFreeze(transport.freeze, context); const report = read(args[1]); verifyRunEvidence(report, transport.freeze);
    demand(report.transportSha256 === sha256(readFileSync(args[0])), 'Executed transport differs');
    const native = {};
    for (const row of transport.freeze.carriers) {
      const path = join(args[2], `final-${row.kind}.json`), proof = read(path);
      verifyNativeProof(proof, row, context);
      native[row.kind] = { sha256: sha256(readFileSync(path)), coreSha256: proof.native.input.core.sha256,
        cronetSha256: proof.native.input.library.sha256, actualCoreStarted: proof.native.actualCore.started,
        actualCoreMappedSha256: proof.native.actualCore.mappedLibrary.sha256,
        capiEngineInitialized: proof.native.capi.engineInitialized };
    }
    write(args[3], stamp({ schema: SCHEMA, result: 'success', scope: 'linux-nonroot-unpacked-no-tun',
      freeze: transport.freeze, carrierArtifact: transport.artifact, transportSha256: sha256(readFileSync(args[0])),
      native, run: report })); return;
  }
  if (command === 'release-check') {
    demand(args.length === 2 && selection(env).release && env.PACKAGE_ACCEPTANCE_RESULT === 'success', 'Successful tag acceptance required');
    const destination = args[0], expected = { id: env.PACKAGE_ACCEPTED_ARTIFACT_ID, sha256: env.PACKAGE_ACCEPTED_ARTIFACT_DIGEST,
      name: artifactName('accepted', context) };
    download(root, expected, context, destination, env, run);
    const accepted = verifyAccepted(expectSingle(destination, 'accepted.json'), context);
    verifyCarrierFiles(args[1], accepted.freeze.carriers);
    if (env.PACKAGE_CHECK_REMOTE === 'true') {
      const identity = JSON.parse(run('gh', ['release', 'view', env.GITHUB_REF_NAME, '--repo', context.repository,
        '--json', 'apiUrl,isDraft,tagName'], { env, encoding: 'utf8', timeout: 60_000 }));
      demand(identity.isDraft === true && identity.tagName === env.GITHUB_REF_NAME
        && new RegExp(`^https://api\\.github\\.com/repos/${context.repository}/releases/[1-9][0-9]*$`).test(identity.apiUrl),
      'Actual draft API identity required');
      const release = apiJson(identity.apiUrl, env, run);
      demand(release.tag_name === env.GITHUB_REF_NAME && release.draft === true, 'Same draft required');
      verifyRemoteAssets(release.assets, accepted.freeze.carriers);
    }
    return;
  }
  throw Error('Unknown package acceptance command');
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
