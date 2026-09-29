#!/usr/bin/env node
// Offline plan/receipt gate. No CDP attachment, Activity start, VPN or policy writes.
import assert from 'node:assert/strict';
import { createHash, randomBytes } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { isIP } from 'node:net';
import { resolve } from 'node:path';
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
  appPid, physicalTransport, networkHandle, wifiEnabled, cellularEnabled, airplaneEnabled }) {
  assert.ok(Array.isArray(config.servers), 'config.servers required');
  const target = config.servers.find(server => server.id === targetId);
  assert.ok(target && target.protocol === 'tailscale', 'existing Headscale tailscale server required');
  const stable = structuredClone(target);
  delete stable.meshInboundPolicy;
  const invariant = structuredClone(config);
  delete invariant.selectedServerId;
  delete invariant.servers.find(server => server.id === targetId).meshInboundPolicy;
  assert.ok(identity !== undefined && pcState !== undefined && networkHandle !== undefined);
  return validateSnapshot({ configSha256: digest(config), invariantsSha256: digest(invariant),
    identitySha256: digest(identity), serverStableSha256: digest(stable),
    selectedServerSha256: digest(config.selectedServerId ?? null),
    policySha256: digest(target.meshInboundPolicy ?? null), pcStateSha256: digest(pcState),
    appPid, proxyRunning: status.running, proxyStarting: status.starting ?? false,
    pendingChanges, physicalTransport, networkHandleSha256: digest(networkHandle),
    wifiEnabled, cellularEnabled, airplaneEnabled });
}
export function createPlan(input, nonce = randomBytes(24).toString('hex')) {
  exactKeys(input, ['schemaVersion', 'package', 'serialSha256', 'sourceHead', 'apkSha256',
    'targetServerSha256', 'pcMeshIp', 'androidMeshIp', 'tcpPort', 'udpPort',
    'attempts', 'timeoutMs', 'baseline'], 'manifest');
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
    scopes: ['attach', 'witness', 'traffic', 'android-policy', 'android-vpn', 'wifi-toggle'],
    claimsExcluded: ['warp-egress', 'dns-data-plane', 'ipv6', 'direct-vs-relay'],
  };
  return { ...body, planSha256: digest(body) };
}
export function validatePlan(plan) {
  const { planSha256, ...body } = plan;
  hash(planSha256, 'planSha256');
  assert.equal(digest(body), planSha256, 'plan or immutable baseline changed');
  const manifest = Object.fromEntries(['schemaVersion', 'package', 'serialSha256', 'sourceHead',
    'apkSha256', 'targetServerSha256', 'pcMeshIp', 'androidMeshIp', 'tcpPort', 'udpPort',
    'attempts', 'timeoutMs', 'baseline'].map(key => [key, plan[key]]));
  assert.deepEqual(plan, createPlan(manifest, plan.nonce), 'unrecognized plan shape');
  return plan;
}
export function requireApproval(plan, approval, scope, now = Date.now()) {
  validatePlan(plan);
  exactKeys(approval, ['schemaVersion', 'planSha256', 'approvedByOperator', 'scopes',
    'issuedAtMs', 'expiresAtMs'], 'approval');
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
    'received', 'matched', 'challengeSha256s'], 'witness counter');
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
function checkProbe(plan, phase, value) {
  exactKeys(value, ['direction', 'protocol', 'attempted', 'ackSha256s', 'outcomes',
    'before', 'after', 'loopbackHealth'], 'probe');
  const { direction, protocol } = value;
  assert.ok(DIRECTIONS.includes(direction) && PROTOCOLS.includes(protocol));
  assert.equal(value.attempted, plan.attempts, 'attempts cannot be skipped');
  assert.ok(Array.isArray(value.outcomes) && value.outcomes.length === plan.attempts);
  assert.ok(Array.isArray(value.ackSha256s));
  assert.equal(typeof value.loopbackHealth, 'boolean', 'loopback health must be a typed observation');
  counter(value.before, plan, protocol, direction);
  counter(value.after, plan, protocol, direction);
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
  const { networkHandleSha256, ...stable } = snapshot;
  return stable;
}
function checkCleanup(plan, event) {
  exactKeys(event, ['stage', 'snapshot', 'witnessesStopped', 'forwardsRemoved', 'artifactsRemoved'], 'cleanup');
  assert.equal(event.stage, 'cleanup');
  validateSnapshot(event.snapshot);
  assert.deepEqual(stableSnapshot(event.snapshot), stableSnapshot(plan.baseline), 'restore did not match immutable baseline');
  assert.equal(event.witnessesStopped, true);
  assert.equal(event.forwardsRemoved, true);
  assert.equal(event.artifactsRemoved, true);
}
export function assessBatch(plan, receipt) {
  validatePlan(plan);
  exactKeys(receipt, ['schemaVersion', 'planSha256', 'events'], 'receipt');
  assert.equal(receipt.schemaVersion, 1);
  assert.equal(receipt.planSha256, plan.planSha256);
  assert.ok(Array.isArray(receipt.events) && receipt.events.length <= plan.stages.length);
  const continuity = new Map();
  let cellularHandle;
  for (let i = 0; i < receipt.events.length; i++) {
    const event = receipt.events[i];
    const stage = plan.stages[i];
    assert.equal(event.stage, stage, 'missing, reordered or duplicated stage; stop batch');
    exactKeys(event, stage === 'cleanup'
      ? ['stage', 'snapshot', 'witnessesStopped', 'forwardsRemoved', 'artifactsRemoved']
      : stage === 'baseline' ? ['stage', 'snapshot'] : ['stage', 'snapshot', 'probes'], stage);
    const snapshot = validateSnapshot(event.snapshot);
    if (stage === 'baseline') assert.deepEqual(snapshot, plan.baseline, 'baseline drift before first mutation');
    else if (stage === 'cleanup') checkCleanup(plan, event);
    else {
      for (const key of ['invariantsSha256', 'identitySha256', 'serverStableSha256', 'pcStateSha256', 'appPid']) {
        assert.equal(snapshot[key], plan.baseline[key], `${stage}: protected baseline changed: ${key}`);
      }
      assert.equal(snapshot.selectedServerSha256, plan.targetServerSha256);
      assert.equal(snapshot.policySha256, stage === 'policy-negative' ? plan.blockPolicySha256 : plan.allowPolicySha256);
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
      for (const probe of event.probes) {
        const key = `${probe.direction}/${probe.protocol}`;
        assert.ok(!pairs.has(key), 'duplicate direction/protocol coverage');
        pairs.add(key);
        checkProbe(plan, stage, probe);
        if (continuity.has(key)) assert.deepEqual(probe.before, continuity.get(key), 'cross-phase counter discontinuity');
        else {
          assert.equal(probe.before.received, 0, 'fresh witness must start with zero received packets');
          assert.equal(probe.before.matched, 0, 'fresh witness must start with zero matched challenges');
        }
        continuity.set(key, probe.after);
      }
    }
  }
  const complete = receipt.events.length === plan.stages.length;
  return { verdict: complete ? 'PASS' : 'INCOMPLETE', completedStages: receipt.events.length,
    nextStage: complete ? null : plan.stages[receipt.events.length],
    claims: complete ? ['P5 IPv4 TCP/UDP bidirectional echo', 'P5 inbound-policy rejection bracketed by positive controls',
      'P8 Wi-Fi/cellular/Wi-Fi IPv4 TCP/UDP fresh echoes', 'baseline restored'] : [],
    excluded: plan.claimsExcluded };
}
export function requireNextStage(plan, receipt, stage, approval, scope, now = Date.now()) {
  const result = assessBatch(plan, receipt);
  assert.equal(result.nextStage, stage, 'prior checkpoint failed or requested stage is out of order');
  requireApproval(plan, approval, scope, now);
}
export function appendEvent(plan, receipt, event) {
  const next = { ...structuredClone(receipt), events: [...structuredClone(receipt.events), structuredClone(event)] };
  assessBatch(plan, next); // validate before saving or advancing to another external action
  return next;
}
export function abortBatch(plan, lastAcceptedReceipt, cleanup) {
  const last = assessBatch(plan, lastAcceptedReceipt);
  assert.equal(last.verdict, 'INCOMPLETE', 'completed batch cannot be retroactively aborted');
  let restored = false;
  try { checkCleanup(plan, cleanup); restored = true; } catch { /* preserve incomplete/failed restore verdict */ }
  return { schemaVersion: 1, planSha256: plan.planSha256,
    verdict: restored ? 'ABORTED_RESTORED' : 'RESTORE_FAILED',
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
  if (args[0] === 'plan' && args.length === 3) {
    const plan = createPlan(read(args[1]));
    writePrivateJson(args[2], plan);
    return { mode: 'DRY_RUN', planSha256: plan.planSha256, stages: plan.stages,
      requiredApprovalScopes: plan.scopes, excluded: plan.claimsExcluded };
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
  if (args[0] === 'preflight' && args.length >= 3 && args.length <= 4) {
    assert.equal(args[2], '--read-device', 'preflight requires explicit --read-device');
    assert.ok(args.length === 3 || args[3] === '--release');
    const app = args.length === 4 ? RELEASE_APP : DEBUG_APP;
    const adbPath = `${process.env.ANDROID_HOME ?? `${process.env.HOME}/Android/Sdk`}/platform-tools/adb`;
    const adb = (...tail) => execFileSync(adbPath, ['-s', args[1], ...tail], { encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'pipe'] });
    return readOnlyPreflight(args[1], app, adb);
  }
  throw new Error('Usage: android-batch-qa.mjs plan MANIFEST NEW_PLAN | assess PLAN RECEIPT | record PLAN OLD_RECEIPT EVENT NEW_RECEIPT | abort PLAN LAST_ACCEPTED CLEANUP NEW_ABORT | preflight SERIAL --read-device [--release]');
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const result = main(process.argv.slice(2));
    console.log(JSON.stringify(result, null, 2));
    if (['INCOMPLETE', 'ABORTED_RESTORED'].includes(result.verdict)) process.exitCode = 2;
    if (result.verdict === 'RESTORE_FAILED') process.exitCode = 1;
  } catch (error) {
    // Assertion errors may embed caller input; do not print raw config/receipts or stack traces.
    console.error(`FAIL: ${error.message.split('\n')[0]}`);
    process.exitCode = 1;
  }
}
