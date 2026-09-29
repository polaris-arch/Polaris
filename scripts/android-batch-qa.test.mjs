import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { abortBatch, appendEvent, assessBatch, challenge, createPlan, digest, expectedChallenges,
  main, PHASES, projectSnapshot, readOnlyPreflight, requireApproval, requireNextStage,
  validatePlan, writePrivateJson } from './android-batch-qa.mjs';
import { androidArgs, probe, WitnessLedger } from './android-batch-witness.mjs';

function fixture() {
  const h = label => digest(label);
  const baseline = { configSha256: h('config'), invariantsSha256: h('invariants'),
    identitySha256: h('identity'), serverStableSha256: h('server'), selectedServerSha256: h('target'),
    policySha256: h(null), pcStateSha256: h('pc'), appPid: 1234, proxyRunning: false,
    proxyStarting: false, pendingChanges: false, physicalTransport: 'wifi',
    networkHandleSha256: h('wifi-handle'), wifiEnabled: true, cellularEnabled: true, airplaneEnabled: false };
  const plan = createPlan({ schemaVersion: 1, package: 'com.polaris2.app.debug',
    serialSha256: h('emulator-5554'), sourceHead: 'a'.repeat(40), apkSha256: h('apk'),
    targetServerSha256: h('target'), pcMeshIp: '100.64.10.1', androidMeshIp: '100.64.10.2',
    tcpPort: 40101, udpPort: 40102, attempts: 3, timeoutMs: 1500, baseline }, 'e'.repeat(48));
  const receipt = { schemaVersion: 1, planSha256: plan.planSha256,
    events: [{ stage: 'baseline', snapshot: structuredClone(baseline) }] };
  const ledgers = new Map();
  for (const direction of ['pc-to-android', 'android-to-pc']) for (const protocol of ['tcp', 'udp'])
    ledgers.set(`${direction}/${protocol}`, new WitnessLedger(plan, direction === 'pc-to-android' ? 'android' : 'pc', protocol));
  for (const phase of PHASES) {
    const snapshot = { ...baseline, configSha256: h(phase), proxyRunning: true,
      selectedServerSha256: plan.targetServerSha256,
      policySha256: phase === 'policy-negative' ? plan.blockPolicySha256 : plan.allowPolicySha256,
      physicalTransport: phase === 'cellular-positive' ? 'cellular' : 'wifi',
      wifiEnabled: phase !== 'cellular-positive',
      networkHandleSha256: phase === 'cellular-positive' ? h('cellular-handle') : h('wifi-handle') };
    const probes = [];
    for (const direction of ['pc-to-android', 'android-to-pc']) for (const protocol of ['tcp', 'udp']) {
      const ledger = ledgers.get(`${direction}/${protocol}`);
      const before = ledger.snapshot();
      const blocked = phase === 'policy-negative' && direction === 'pc-to-android';
      if (!blocked) for (let i = 0; i < plan.attempts; i++) {
        const payload = challenge(plan, phase, direction, protocol, i);
        assert.equal(ledger.receive(payload), payload);
      }
      probes.push({ direction, protocol, attempted: plan.attempts,
        outcomes: Array(plan.attempts).fill(blocked ? 'timeout' : 'echo'),
        ackSha256s: blocked ? [] : expectedChallenges(plan, phase, direction, protocol),
        before, after: ledger.snapshot(), loopbackHealth: blocked });
    }
    receipt.events.push({ stage: phase, snapshot, probes });
  }
  receipt.events.push({ stage: 'cleanup', snapshot: { ...baseline, networkHandleSha256: h('new-wifi-handle') },
    witnessesStopped: true, forwardsRemoved: true, artifactsRemoved: true });
  const now = Date.now();
  const approval = { schemaVersion: 1, planSha256: plan.planSha256, approvedByOperator: true,
    scopes: [...plan.scopes], issuedAtMs: now - 1000, expiresAtMs: now + 60000 };
  return { plan, receipt, approval };
}
test('one batch passes only after bracketed negative, physical handover and complete restore', () => {
  const { plan, receipt } = fixture();
  const result = assessBatch(plan, receipt);
  assert.equal(result.verdict, 'PASS');
  assert.equal(result.claims.length, 4);
  assert.deepEqual(result.excluded, ['warp-egress', 'dns-data-plane', 'ipv6', 'direct-vs-relay']);
  const partial = { ...receipt, events: receipt.events.slice(0, -1) };
  assert.equal(assessBatch(plan, partial).verdict, 'INCOMPLETE');
  assert.equal(assessBatch(plan, partial).nextStage, 'cleanup');
});
test('immutable plan, private schema, installed variant and fixed scope resist drift', () => {
  const { plan } = fixture();
  for (const mutate of [p => { p.baseline.identitySha256 = digest('changed'); },
    p => { p.attempts = 0; }, p => { p.package = 'com.polaris.app'; },
    p => { p.rawConfig = { token: 'do-not-write' }; }]) {
    const changed = structuredClone(plan); mutate(changed);
    assert.throws(() => validatePlan(changed));
  }
  assert.equal(plan.allowPolicy.rules[0].sourceCidrs[0], '100.64.10.1/32');
  assert.deepEqual(plan.allowPolicy.rules[0].ports, ['40101', '40102']);
});
test('negative refuses dead listener, local errors, matching nonce and skipped attempts', () => {
  for (const mutate of [p => { p.loopbackHealth = false; },
    p => { p.before.alive = false; }, p => { p.outcomes[0] = 'local-error'; },
    p => { p.attempted = 0; }, p => { p.after.received++; },
    p => { p.ackSha256s = [digest('unexpected ACK')]; }]) {
    const { plan, receipt } = fixture();
    mutate(receipt.events[2].probes[0]);
    assert.throws(() => assessBatch(plan, receipt));
  }
});
test('fresh client echoes require exact receiver counters and prohibit replay/restart/reset', () => {
  for (const mutate of [p => { p.ackSha256s[0] = digest('old nonce'); },
    p => { p.after.matched--; }, p => { p.after.instanceSha256 = digest('new process'); },
    p => { p.after.nonceSha256 = digest('old batch'); },
    p => { p.before = structuredClone(p.after); },
    p => { p.after.challengeSha256s.push(p.after.challengeSha256s[0]); }]) {
    const { plan, receipt } = fixture(); mutate(receipt.events[1].probes[0]);
    assert.throws(() => assessBatch(plan, receipt));
  }
  const { plan, receipt } = fixture();
  receipt.events[3].probes[0].before.received++;
  receipt.events[3].probes[0].after.received++;
  assert.throws(() => assessBatch(plan, receipt), /cross-phase counter discontinuity/);
});
test('stage order and all four direction/protocol pairs are mandatory', () => {
  for (const mutate of [r => { r.events.splice(2, 1); },
    r => { [r.events[1], r.events[2]] = [r.events[2], r.events[1]]; },
    r => { r.events[1].probes.pop(); },
    r => { r.events[1].probes[1] = structuredClone(r.events[1].probes[0]); }]) {
    const { plan, receipt } = fixture(); mutate(receipt);
    assert.throws(() => assessBatch(plan, receipt));
  }
});
test('policy application, stable identities, PC baseline and PID are required at every stage', () => {
  for (const mutate of [s => { s.policySha256 = digest('not applied'); },
    s => { s.pendingChanges = true; }, s => { s.identitySha256 = digest('new login'); },
    s => { s.invariantsSha256 = digest('unrelated edit'); },
    s => { s.serverStableSha256 = digest('credential changed'); },
    s => { s.pcStateSha256 = digest('PC peer switched'); }, s => { s.appPid++; }]) {
    const { plan, receipt } = fixture(); mutate(receipt.events[2].snapshot);
    assert.throws(() => assessBatch(plan, receipt));
  }
});
test('P8 requires observed underlying network change with both network switches restored', () => {
  for (const mutate of [r => { r.events[4].snapshot.networkHandleSha256 = r.events[0].snapshot.networkHandleSha256; },
    r => { r.events[4].snapshot.physicalTransport = 'wifi'; },
    r => { r.events[5].snapshot.networkHandleSha256 = r.events[4].snapshot.networkHandleSha256; },
    r => { r.events[6].snapshot.wifiEnabled = false; },
    r => { r.events[6].snapshot.configSha256 = digest('not restored'); },
    r => { r.events[6].witnessesStopped = false; },
    r => { r.events[6].forwardsRemoved = false; }, r => { r.events[6].artifactsRemoved = false; }]) {
    const { plan, receipt } = fixture(); mutate(receipt);
    assert.throws(() => assessBatch(plan, receipt));
  }
});
test('scope, plan and expiry gates run before any probe side effect', async () => {
  const { plan, approval } = fixture();
  let calls = 0;
  const send = async () => { calls++; return { outcome: 'echo', ackSha256: digest('stub') }; };
  for (const mutate of [a => { a.scopes = ['witness']; },
    a => { a.planSha256 = digest('another batch'); }, a => { a.approvedByOperator = false; },
    a => { a.expiresAtMs = a.issuedAtMs + 1; }]) {
    const changed = structuredClone(approval); mutate(changed);
    await assert.rejects(() => probe(plan, changed, 'wifi-positive', 'tcp', send));
  }
  assert.equal(calls, 0);
  requireApproval(plan, approval, 'traffic');
});
test('probe uses only planned mesh IP/port, fresh challenges and stops at local error/expiry', async () => {
  const { plan, approval } = fixture();
  let sequence = 0;
  const result = await probe(plan, approval, 'wifi-return', 'udp', async (protocol, host, port, payload, timeout) => {
    assert.equal(protocol, 'udp'); assert.equal(host, plan.androidMeshIp);
    assert.equal(port, plan.udpPort); assert.equal(timeout, plan.timeoutMs);
    assert.equal(payload, challenge(plan, 'wifi-return', 'pc-to-android', 'udp', sequence++));
    return { outcome: 'echo', ackSha256: digest('mock') };
  });
  assert.equal(result.attempted, plan.attempts);
  let calls = 0;
  assert.equal((await probe(plan, approval, 'wifi-positive', 'tcp', async () => {
    calls++; return { outcome: 'local-error', ackSha256: null };
  })).attempted, 1);
  assert.equal(calls, 1);
  calls = 0;
  await assert.rejects(() => probe(plan, approval, 'wifi-positive', 'tcp', async () => {
    calls++; approval.expiresAtMs = Date.now() - 1;
    return { outcome: 'timeout', ackSha256: null };
  }), /approval not current/);
  assert.equal(calls, 1);
});
test('failed prior receipt cannot authorize a new stage; append validates before write', () => {
  const { plan, receipt, approval } = fixture();
  const baseline = { ...receipt, events: receipt.events.slice(0, 1) };
  requireNextStage(plan, baseline, 'wifi-positive', approval, 'android-policy');
  assert.throws(() => requireNextStage(plan, baseline, 'policy-negative', approval, 'android-policy'));
  const next = appendEvent(plan, baseline, receipt.events[1]);
  assert.equal(next.events.length, 2);
  assert.equal(baseline.events.length, 1);
  const bad = structuredClone(receipt.events[1]); bad.snapshot.identitySha256 = digest('drift');
  assert.throws(() => appendEvent(plan, baseline, bad));
});
test('failure stops progression but still permits cleanup without issuing a PASS claim', () => {
  const { plan, receipt } = fixture();
  const last = { ...receipt, events: receipt.events.slice(0, 2) };
  const cleanup = receipt.events.at(-1);
  const result = abortBatch(plan, last, cleanup);
  assert.equal(result.verdict, 'ABORTED_RESTORED');
  assert.deepEqual(result.claims, []);
  assert.equal(abortBatch(plan, last, { ...cleanup, witnessesStopped: false }).verdict, 'RESTORE_FAILED');
});
test('counter ledger rejects another phase direction, old nonce and duplicate echoes', () => {
  const { plan } = fixture();
  const ledger = new WitnessLedger(plan, 'android', 'tcp');
  const payload = challenge(plan, 'wifi-positive', 'pc-to-android', 'tcp', 0);
  assert.equal(ledger.receive(payload), payload);
  assert.equal(ledger.receive(payload), null);
  assert.equal(ledger.receive(payload.replace(plan.nonce, 'b'.repeat(48))), null);
  assert.equal(ledger.receive(challenge(plan, 'wifi-positive', 'android-to-pc', 'tcp', 0)), null);
  const before = ledger.snapshot();
  assert.equal(ledger.receive(`polaris-health-v1:${plan.nonce}:tcp\n`), `polaris-health-v1:${plan.nonce}:tcp\n`);
  assert.deepEqual(ledger.snapshot(), before);
  assert.equal(before.matched, 1);
  assert.equal(before.received, 4);
});
test('projection writes only hashes and detects changes outside selected server/policy', () => {
  const input = { config: { selectedServerId: null,
    clashApiSecret: 'PRIVATE_TEST_SECRET', servers: [{ id: 'target', protocol: 'tailscale', authKey: 'PRIVATE_TEST_KEY' }] },
    targetId: 'target', identity: { key: 'PRIVATE_TEST_IDENTITY' }, pcState: { id: 'pc' },
    status: { running: false, starting: false }, pendingChanges: false, appPid: 123,
    physicalTransport: 'wifi', networkHandle: 'net1', wifiEnabled: true, cellularEnabled: true, airplaneEnabled: false };
  const baseline = projectSnapshot(input);
  assert.ok(!JSON.stringify(baseline).includes('PRIVATE_TEST'));
  input.config.selectedServerId = 'target'; input.config.servers[0].meshInboundPolicy = { mode: 'block' };
  assert.equal(projectSnapshot(input).invariantsSha256, baseline.invariantsSha256);
  input.config.servers[0].authKey = 'CHANGED';
  assert.notEqual(projectSnapshot(input).invariantsSha256, baseline.invariantsSha256);
});
test('read-only ADB preflight never launches, attaches, forwards or reads raw config', () => {
  const calls = [];
  const adb = (...args) => {
    calls.push(args);
    if (args[0] === 'get-state') return 'device\n';
    if (args[1] === 'pm') return 'package:com.polaris2.app.debug\n';
    return '1234\n';
  };
  assert.equal(readOnlyPreflight('emulator-5554', 'com.polaris2.app.debug', adb).appPid, 1234);
  assert.deepEqual(calls, [['get-state'], ['shell', 'pm', 'list', 'packages', 'com.polaris2.app'],
    ['shell', 'pidof', 'com.polaris2.app.debug']]);
  assert.throws(() => main(['preflight', 'emulator-5554']), /Usage/);
});
test('private files refuse overwrite; witness CLI default needs no approval and opens no sockets', () => {
  const { plan } = fixture();
  const dir = mkdtempSync(join(tmpdir(), 'polaris-batch-qa-'));
  try {
    const path = join(dir, 'plan.json');
    writePrivateJson(path, plan);
    assert.equal(statSync(path).mode & 0o777, 0o600);
    assert.throws(() => writePrivateJson(path, plan));
    const script = new URL('./android-batch-witness.mjs', import.meta.url);
    for (const args of [['serve', path, '/nonexistent/approval', '/nonexistent/counter-file'],
      ['probe', path, '/nonexistent/approval', 'wifi-positive', 'tcp'],
      ['android-serve', path, '/nonexistent/approval']]) {
      const result = spawnSync(process.execPath, [script.pathname, ...args], { encoding: 'utf8', timeout: 5000 });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(JSON.parse(result.stdout).mode, 'DRY_RUN');
    }
    assert.deepEqual(JSON.parse(readFileSync(path, 'utf8')), plan);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('approved Android argv is tightly bounded and has no private sessions or raw config', () => {
  const { plan, approval } = fixture();
  const serve = androidArgs(plan, approval, 'serve', []);
  assert.equal(serve[5], `cache/polaris-qa-${plan.nonce}/counters.json`);
  assert.equal(serve.at(-2), plan.planSha256);
  assert.equal(serve.at(-1), String(approval.expiresAtMs));
  assert.equal(androidArgs(plan, approval, 'probe', ['wifi-return', 'udp'])[4], plan.pcMeshIp);
  assert.throws(() => androidArgs(plan, { ...approval, scopes: [] }, 'serve', []));
});
