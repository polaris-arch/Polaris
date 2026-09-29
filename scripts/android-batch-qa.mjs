#!/usr/bin/env node
// Offline plan/receipt gate. No CDP attachment, Activity start, VPN or policy writes.
import assert from 'node:assert/strict';
import { createHash, randomBytes } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { isIP } from 'node:net';
import { isAbsolute, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { DEBUG_APP, RELEASE_APP, assertInstalledAndroidPackage } from './android-cdp.mjs';

export const PHASES = Object.freeze([
  'wifi-positive', 'policy-negative', 'policy-restored', 'cellular-positive', 'wifi-return',
]);
export const DIRECTIONS = Object.freeze(['pc-to-android', 'android-to-pc']);
export const PROTOCOLS = Object.freeze(['tcp', 'udp']);
const HASH = /^[a-f0-9]{64}$/;
const SNAPSHOT_KEYS = [
  'configSha256', 'invariantsSha256', 'identitySha256', 'serverStableSha256',
  'selectedServerSha256', 'policySha256', 'pcStateSha256', 'appPid',
  'runtimePolicySha256', 'perAppVpnSha256', 'observedAtMs',
  'proxyRunning', 'proxyStarting', 'pendingChanges', 'physicalTransport',
  'networkHandleSha256', 'wifiEnabled', 'cellularEnabled', 'airplaneEnabled',
];

export function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === 'object') return Object.fromEntries(
    Object.keys(value).sort().map(key => [key, canonical(value[key])]),
  );
  return value;
}
export const sha256 = value => createHash('sha256').update(value).digest('hex');
export const digest = value => sha256(JSON.stringify(canonical(value)));
function exactKeys(value, keys, label) {
  assert.ok(value && typeof value === 'object' && !Array.isArray(value), `${label}: object required`);
  assert.deepEqual(Object.keys(value).sort(), [...keys].sort(), `${label}: unexpected/missing fields`);
}
function hash(value, label) { assert.match(value, HASH, `${label}: SHA-256 required`); }
function integer(value, min, max, label) {
  assert.ok(Number.isSafeInteger(value) && value >= min && value <= max, `${label}: invalid integer`);
}
export function validateSnapshot(value) {
  exactKeys(value, SNAPSHOT_KEYS, 'snapshot');
  for (const key of SNAPSHOT_KEYS.filter(key => key.endsWith('Sha256'))) hash(value[key], key);
  integer(value.appPid, 1, 2 ** 31 - 1, 'appPid');
  integer(value.observedAtMs, 1, Number.MAX_SAFE_INTEGER, 'snapshot observedAtMs');
  for (const key of ['proxyRunning', 'proxyStarting', 'pendingChanges',
    'wifiEnabled', 'cellularEnabled', 'airplaneEnabled']) assert.equal(typeof value[key], 'boolean', key);
  assert.ok(['wifi', 'cellular'].includes(value.physicalTransport), 'physicalTransport required');
  return value;
}
export function hasPendingChanges(value) {
  exactKeys(value, ['added', 'modified', 'removed', 'restartDeferred'], 'pending changes');
  for (const key of ['added', 'modified', 'removed']) assert.ok(Array.isArray(value[key]), key);
  assert.equal(typeof value.restartDeferred, 'boolean');
  return value.restartDeferred || ['added', 'modified', 'removed'].some(key => value[key].length > 0);
}
/** Project in-memory readings to hashes; never write raw config, credentials or identity. */
export function projectSnapshot({ config, targetId, identity, pcState, status, pendingChanges,
  appPid, physicalTransport, networkHandle, wifiEnabled, cellularEnabled, airplaneEnabled,
  runtimePolicy, perAppVpn, observedAtMs }) {
  assert.ok(Array.isArray(config.servers), 'config.servers required');
  const target = config.servers.find(server => server.id === targetId);
  assert.ok(target && target.protocol === 'tailscale', 'existing Headscale tailscale server required');
  const stable = structuredClone(target);
  delete stable.meshInboundPolicy;
  const invariant = structuredClone(config);
  delete invariant.selectedServerId;
  delete invariant.servers.find(server => server.id === targetId).meshInboundPolicy;
  assert.ok(identity !== undefined && pcState !== undefined && networkHandle !== undefined);
  assert.ok(runtimePolicy !== undefined && perAppVpn !== undefined, 'runtime policy and per-app VPN observations required');
  return validateSnapshot({ configSha256: digest(config), invariantsSha256: digest(invariant),
    identitySha256: digest(identity), serverStableSha256: digest(stable),
    selectedServerSha256: digest(config.selectedServerId ?? null),
    policySha256: digest(target.meshInboundPolicy ?? null), pcStateSha256: digest(pcState),
    runtimePolicySha256: digest(runtimePolicy), perAppVpnSha256: digest(perAppVpn), observedAtMs,
    appPid, proxyRunning: status.running, proxyStarting: status.starting ?? false,
    pendingChanges, physicalTransport, networkHandleSha256: digest(networkHandle),
    wifiEnabled, cellularEnabled, airplaneEnabled });
}
export function validateBuildReceipt(receipt) {
  exactKeys(receipt, ['schemaVersion', 'sourceSha256', 'javacSha256', 'd8Sha256',
    'androidJarSha256', 'classJarSha256', 'dexSha256', 'minApi'], 'witness build receipt');
  assert.equal(receipt.schemaVersion, 1);
  assert.equal(receipt.minApi, 24);
  for (const key of Object.keys(receipt).filter(key => key.endsWith('Sha256'))) hash(receipt[key], key);
  return receipt;
}
export const MISSING_OBSERVERS = Object.freeze([
  'runtime-policy', 'per-app-vpn', 'vpn-underlying-netId-transport',
  'run-as-app-process-MIUI-no-socket', 'nonce-owned-process-listener-cleanup',
  'nonce-owned-counter-artifact-cleanup', 'owned-ADB-forward-cleanup', 'continuous-user-presence',
]);
export function requireLiveExecution(plan) {
  validatePlan(plan);
  throw new Error('NOT_READY: prepared-only; trusted device/runtime/cleanup observers are unavailable');
}
export function privateManifestBytes(plan) {
  validatePlan(plan);
  const { planSha256, ...body } = plan;
  return Buffer.from(JSON.stringify(canonical(body)), 'utf8');
}
export function verifyWitnessBytes(plan, manifestBytes, dexBytes) {
  validatePlan(plan);
  assert.equal(sha256(manifestBytes), plan.planSha256, 'copied private manifest hash mismatch');
  assert.equal(sha256(dexBytes), plan.witnessBuild.dexSha256, 'copied witness DEX hash mismatch');
  return { manifestSha256: plan.planSha256, dexSha256: plan.witnessBuild.dexSha256,
    buildReceiptSha256: plan.witnessBuildReceiptSha256 };
}
export function verifyCopiedHashOutput(plan, output) {
  validatePlan(plan);
  const names = [`cache/polaris-qa-${plan.nonce}/manifest.json`, `cache/polaris-qa-${plan.nonce}/witness.dex.jar`];
  const lines = output.trim().split(/\r?\n/);
  assert.equal(lines.length, 2, 'run-as copy hash output requires exactly manifest and DEX');
  for (let i = 0; i < 2; i++) {
    const match = /^([a-f0-9]{64})\s+([^\s]+)$/.exec(lines[i]);
    assert.ok(match && match[2] === names[i], 'unexpected run-as artifact path/hash');
    assert.equal(match[1], i === 0 ? plan.planSha256 : plan.witnessBuild.dexSha256, 'run-as copied artifact hash mismatch');
  }
  return { verdict: 'NOT_READY', claims: [], copiedHashesMatch: true };
}
export function createPlan(input, nonce = randomBytes(24).toString('hex')) {
  exactKeys(input, ['schemaVersion', 'package', 'serialSha256', 'sourceHead', 'apkSha256',
    'targetServerSha256', 'pcMeshIp', 'androidMeshIp', 'tcpPort', 'udpPort',
    'attempts', 'timeoutMs', 'baseline', 'witnessBuild', 'qaRoot'], 'manifest');
  validateBuildReceipt(input.witnessBuild);
  assert.ok(isAbsolute(input.qaRoot) && resolve(input.qaRoot) === input.qaRoot, 'explicit canonical absolute QA root required');
  assert.equal(input.schemaVersion, 1);
  assert.ok([DEBUG_APP, RELEASE_APP].includes(input.package), 'explicit current package required');
  for (const key of ['serialSha256', 'apkSha256', 'targetServerSha256']) hash(input[key], key);
  assert.match(input.sourceHead, /^[a-f0-9]{40}$/);
  for (const key of ['pcMeshIp', 'androidMeshIp']) {
    // Witness v1 deliberately supports IPv4 only; IPv6 cannot silently be marked covered.
    assert.equal(isIP(input[key]), 4, `${key}: IPv4 literal required for this witness`);
    assert.ok(!/^(?:0\.|127\.|169\.254\.|22[4-9]\.|23\d\.|24\d\.|25[0-5]\.)/.test(input[key]), `${key}: unicast mesh address required`);
  }
  assert.notEqual(input.pcMeshIp, input.androidMeshIp);
  for (const key of ['tcpPort', 'udpPort']) integer(input[key], 1024, 65535, key);
  integer(input.attempts, 2, 20, 'attempts');
  integer(input.timeoutMs, 500, 10000, 'timeoutMs');
  assert.match(nonce, /^[a-f0-9]{48}$/);
  validateSnapshot(input.baseline);
  const b = input.baseline;
  // server_switch also writes backend-owned MRU history, which a generic rollback cannot restore.
  // Require the already selected endpoint rather than pretending only selection would change.
  assert.equal(b.selectedServerSha256, input.targetServerSha256, 'baseline must already select the existing Headscale endpoint; no server_switch/MRU edits');
  assert.equal(b.proxyRunning, false, 'baseline requires an already stopped Android VPN');
  assert.equal(b.proxyStarting, false);
  assert.equal(b.pendingChanges, false);
  assert.equal(b.physicalTransport, 'wifi');
  assert.equal(b.wifiEnabled, true);
  assert.equal(b.cellularEnabled, true, 'mobile data must already be enabled');
  assert.equal(b.airplaneEnabled, false);
  const allowPolicy = { mode: 'allowlist', rules: [{
    sourceCidrs: [`${input.pcMeshIp}/32`], network: 'both',
    ports: [...new Set([input.tcpPort, input.udpPort])].sort((a, b) => a - b).map(String), target: 'local',
  }] };
  const body = { ...structuredClone(input), nonce, allowPolicy,
    allowPolicySha256: digest(allowPolicy), blockPolicySha256: digest({ mode: 'block' }),
    stages: ['baseline', ...PHASES, 'cleanup'],
    witnessBuildReceiptSha256: digest(input.witnessBuild),
    readiness: { level: 'prepared-only', executable: false, missingObservers: [...MISSING_OBSERVERS] },
    scopes: ['device-observe', 'artifact-copy', 'witness', 'traffic', 'android-policy', 'android-vpn', 'wifi-toggle', 'cleanup'],
    scopeGroups: { readOnly: ['device-observe'], mutation: ['artifact-copy', 'witness', 'traffic', 'android-policy', 'android-vpn'],
      wifi: ['wifi-toggle'], cleanup: ['cleanup'] },
    claimsExcluded: ['P5-data-plane', 'P8-handover', 'baseline-restored', 'warp-egress', 'dns-data-plane', 'ipv6', 'direct-vs-relay'],
  };
  return { ...body, planSha256: digest(body) };
}
export function validatePlan(plan) {
  const { planSha256, ...body } = plan;
  hash(planSha256, 'planSha256');
  assert.equal(digest(body), planSha256, 'plan or immutable baseline changed');
  const manifest = Object.fromEntries(['schemaVersion', 'package', 'serialSha256', 'sourceHead',
    'apkSha256', 'targetServerSha256', 'pcMeshIp', 'androidMeshIp', 'tcpPort', 'udpPort',
    'attempts', 'timeoutMs', 'baseline', 'witnessBuild', 'qaRoot'].map(key => [key, plan[key]]));
  assert.deepEqual(plan, createPlan(manifest, plan.nonce), 'unrecognized plan shape');
  return plan;
}
export function requireApproval(plan, approval, scope, now = Date.now()) {
  validatePlan(plan);
  exactKeys(approval, ['schemaVersion', 'planSha256', 'approvedByOperator', 'scopes',
    'issuedAtMs', 'expiresAtMs', 'presenceConfirmedAtMs'], 'approval');
  assert.equal(approval.schemaVersion, 1);
  assert.equal(approval.planSha256, plan.planSha256, 'approval belongs to another plan');
  assert.equal(approval.approvedByOperator, true, 'user approval must be recorded by operator');
  integer(approval.issuedAtMs, 1, Number.MAX_SAFE_INTEGER, 'issuedAtMs');
  integer(approval.expiresAtMs, 1, Number.MAX_SAFE_INTEGER, 'expiresAtMs');
  assert.ok(approval.issuedAtMs <= now && now < approval.expiresAtMs, 'approval not current');
  assert.ok(approval.expiresAtMs - approval.issuedAtMs <= 3_600_000, 'approval window exceeds one hour');
  assert.ok(Array.isArray(approval.scopes) && new Set(approval.scopes).size === approval.scopes.length);
  assert.ok(approval.scopes.every(item => plan.scopes.includes(item)), 'unknown mutation scope');
  assert.ok(approval.scopes.includes(scope), `scope not approved: ${scope}`);
  integer(approval.presenceConfirmedAtMs, 1, Number.MAX_SAFE_INTEGER, 'presenceConfirmedAtMs');
  assert.ok(approval.presenceConfirmedAtMs <= now && now - approval.presenceConfirmedAtMs <= 60000,
    'operator presence checkpoint expired; this self-attestation does not verify user presence');
}
export function challenge(plan, phase, direction, protocol, sequence) {
  assert.ok(PHASES.includes(phase) && DIRECTIONS.includes(direction) && PROTOCOLS.includes(protocol));
  integer(sequence, 0, plan.attempts - 1, 'sequence');
  return `polaris-qa-v1:${plan.nonce}:${phase}:${direction}:${protocol}:${sequence}\n`;
}
export function expectedChallenges(plan, phase, direction, protocol) {
  return Array.from({ length: plan.attempts }, (_, i) => sha256(challenge(plan, phase, direction, protocol, i))).sort();
}
function counter(value, plan, protocol, direction) {
  exactKeys(value, ['instanceSha256', 'nonceSha256', 'protocol', 'port', 'alive',
    'received', 'matched', 'challengeSha256s', 'observedAtMs',
    'dexSha256', 'manifestSha256', 'buildReceiptSha256'], 'witness counter');
  integer(value.observedAtMs, 1, Number.MAX_SAFE_INTEGER, 'counter observedAtMs');
  assert.equal(value.dexSha256, plan.witnessBuild.dexSha256, 'counter witness DEX mismatch');
  assert.equal(value.manifestSha256, plan.planSha256, 'counter private manifest mismatch');
  assert.equal(value.buildReceiptSha256, plan.witnessBuildReceiptSha256, 'counter build receipt mismatch');
  hash(value.instanceSha256, 'instanceSha256');
  assert.equal(value.nonceSha256, sha256(plan.nonce), 'stale witness nonce');
  assert.equal(value.protocol, protocol);
  assert.equal(value.port, plan[`${protocol}Port`]);
  assert.equal(value.alive, true, `${direction}: listener not alive`);
  integer(value.received, 0, Number.MAX_SAFE_INTEGER, 'received');
  integer(value.matched, 0, value.received, 'matched');
  assert.ok(Array.isArray(value.challengeSha256s));
  value.challengeSha256s.forEach(item => hash(item, 'challenge hash'));
  assert.equal(new Set(value.challengeSha256s).size, value.challengeSha256s.length, 'duplicate witness challenge');
  assert.equal(value.matched, value.challengeSha256s.length, 'counter/unique nonce mismatch');
}
function checkProbe(plan, phase, value, beforeSnapshot, afterSnapshot) {
  exactKeys(value, ['direction', 'protocol', 'attempted', 'ackSha256s', 'outcomes',
    'before', 'after', 'loopbackHealth', 'startedAtMs', 'finishedAtMs'], 'probe');
  const { direction, protocol } = value;
  assert.ok(DIRECTIONS.includes(direction) && PROTOCOLS.includes(protocol));
  assert.equal(value.attempted, plan.attempts, 'attempts cannot be skipped');
  assert.ok(Array.isArray(value.outcomes) && value.outcomes.length === plan.attempts);
  assert.ok(Array.isArray(value.ackSha256s));
  assert.equal(typeof value.loopbackHealth, 'boolean', 'loopback health must be a typed observation');
  counter(value.before, plan, protocol, direction);
  counter(value.after, plan, protocol, direction);
  integer(value.startedAtMs, 1, Number.MAX_SAFE_INTEGER, 'probe startedAtMs');
  integer(value.finishedAtMs, 1, Number.MAX_SAFE_INTEGER, 'probe finishedAtMs');
  assert.ok(beforeSnapshot.observedAtMs < value.before.observedAtMs
    && value.before.observedAtMs < value.startedAtMs && value.startedAtMs <= value.finishedAtMs
    && value.finishedAtMs < value.after.observedAtMs && value.after.observedAtMs < afterSnapshot.observedAtMs,
  'probe/counter observations must be bracketed by phase snapshots');
  assert.equal(value.before.instanceSha256, value.after.instanceSha256, 'witness restarted during probe');
  assert.ok(value.before.challengeSha256s.every(item => value.after.challengeSha256s.includes(item)), 'counter rolled back');
  const hashes = expectedChallenges(plan, phase, direction, protocol);
  assert.ok(hashes.every(item => !value.before.challengeSha256s.includes(item)), 'probe already executed/replayed');
  const added = value.after.challengeSha256s.filter(item => !value.before.challengeSha256s.includes(item)).sort();
  const blocked = phase === 'policy-negative' && direction === 'pc-to-android';
  if (blocked) {
    assert.deepEqual(value.ackSha256s, [], 'blocked probe unexpectedly echoed');
    assert.ok(value.outcomes.every(item => ['timeout', 'refused', 'reset'].includes(item)),
      'local/routing errors cannot count as policy rejection');
    assert.deepEqual(added, [], 'blocked listener accepted a current nonce');
    assert.equal(value.after.received, value.before.received, 'blocked packet reached local witness');
    assert.equal(value.loopbackHealth, true, 'negative probe requires a contemporaneous Android loopback echo');
  } else {
    assert.deepEqual([...value.ackSha256s].sort(), hashes, 'fresh exact echoes required');
    assert.ok(value.outcomes.every(item => item === 'echo'));
    assert.deepEqual(added, hashes, 'client ACK needs receiver-side nonce counters');
    assert.equal(value.after.received - value.before.received, plan.attempts, 'unexpected witness traffic');
  }
}
function stableSnapshot(snapshot) {
  const { networkHandleSha256, observedAtMs, ...stable } = snapshot;
  return stable;
}
function withoutObservation(value) {
  const { observedAtMs, ...data } = value;
  return data;
}
function checkCleanup(plan, event) {
  exactKeys(event, ['stage', 'snapshot', 'cleanupEvidence'], 'cleanup');
  assert.equal(event.stage, 'cleanup');
  validateSnapshot(event.snapshot);
  assert.deepEqual(stableSnapshot(event.snapshot), stableSnapshot(plan.baseline), 'restore did not match immutable baseline');
  assert.deepEqual(event.cleanupEvidence, { verdict: 'NOT_OBSERVED',
    missingObservers: plan.readiness.missingObservers }, 'caller booleans cannot certify cleanup');
}
export function assessBatch(plan, receipt) {
  validatePlan(plan);
  exactKeys(receipt, ['schemaVersion', 'planSha256', 'events', 'witness'], 'receipt');
  assert.equal(receipt.schemaVersion, 1);
  assert.equal(receipt.planSha256, plan.planSha256);
  assert.deepEqual(receipt.witness, { dexSha256: plan.witnessBuild.dexSha256,
    manifestSha256: plan.planSha256, buildReceiptSha256: plan.witnessBuildReceiptSha256 }, 'receipt artifact binding mismatch');
  assert.ok(Array.isArray(receipt.events) && receipt.events.length <= plan.stages.length);
  const continuity = new Map();
  let cellularHandle;
  for (let i = 0; i < receipt.events.length; i++) {
    const event = receipt.events[i];
    const stage = plan.stages[i];
    assert.equal(event.stage, stage, 'missing, reordered or duplicated stage; stop batch');
    exactKeys(event, stage === 'cleanup'
      ? ['stage', 'snapshot', 'cleanupEvidence']
      : stage === 'baseline' ? ['stage', 'snapshot'] : ['stage', 'beforeSnapshot', 'afterSnapshot', 'probes'], stage);
    const snapshot = validateSnapshot(stage === 'baseline' || stage === 'cleanup' ? event.snapshot : event.beforeSnapshot);
    if (stage === 'baseline') assert.deepEqual(withoutObservation(snapshot), withoutObservation(plan.baseline), 'baseline drift before first mutation');
    else if (stage === 'cleanup') checkCleanup(plan, event);
    else {
      const afterSnapshot = validateSnapshot(event.afterSnapshot);
      assert.ok(snapshot.observedAtMs < afterSnapshot.observedAtMs, 'phase snapshots reversed');
      assert.deepEqual(withoutObservation(snapshot), withoutObservation(afterSnapshot),
        'phase policy/network/identity/PC/selection/pending drift; abort');
      for (const key of ['invariantsSha256', 'identitySha256', 'serverStableSha256', 'pcStateSha256', 'appPid']) {
        assert.equal(snapshot[key], plan.baseline[key], `${stage}: protected baseline changed: ${key}`);
      }
      assert.equal(snapshot.selectedServerSha256, plan.targetServerSha256);
      assert.equal(snapshot.policySha256, stage === 'policy-negative' ? plan.blockPolicySha256 : plan.allowPolicySha256);
      assert.equal(snapshot.runtimePolicySha256, snapshot.policySha256, 'runtime policy witness differs from saved intent');
      assert.equal(snapshot.perAppVpnSha256, plan.baseline.perAppVpnSha256, 'per-app VPN routing witness changed');
      assert.equal(snapshot.proxyRunning, true);
      assert.equal(snapshot.proxyStarting, false);
      assert.equal(snapshot.pendingChanges, false, 'policy not fully applied');
      assert.equal(snapshot.airplaneEnabled, false);
      assert.equal(snapshot.cellularEnabled, true);
      const cellular = stage === 'cellular-positive';
      assert.equal(snapshot.physicalTransport, cellular ? 'cellular' : 'wifi');
      assert.equal(snapshot.wifiEnabled, !cellular);
      if (cellular) {
        assert.notEqual(snapshot.networkHandleSha256, plan.baseline.networkHandleSha256, 'no physical network transition');
        cellularHandle = snapshot.networkHandleSha256;
      }
      if (stage === 'wifi-return') assert.notEqual(snapshot.networkHandleSha256, cellularHandle, 'cellular network still active');
      assert.ok(Array.isArray(event.probes) && event.probes.length === 4, 'both directions and TCP/UDP required');
      const pairs = new Set();
      let priorAfterTime = snapshot.observedAtMs;
      for (const probe of event.probes) {
        const key = `${probe.direction}/${probe.protocol}`;
        assert.ok(!pairs.has(key), 'duplicate direction/protocol coverage');
        pairs.add(key);
        checkProbe(plan, stage, probe, snapshot, afterSnapshot);
        assert.ok(probe.before.observedAtMs > priorAfterTime, 'phase probes must execute sequentially');
        priorAfterTime = probe.after.observedAtMs;
        if (continuity.has(key)) assert.deepEqual(withoutObservation(probe.before), withoutObservation(continuity.get(key)), 'cross-phase counter discontinuity');
        else {
          assert.equal(probe.before.received, 0, 'fresh witness must start with zero received packets');
          assert.equal(probe.before.matched, 0, 'fresh witness must start with zero matched challenges');
        }
        continuity.set(key, probe.after);
      }
    }
  }
  const complete = receipt.events.length === plan.stages.length;
  return { verdict: 'NOT_READY', preparedOnly: true, structurallyComplete: complete, completedStages: receipt.events.length,
    nextStage: complete ? null : plan.stages[receipt.events.length],
    claims: [], missingObservers: plan.readiness.missingObservers, excluded: plan.claimsExcluded };
}
export function requireNextStage(plan, receipt, stage, approval, scope, now = Date.now()) {
  const result = assessBatch(plan, receipt);
  assert.equal(result.nextStage, stage, 'prior checkpoint failed or requested stage is out of order');
  requireApproval(plan, approval, scope, now);
  requireLiveExecution(plan);
}
export function appendEvent(plan, receipt, event) {
  const next = { ...structuredClone(receipt), events: [...structuredClone(receipt.events), structuredClone(event)] };
  assessBatch(plan, next); // validate before saving or advancing to another external action
  return next;
}
export function abortBatch(plan, lastAcceptedReceipt, cleanup) {
  const last = assessBatch(plan, lastAcceptedReceipt);
  assert.equal(last.structurallyComplete, false, 'completed fixture cannot be retroactively aborted');
  let restored = false;
  try { checkCleanup(plan, cleanup); restored = true; } catch { /* preserve incomplete/failed restore verdict */ }
  return { schemaVersion: 1, planSha256: plan.planSha256,
    verdict: restored ? 'NOT_READY' : 'INVALID_EVIDENCE', restoration: 'NOT_OBSERVED',
    lastAcceptedStage: last.completedStages ? plan.stages[last.completedStages - 1] : null,
    nextStageAtAbort: last.nextStage, claims: [], excluded: plan.claimsExcluded };
}
/** ADB observation only. Caller must explicitly opt in; does not invoke config_get/CDP. */
export function readOnlyPreflight(serial, app, adb) {
  assert.match(serial, /^[A-Za-z0-9_.:-]+$/, 'explicit ADB serial required');
  assert.ok(!serial.startsWith('-'), 'ADB serial cannot be an option');
  assert.equal(adb('get-state').trim(), 'device');
  assertInstalledAndroidPackage(adb, app);
  const pid = adb('shell', 'pidof', app).trim();
  assert.match(pid, /^\d+$/, 'selected app must already be running; preflight never starts it');
  return { package: app, serialSha256: sha256(serial), appPid: Number(pid),
    readiness: 'package/process only; runtime, Headscale, WARP and DNS not verified' };
}
export function writePrivateJson(path, value) {
  // Never overwrite an immutable plan/receipt or follow an existing symlink.
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600, flag: 'wx' });
}
export function main(args) {
  const read = path => JSON.parse(readFileSync(path, 'utf8'));
  if (args[0] === 'build-receipt' && args.length === 8) {
    const keys = ['sourceSha256', 'javacSha256', 'd8Sha256', 'androidJarSha256', 'classJarSha256', 'dexSha256'];
    const receipt = { schemaVersion: 1, minApi: 24,
      ...Object.fromEntries(keys.map((key, index) => [key, sha256(readFileSync(args[index + 1]))])) };
    validateBuildReceipt(receipt);
    writePrivateJson(args[7], receipt);
    return { mode: 'OFFLINE_BUILD_RECEIPT', buildReceiptSha256: digest(receipt), verdict: 'NOT_READY', claims: [] };
  }
  if (args[0] === 'private-manifest' && args.length === 3) {
    const plan = read(args[1]);
    const bytes = privateManifestBytes(plan);
    writeFileSync(args[2], bytes, { mode: 0o444, flag: 'wx' });
    return { mode: 'OFFLINE_PRIVATE_MANIFEST', manifestSha256: sha256(bytes), verdict: 'NOT_READY', claims: [] };
  }
  if (args[0] === 'plan' && args.length === 3) {
    const plan = createPlan(read(args[1]));
    writePrivateJson(args[2], plan);
    return { mode: 'DRY_RUN', planSha256: plan.planSha256, stages: plan.stages,
      verdict: 'NOT_READY', preparedOnly: true, requiredApprovalScopes: plan.scopeGroups,
      missingObservers: plan.readiness.missingObservers, claims: [], excluded: plan.claimsExcluded };
  }
  if (args[0] === 'assess' && args.length === 3) return assessBatch(read(args[1]), read(args[2]));
  if (args[0] === 'record' && args.length === 5) {
    const plan = read(args[1]);
    const next = appendEvent(plan, read(args[2]), read(args[3]));
    writePrivateJson(args[4], next);
    return assessBatch(plan, next);
  }
  if (args[0] === 'abort' && args.length === 5) {
    const result = abortBatch(read(args[1]), read(args[2]), read(args[3]));
    writePrivateJson(args[4], result);
    return result;
  }
  if (args[0] === 'preflight' && args.length === 5) {
    assert.equal(args[4], '--read-device', 'preflight requires explicit --read-device');
    const plan = validatePlan(read(args[1]));
    const approval = read(args[2]);
    requireApproval(plan, approval, 'device-observe');
    assert.equal(sha256(args[3]), plan.serialSha256, 'wrong authorized device');
    const app = plan.package;
    const adbPath = `${process.env.ANDROID_HOME ?? `${process.env.HOME}/Android/Sdk`}/platform-tools/adb`;
    const adb = (...tail) => {
      requireApproval(plan, approval, 'device-observe');
      return execFileSync(adbPath, ['-s', args[3], ...tail], { encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'pipe'] });
    };
    return { ...readOnlyPreflight(args[3], app, adb), verdict: 'NOT_READY', claims: [], missingObservers: plan.readiness.missingObservers };
  }
  throw new Error('Usage: plan INPUT NEW_PLAN | build-receipt SOURCE JAVAC D8 ANDROID_JAR CLASS_JAR DEX_JAR NEW_OUT | private-manifest PLAN NEW_OUT | assess PLAN RECEIPT | record PLAN OLD_RECEIPT EVENT NEW_RECEIPT | abort PLAN LAST_ACCEPTED CLEANUP NEW_ABORT | preflight PLAN APPROVAL SERIAL --read-device');
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const result = main(process.argv.slice(2));
    console.log(JSON.stringify(result, null, 2));
    if (result.verdict === 'NOT_READY') process.exitCode = 2;
    if (result.verdict === 'INVALID_EVIDENCE') process.exitCode = 1;
  } catch (error) {
    // Assertion errors may embed caller input; do not print raw config/receipts or stack traces.
    console.error(`FAIL: ${error.message.split('\n')[0]}`);
    process.exitCode = 1;
  }
}
