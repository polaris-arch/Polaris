import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { abortBatch, appendEvent, assessBatch, challenge, createPlan, digest, expectedChallenges,
  main, PHASES, projectSnapshot, readOnlyPreflight, requireApproval, requireNextStage,
  privateManifestBytes, sha256, validatePlan, verifyCopiedHashOutput, verifyWitnessBytes,
  writePrivateJson } from './android-batch-qa.mjs';
import { androidArgs, probe, serve, validateCounterFile, WitnessLedger } from './android-batch-witness.mjs';

function fixture(qaRoot = '/tmp/polaris-qa-offline-fixture') {
  const h = label => digest(label);
  const baseline = { configSha256: h('config'), invariantsSha256: h('invariants'),
    identitySha256: h('identity'), serverStableSha256: h('server'), selectedServerSha256: h('target'),
    policySha256: h(null), runtimePolicySha256: h(null), perAppVpnSha256: h('app-included'),
    observedAtMs: 1700000000000, pcStateSha256: h('pc'), appPid: 1234, proxyRunning: false,
    proxyStarting: false, pendingChanges: false, physicalTransport: 'wifi',
    networkHandleSha256: h('wifi-handle'), wifiEnabled: true, cellularEnabled: true, airplaneEnabled: false };
  const plan = createPlan({ schemaVersion: 1, package: 'com.polaris2.app.debug',
    serialSha256: h('emulator-5554'), sourceHead: 'a'.repeat(40), apkSha256: h('apk'),
    targetServerSha256: h('target'), pcMeshIp: '100.64.10.1', androidMeshIp: '100.64.10.2',
    tcpPort: 40101, udpPort: 40102, attempts: 3, timeoutMs: 1500, baseline, qaRoot,
    witnessBuild: { schemaVersion: 1, sourceSha256: h('source'), javacSha256: h('javac'),
      d8Sha256: h('d8'), androidJarSha256: h('android'), classJarSha256: h('classes'),
      dexSha256: sha256('mock-dex'), minApi: 24 } }, 'e'.repeat(48));
  const receipt = { schemaVersion: 1, planSha256: plan.planSha256,
    witness: { dexSha256: plan.witnessBuild.dexSha256, manifestSha256: plan.planSha256,
      buildReceiptSha256: plan.witnessBuildReceiptSha256 },
    events: [{ stage: 'baseline', snapshot: structuredClone(baseline) }] };
  const ledgers = new Map();
  for (const direction of ['pc-to-android', 'android-to-pc']) for (const protocol of ['tcp', 'udp'])
    ledgers.set(`${direction}/${protocol}`, new WitnessLedger(plan, direction === 'pc-to-android' ? 'android' : 'pc', protocol));
  let clock = baseline.observedAtMs + 100;
  for (const phase of PHASES) {
    const snapshot = { ...baseline, configSha256: h(phase), proxyRunning: true,
      selectedServerSha256: plan.targetServerSha256,
      policySha256: phase === 'policy-negative' ? plan.blockPolicySha256 : plan.allowPolicySha256,
      runtimePolicySha256: phase === 'policy-negative' ? plan.blockPolicySha256 : plan.allowPolicySha256,
      observedAtMs: clock++,
      physicalTransport: phase === 'cellular-positive' ? 'cellular' : 'wifi',
      wifiEnabled: phase !== 'cellular-positive',
      networkHandleSha256: phase === 'cellular-positive' ? h('cellular-handle') : h('wifi-handle') };
    const probes = [];
    for (const direction of ['pc-to-android', 'android-to-pc']) for (const protocol of ['tcp', 'udp']) {
      const ledger = ledgers.get(`${direction}/${protocol}`);
      const before = ledger.snapshot(clock++);
      const startedAtMs = clock++, finishedAtMs = clock++;
      const blocked = phase === 'policy-negative' && direction === 'pc-to-android';
      if (!blocked) for (let i = 0; i < plan.attempts; i++) {
        const payload = challenge(plan, phase, direction, protocol, i);
        assert.equal(ledger.receive(payload), payload);
      }
      probes.push({ direction, protocol, attempted: plan.attempts,
        outcomes: Array(plan.attempts).fill(blocked ? 'timeout' : 'echo'),
        ackSha256s: blocked ? [] : expectedChallenges(plan, phase, direction, protocol),
        before, after: ledger.snapshot(clock++), loopbackHealth: blocked, startedAtMs, finishedAtMs });
    }
    receipt.events.push({ stage: phase, beforeSnapshot: snapshot,
      afterSnapshot: { ...snapshot, observedAtMs: clock++ }, probes });
  }
  receipt.events.push({ stage: 'cleanup', snapshot: { ...baseline, networkHandleSha256: h('new-wifi-handle') },
    cleanupEvidence: { verdict: 'NOT_OBSERVED', missingObservers: plan.readiness.missingObservers } });
  const now = Date.now();
  const approval = { schemaVersion: 1, planSha256: plan.planSha256, approvedByOperator: true,
    scopes: [...plan.scopes], issuedAtMs: now - 1000, expiresAtMs: now + 60000, presenceConfirmedAtMs: now };
  return { plan, receipt, approval };
}
test('even a complete bracketed fixture is NOT_READY and cannot claim P5/P8 or restoration', () => {
  const { plan, receipt } = fixture();
  const result = assessBatch(plan, receipt);
  assert.equal(result.verdict, 'NOT_READY');
  assert.equal(result.structurallyComplete, true);
  assert.deepEqual(result.claims, []);
  assert.ok(result.excluded.includes('P5-data-plane') && result.excluded.includes('P8-handover'));
  const partial = { ...receipt, events: receipt.events.slice(0, -1) };
  assert.equal(assessBatch(plan, partial).verdict, 'NOT_READY');
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
  const forged = structuredClone(plan);
  forged.readiness.executable = true;
  const { planSha256, ...body } = forged;
  forged.planSha256 = digest(body);
  assert.throws(() => validatePlan(forged), /unrecognized plan shape/);
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
    const { plan, receipt } = fixture(); mutate(receipt.events[2].beforeSnapshot);
    assert.throws(() => assessBatch(plan, receipt));
  }
});
test('P8 requires observed underlying network change with both network switches restored', () => {
  for (const mutate of [r => { r.events[4].beforeSnapshot.networkHandleSha256 = r.events[0].snapshot.networkHandleSha256; },
    r => { r.events[4].beforeSnapshot.physicalTransport = 'wifi'; },
    r => { r.events[5].beforeSnapshot.networkHandleSha256 = r.events[4].beforeSnapshot.networkHandleSha256; },
    r => { r.events[6].snapshot.wifiEnabled = false; },
    r => { r.events[6].snapshot.configSha256 = digest('not restored'); },
    r => { r.events[6].witnessesStopped = true; },
    r => { r.events[6].cleanupEvidence.verdict = 'OBSERVED'; }]) {
    const { plan, receipt } = fixture(); mutate(receipt);
    assert.throws(() => assessBatch(plan, receipt));
  }
});
test('prepared-only rejects every live operation before any callback or socket', () => {
  const { plan, approval } = fixture();
  let calls = 0;
  const send = async () => { calls++; return { outcome: 'echo', ackSha256: digest('stub') }; };
  for (const mutate of [a => { a.scopes = ['witness']; },
    a => { a.planSha256 = digest('another batch'); }, a => { a.approvedByOperator = false; },
    a => { a.expiresAtMs = a.issuedAtMs + 1; }]) {
    const changed = structuredClone(approval); mutate(changed);
    assert.throws(() => probe(plan, changed, 'wifi-positive', 'tcp', send), /NOT_READY/);
  }
  assert.equal(calls, 0);
  requireApproval(plan, approval, 'traffic');
});
test('approval separates scopes and rejects stale presence even for read-only device observation', () => {
  const { plan, approval } = fixture();
  assert.deepEqual(plan.scopeGroups.readOnly, ['device-observe']);
  assert.deepEqual(plan.scopeGroups.wifi, ['wifi-toggle']);
  assert.deepEqual(plan.scopeGroups.cleanup, ['cleanup']);
  assert.throws(() => requireApproval(plan, { ...approval, scopes: ['device-observe'] }, 'android-policy'));
  assert.throws(() => requireApproval(plan, { ...approval, presenceConfirmedAtMs: Date.now() - 60001 }, 'device-observe'), /presence/);
  requireApproval(plan, approval, 'device-observe');
  assert.throws(() => serve(plan), /NOT_READY/);
});
test('failed prior receipt cannot authorize a new stage; append validates before write', () => {
  const { plan, receipt, approval } = fixture();
  const baseline = { ...receipt, events: receipt.events.slice(0, 1) };
  assert.throws(() => requireNextStage(plan, baseline, 'wifi-positive', approval, 'android-policy'), /NOT_READY/);
  assert.throws(() => requireNextStage(plan, baseline, 'policy-negative', approval, 'android-policy'));
  const next = appendEvent(plan, baseline, receipt.events[1]);
  assert.equal(next.events.length, 2);
  assert.equal(baseline.events.length, 1);
  const bad = structuredClone(receipt.events[1]); bad.beforeSnapshot.identitySha256 = digest('drift');
  assert.throws(() => appendEvent(plan, baseline, bad));
});
test('failure stops progression but still permits cleanup without issuing a PASS claim', () => {
  const { plan, receipt } = fixture();
  const last = { ...receipt, events: receipt.events.slice(0, 2) };
  const cleanup = receipt.events.at(-1);
  const result = abortBatch(plan, last, cleanup);
  assert.equal(result.verdict, 'NOT_READY');
  assert.equal(result.restoration, 'NOT_OBSERVED');
  assert.deepEqual(result.claims, []);
  assert.equal(abortBatch(plan, last, { ...cleanup, witnessesStopped: true }).verdict, 'INVALID_EVIDENCE');
});
test('counter ledger rejects another phase direction, old nonce and duplicate echoes', () => {
  const { plan } = fixture();
  const ledger = new WitnessLedger(plan, 'android', 'tcp');
  const payload = challenge(plan, 'wifi-positive', 'pc-to-android', 'tcp', 0);
  assert.equal(ledger.receive(payload), payload);
  assert.equal(ledger.receive(payload), null);
  assert.equal(ledger.receive(payload.replace(plan.nonce, 'b'.repeat(48))), null);
  assert.equal(ledger.receive(challenge(plan, 'wifi-positive', 'android-to-pc', 'tcp', 0)), null);
  const before = ledger.snapshot(1);
  assert.equal(ledger.receive(`polaris-health-v1:${plan.nonce}:tcp\n`), `polaris-health-v1:${plan.nonce}:tcp\n`);
  assert.deepEqual(ledger.snapshot(1), before);
  assert.equal(before.matched, 1);
  assert.equal(before.received, 4);
});
test('projection writes only hashes and detects changes outside selected server/policy', () => {
  const input = { config: { selectedServerId: null,
    clashApiSecret: 'PRIVATE_TEST_SECRET', servers: [{ id: 'target', protocol: 'tailscale', authKey: 'PRIVATE_TEST_KEY' }] },
    targetId: 'target', identity: { key: 'PRIVATE_TEST_IDENTITY' }, pcState: { id: 'pc' },
    status: { running: false, starting: false }, pendingChanges: false, appPid: 123,
    runtimePolicy: null, perAppVpn: { included: true }, observedAtMs: Date.now(),
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
test('private files refuse overwrite; CLI default and --execute can never produce PASS or sockets', () => {
  const dir = mkdtempSync(join(tmpdir(), 'polaris-batch-qa-'));
  const { plan } = fixture(dir);
  try {
    const path = join(dir, 'plan.json');
    writePrivateJson(path, plan);
    assert.equal(statSync(path).mode & 0o777, 0o600);
    assert.throws(() => writePrivateJson(path, plan));
    const script = new URL('./android-batch-witness.mjs', import.meta.url);
    for (const args of [['serve', path, '/nonexistent/approval', join(dir, `pc-counters-${plan.nonce}.json`)],
      ['probe', path, '/nonexistent/approval', 'wifi-positive', 'tcp'],
      ['android-serve', path, '/nonexistent/approval']]) {
      const result = spawnSync(process.execPath, [script.pathname, ...args], { encoding: 'utf8', timeout: 5000 });
      assert.equal(result.status, 2, result.stderr);
      assert.equal(JSON.parse(result.stdout).mode, 'DRY_RUN');
      const execute = spawnSync(process.execPath, [script.pathname, ...args, '--execute'], { encoding: 'utf8', timeout: 5000 });
      assert.equal(execute.status, 1);
      assert.match(execute.stderr, /NOT_READY/);
    }
    assert.deepEqual(JSON.parse(readFileSync(path, 'utf8')), plan);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('prepared-only never emits executable Android argv, including fully approved plans', () => {
  const { plan, approval } = fixture();
  assert.throws(() => androidArgs(plan, approval, 'serve', []), /NOT_READY/);
  assert.throws(() => androidArgs(plan, { ...approval, scopes: [] }, 'serve', []));
});
test('both phase snapshots bracket counters/probes and reject pre/post drift or counter time escape', () => {
  for (const mutate of [event => { event.beforeSnapshot.policySha256 = digest('previous policy'); },
    event => { event.afterSnapshot.networkHandleSha256 = digest('changed mid-probe'); },
    event => { event.afterSnapshot.physicalTransport = 'cellular'; },
    event => { event.afterSnapshot.identitySha256 = digest('changed identity'); },
    event => { event.afterSnapshot.pcStateSha256 = digest('changed PC'); },
    event => { event.afterSnapshot.selectedServerSha256 = digest('changed selection'); },
    event => { event.afterSnapshot.pendingChanges = true; },
    event => { event.probes[0].before.observedAtMs = event.beforeSnapshot.observedAtMs - 1; },
    event => { event.probes[0].after.observedAtMs = event.afterSnapshot.observedAtMs + 1; },
    event => { event.probes[0].startedAtMs = event.probes[0].before.observedAtMs - 1; },
    event => { event.probes[1].before.observedAtMs = event.probes[0].after.observedAtMs; },
    event => { [event.beforeSnapshot, event.afterSnapshot] = [event.afterSnapshot, event.beforeSnapshot]; }]) {
    const { plan, receipt } = fixture(); mutate(receipt.events[2]);
    assert.throws(() => assessBatch(plan, receipt));
  }
});
test('plan, private manifest, copied DEX and every receipt/counter pin the same build', () => {
  const { plan, receipt } = fixture();
  const bytes = privateManifestBytes(plan);
  assert.equal(sha256(bytes), plan.planSha256);
  assert.deepEqual(verifyWitnessBytes(plan, bytes, Buffer.from('mock-dex')), receipt.witness);
  assert.throws(() => verifyWitnessBytes(plan, Buffer.concat([bytes, Buffer.from('\n')]), Buffer.from('mock-dex')));
  assert.throws(() => verifyWitnessBytes(plan, bytes, Buffer.from('different dex')));
  const paths = `cache/polaris-qa-${plan.nonce}`;
  const output = `${plan.planSha256}  ${paths}/manifest.json\n${plan.witnessBuild.dexSha256}  ${paths}/witness.dex.jar\n`;
  assert.equal(verifyCopiedHashOutput(plan, output).copiedHashesMatch, true);
  assert.throws(() => verifyCopiedHashOutput(plan, output.replace(plan.nonce, 'a'.repeat(48))));
  assert.throws(() => verifyCopiedHashOutput(plan, output.replace(plan.witnessBuild.dexSha256, digest('old jar'))));
  for (const mutate of [r => { r.witness.dexSha256 = digest('old jar'); },
    r => { r.events[1].probes[0].before.manifestSha256 = digest('old manifest'); },
    r => { r.events[1].probes[0].after.buildReceiptSha256 = digest('old build'); }]) {
    const changed = structuredClone(receipt); mutate(changed);
    assert.throws(() => assessBatch(plan, changed));
  }
  const changed = structuredClone(plan); changed.witnessBuild.dexSha256 = digest('new unapproved dex');
  assert.throws(() => validatePlan(changed));
});
test('planned PC counter file is new, nonce-owned and confined to an operator-owned 0700 QA root', () => {
  const root = mkdtempSync(join(tmpdir(), 'polaris-qa-root-'));
  const { plan } = fixture(root);
  const path = join(root, `pc-counters-${plan.nonce}.json`);
  try {
    assert.equal(validateCounterFile(plan, path), path);
    assert.throws(() => validateCounterFile(plan, join(tmpdir(), `pc-counters-${plan.nonce}.json`)));
    assert.throws(() => validateCounterFile(plan, join(root, 'another-counter.json')));
    chmodSync(root, 0o755);
    assert.throws(() => validateCounterFile(plan, path), /0700/);
    chmodSync(root, 0o700);
    writeFileSync(path, '{}');
    assert.throws(() => validateCounterFile(plan, path), /reused/);
    rmSync(path);
    symlinkSync(join(root, 'missing-target'), path);
    assert.throws(() => validateCounterFile(plan, path), /reused/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
test('complete CLI fixture exits NOT_READY and the source exposes no live sockets', () => {
  const dir = mkdtempSync(join(tmpdir(), 'polaris-qa-complete-'));
  const { plan, receipt } = fixture(dir);
  try {
    writePrivateJson(join(dir, 'plan.json'), plan); writePrivateJson(join(dir, 'receipt.json'), receipt);
    const result = spawnSync(process.execPath, [new URL('./android-batch-qa.mjs', import.meta.url).pathname,
      'assess', join(dir, 'plan.json'), join(dir, 'receipt.json')], { encoding: 'utf8' });
    assert.equal(result.status, 2);
    const output = JSON.parse(result.stdout);
    assert.equal(output.verdict, 'NOT_READY'); assert.deepEqual(output.claims, []);
    const node = readFileSync(new URL('./android-batch-witness.mjs', import.meta.url), 'utf8');
    const java = readFileSync(new URL('./qa/MeshWitness.java', import.meta.url), 'utf8');
    assert.ok(!/node:dgram|createConnection|createServer/.test(node));
    assert.ok(!/java\.net|ServerSocket|DatagramSocket|new Socket/.test(java));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
