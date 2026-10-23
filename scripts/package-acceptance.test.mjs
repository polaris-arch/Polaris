import test from 'node:test';
import vm from 'node:vm';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonical } from './desktop-core/source-graph.mjs';
import { formalWorkflowContext } from './desktop-core/native-dns-gates.mjs';
import { CASES, SCHEMA, admit, artifactName, checkRun, producerContext, selection, sha256,
  validateCarriers, verifyAccepted, verifyCarrierFiles, verifyFreeze, verifyMetadata,
  verifyRemoteAssets, verifyRunEvidence, verifyNativeProof } from './package-acceptance/transport.mjs';

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const sha = 'a'.repeat(40), tree = 'b'.repeat(40), hash = 'c'.repeat(64);
const env = { GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: 'polaris-arch/Polaris', GITHUB_REPOSITORY_ID: '1347028220',
  GITHUB_SHA: sha, GITHUB_EVENT_NAME: 'push', GITHUB_REF: 'refs/tags/v1.0.0', GITHUB_REF_NAME: 'v1.0.0',
  GITHUB_WORKFLOW_REF: 'polaris-arch/Polaris/.github/workflows/package.yml@refs/tags/v1.0.0',
  GITHUB_RUN_ID: '120', GITHUB_RUN_ATTEMPT: '2', PACKAGE_ACCEPTANCE_PLATFORM: 'all' };
const git = (_program, args) => {
  if (args[0] === 'status') return '';
  if (args[0] === 'show') return readFileSync(join(ROOT, args[1].slice(5)), 'utf8');
  return args[1] === 'HEAD' ? sha : tree;
};
const context = admit(ROOT, env, git);
const stamp = (facts) => ({ ...facts, fingerprint: sha256(canonical(facts)) });
const clone = (value) => structuredClone(value);
const carriers = ['deb', 'AppImage'].map((kind) => ({ kind, name: `Polaris_1.0.0_amd64-linux.${kind}`,
  bytes: 4, sha256: sha256('test'), members: Object.fromEntries(['app', 'core', 'cronet'].map((role) =>
    [role, { path: `usr/lib/Polaris/${role}`, bytes: 1, sha256: hash }])) }));
const freeze = stamp({ schema: 'polaris-linux-carrier-freeze-v1', scope: 'producer-byte-record-not-acceptance',
  producerLegacyFlag: '0', context, carriers });
const active = { incarnation: 'owned-instance', appliedRevision: 7, effectiveDigest: hash };
const original = { sha256: hash, bytes: 100, pid: 101, startTime: '124', stableDoubleRead: true,
  sameInodeAndMetadata: true, dashboardEnabled: true, dashboardDetourMatchesFinal: true,
  readStartedUnixNs: '1800000000000000000', readFinishedUnixNs: '1800000000000000001',
  originalBytesRetention: 'private ephemeral runner only; not uploaded or cross-VM archived' };
const flow = backend => ({ backend, bytes: 20, nonceSha256: hash });
const operation = { transactionId: 'owned-transaction', active, before: active, durable: true,
  persistenceMode: 'exact_configuration_file' };
const cleanup = { state: 'normal', forced: [], remaining: [], unknown: [] };
const evidence = {
  default_dashboard: { emptyHome: true, observed: true, defaultGraphics: true, actualGeneratedRuntime: original },
  gui_selection: { selected: 'node-b', before: flow('A'), newConnection: flow('B') },
  gui_manual_disconnect_preference: { interruptPreferenceObserved: true, oldBClosed: true, gracefulOldConnection: flow('A') },
  gui_on_off: { selected: 'node-b', newProcess: true, restoredConnection: flow('B') },
  gui_auto_preserves_existing: { nativeIntervalMinutes: 5, manuallyRequestedSwitch: false, keepaliveExchanges: 2,
    before: flow('A'), oldAfter: flow('A'), newConnection: flow('B') },
  gui_apply_tcp_dns: { appResult: { status: 'hotApplied', transactionId: operation.transactionId }, operation,
    answerTtl: 0, oldBefore: flow('A'), oldAfter: flow('A'), newAfter: flow('B'), acceptedConfig: original },
  gui_durability_status_retry: { pending: { status: 'hotDurabilityPending', transactionId: operation.transactionId },
    recovered: { status: 'hotApplied', transactionId: operation.transactionId }, operation, readOnlyStatusPolls: 3,
    originalSha256: hash, disturbedSha256: 'd'.repeat(64), terminalRetry: operation, acceptedConfig: original },
  gui_rejected_preserves_old: { structuralResult: { status: 'restartRequired' }, explicitRestartDeclined: true,
    rejectedAppResult: { status: 'rejected' }, sourceUnchanged: true, acceptedIdentity: active, oldConnection: flow('B') },
  same_home_restart: { sameHome: true, selected: 'node-a', acceptedBefore: original, restoredConfig: original, restoredDataPlane: flow('B') },
  rpc_contract: { source: 'independent authenticated sideclient, not GUI', methods: ['GetRuntimeCapabilities',
    'PlanRuntimeConfig', 'ApplyRuntimeConfig', 'GetRuntimeConfigStatus', 'RetryRuntimePersistence', 'AcknowledgeRuntimePersistence'],
    unauthenticatedPerMethod: 2, sameTransactionReplay: true, cliRegenerationAckRejected: true,
    wrongDigestReplayRejected: true, stalePlanRejected: true, operation, terminalRetry: operation,
    noOpOperation: operation, before: { active }, after: { active } },
  tray_quit: { entry: 'native WebDriver Ctrl+Q -> original tray_quit', normalProductExit: true,
    identities: [{ pid: 100, startTime: '123' }, { pid: 101, startTime: '124' }],
    cleanExitMarker: { name: 'clean-exit.marker', bytes: 0, sha256: sha256(''), createdAfterShortcut: true,
      mtimeUnixNs: '1800000000000000000', writer: 'original desktop exit commit' } },
  cleanup,
};
const report = { schema: SCHEMA, result: 'success', transportSha256: hash, cleanup: { state: 'normal', forced: [], remaining: [] },
  carriers: carriers.map((r) => ({ kind: r.kind, sha256: r.sha256, result: 'success', cleanup: { state: 'normal', forced: [], remaining: [], unknown: [] },
    cases: Object.fromEntries(CASES.map((key) => [key, { status: 'passed', evidence: clone(evidence[key]) }])),
    execution: { uid: 1001, defaultSandbox: true, originalEntry: true, automation: 'tauri-driver 2.0.5',
      app: { sha256: hash, pid: 100, startTime: '123' }, core: { sha256: hash, pid: 101, startTime: '124' }, cronet: { sha256: hash, mapState: 'mapped-exact-packaged-library' } } })) };
const accepted = stamp({ schema: SCHEMA, result: 'success', scope: 'linux-nonroot-unpacked-no-tun',
  freeze, carrierArtifact: { id: '56', sha256: hash }, transportSha256: hash,
  native: Object.fromEntries(['deb', 'AppImage'].map((kind) => [kind, { sha256: hash, coreSha256: hash, cronetSha256: hash, actualCoreStarted: true, actualCoreMappedSha256: hash, capiEngineInitialized: true }])), run: report });
const expected = { id: '56', name: artifactName('transport', context), sha256: hash };
const metadata = { id: 56, name: expected.name, expired: false, digest: `sha256:${hash}`, size_in_bytes: 100,
  workflow_run: { id: 120, head_sha: sha, repository_id: 1347028220, head_repository_id: 1347028220 } };
const runMetadata = { id: 120, run_attempt: 2, head_sha: sha, repository: { id: 1347028220 },
  head_repository: { id: 1347028220 }, event: 'push', path: '.github/workflows/package.yml', status: 'in_progress', conclusion: null };

test('genuine tag source and direct exact branch are admitted without fake contexts', () => {
  assert.equal(context.tree, tree);
  const branch = { ...env, GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_REF: 'refs/heads/reviewed',
    GITHUB_WORKFLOW_REF: 'polaris-arch/Polaris/.github/workflows/package.yml@refs/heads/reviewed',
    PACKAGE_ACCEPTANCE_REQUESTED: 'true', PACKAGE_ACCEPTANCE_CANDIDATE: sha, PACKAGE_ACCEPTANCE_PLATFORM: 'linux' };
  assert.equal(selection(branch).release, false);
  assert.equal(admit(ROOT, branch, git).candidate, sha);
  for (const change of [{ PACKAGE_ACCEPTANCE_REQUESTED: 'false' }, { PACKAGE_ACCEPTANCE_CANDIDATE: '' },
    { PACKAGE_ACCEPTANCE_CANDIDATE: 'f'.repeat(40) }, { GITHUB_REF: 'refs/tags/fake' },
    { GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/release-risk.yml@${branch.GITHUB_REF}` }])
    assert.throws(() => admit(ROOT, { ...branch, ...change }, git));
});

for (const [key, value] of Object.entries({ GITHUB_REPOSITORY: 'other/Polaris', GITHUB_REPOSITORY_ID: '1',
  GITHUB_SHA: 'f'.repeat(40), GITHUB_ACTIONS: 'false', GITHUB_EVENT_NAME: 'pull_request',
  GITHUB_REF: 'refs/heads/not-a-tag', GITHUB_RUN_ATTEMPT: '0', PACKAGE_ACCEPTANCE_PLATFORM: 'windows',
  PACKAGE_ACCEPTANCE_LIGHT: 'true', PACKAGE_ACCEPTANCE_NATIVE: 'true', PACKAGE_ACCEPTANCE_SKIP_QUALITY: 'true',
  PACKAGE_ACCEPTANCE_PLATFORMS: '["linux"]', PACKAGE_ACCEPTANCE_BUNDLE: 'legacy-bundle',
  POLARIS_NATIVE_DNS_CANDIDATE: '', POLARIS_SINGBOX_PATH: '', POLARIS_NO_KERNEL_RUN: '1', POLARIS_NATIVE_DNS_FORMAL: '0' }))
  test(`formal admission rejects ${key}`, () => assert.throws(() => admit(ROOT, { ...env, [key]: value }, git)));
for (const value of ['0', '1', '']) test(`formal admission rejects legacy property ${JSON.stringify(value)}`, () => {
  assert.throws(() => formalWorkflowContext(sha, { ...env, POLARIS_NATIVE_PAYLOAD_VALIDATION: value }));
  assert.throws(() => admit(ROOT, { ...env, POLARIS_NATIVE_PAYLOAD_VALIDATION: value }, git));
});
test('dirty, wrong HEAD/tree/committed selection all fail source admission', () => {
  for (const key of ['status', 'HEAD', 'HEAD^{tree}', 'show']) assert.throws(() => admit(ROOT, env, (p, args) =>
    key === args[0] || key === args[1] ? 'wrong' : git(p, args)));
});
test('producer existing legacy=0 records bytes, never gains consumer admission', () => {
  const producer = { ...env, POLARIS_NATIVE_PAYLOAD_VALIDATION: '0', POLARIS_LIGHT_BUILD: '0',
    POLARIS_UPLOAD_ARTIFACTS: '1', POLARIS_RUN_KERNEL_GATES: '1', CORE_CANDIDATE: sha };
  assert.deepEqual(producerContext(ROOT, context, producer, git), context);
  assert.throws(() => admit(ROOT, producer, git));
  for (const [key, value] of [['POLARIS_NATIVE_PAYLOAD_VALIDATION', '1'], ['POLARIS_LIGHT_BUILD', '1'],
    ['CORE_CANDIDATE', 'f'.repeat(40)], ['POLARIS_UPLOAD_ARTIFACTS', '0'], ['POLARIS_RUN_KERNEL_GATES', '0'], ['POLARIS_NATIVE_DNS_FORMAL', '1']])
    assert.throws(() => producerContext(ROOT, context, { ...producer, [key]: value }, git));
  for (const key of ['candidate', 'tree', 'contractSha256', 'runId', 'attempt', 'workflowRef', 'repositoryId'])
    assert.throws(() => producerContext(ROOT, { ...context, [key]: 'wrong' }, producer, git));
});
test('frozen both-carrier member identity, context and fingerprint are mandatory', () => {
  verifyFreeze(freeze, context);
  for (const modify of [x => x.carriers.pop(), x => x.carriers.push(clone(x.carriers[0])),
    x => x.carriers[0].sha256 = 'wrong', x => x.carriers[0].members.app.path = '../escape',
    x => delete x.carriers[0].members.cronet, x => x.context.attempt = '1', x => x.scope = 'formal-pass']) {
    const value = clone(freeze); modify(value); assert.throws(() => verifyFreeze(value, context));
  }
  assert.throws(() => validateCarriers([{ ...carriers[0], name: '../x.deb' }, carriers[1]]));
});
test('artifact API pins bind repository/run/head/ID/digest and exact name', () => {
  verifyMetadata(metadata, context, expected);
  for (const change of [{ id: 57 }, { name: 'old' }, { digest: `sha256:${'d'.repeat(64)}` }, { expired: true },
    { size_in_bytes: 0 }, { workflow_run: { ...metadata.workflow_run, id: 119 } },
    { workflow_run: { ...metadata.workflow_run, head_sha: 'f'.repeat(40) } },
    { workflow_run: { ...metadata.workflow_run, repository_id: 1 } },
    { workflow_run: { ...metadata.workflow_run, head_repository_id: 1 } }])
    assert.throws(() => verifyMetadata({ ...metadata, ...change }, context, expected));
  assert.notEqual(artifactName('transport', { ...context, attempt: '1' }), expected.name);
  checkRun(runMetadata, context, env);
  for (const change of [{ run_attempt: 1 }, { path: '.github/workflows/release-risk.yml' },
    { conclusion: 'cancelled' }, { conclusion: 'failure' }, { head_sha: 'f'.repeat(40) }])
    assert.throws(() => checkRun({ ...runMetadata, ...change }, context, env));
});
test('missing/skipped/cancelled cases, disabled sandbox, different ELF and unknown cleanup never pass', () => {
  verifyRunEvidence(report, freeze); verifyAccepted(accepted, context);
  const mutations = [x => x.result = 'skipped', x => x.cleanup.state = 'unknown',
    x => x.cleanup.forced.push({ pid: 12 }), x => x.cleanup.remaining.push({ pid: 12 }),
    x => x.carriers.pop(), x => x.carriers[0].sha256 = hash,
    x => x.carriers[0].execution.defaultSandbox = false, x => x.carriers[0].execution.uid = 0,
    x => x.carriers[0].execution.originalEntry = false, x => x.carriers[0].execution.core.sha256 = 'd'.repeat(64),
    x => x.carriers[0].execution.app.startTime = '', x => x.carriers[0].execution.automation = 'mock'];
  for (const name of CASES) mutations.push(x => delete x.carriers[0].cases[name],
    x => x.carriers[1].cases[name].status = 'cancelled', x => x.carriers[0].cases[name].evidence = {});
  for (const mutate of mutations) { const value = clone(report); mutate(value); assert.throws(() => verifyRunEvidence(value, freeze)); }
  for (const mutate of [x => x.native.deb = '', x => x.result = 'failure', x => x.carrierArtifact.id = '',
    x => x.freeze.context.attempt = '1']) {
    const value = clone(accepted); mutate(value); assert.throws(() => verifyAccepted(value, context));
  }
});
test('local publication bytes and remote asset digests/sizes stay bound to producer freeze', () => {
  const tmp = mkdtempSync(join(tmpdir(), 'package-custody-test-'));
  try {
    for (const r of carriers) writeFileSync(join(tmp, r.name), 'test');
    verifyCarrierFiles(tmp, carriers, true);
    writeFileSync(join(tmp, carriers[0].name), 'evil'); assert.throws(() => verifyCarrierFiles(tmp, carriers));
    writeFileSync(join(tmp, carriers[0].name), 'test'); mkdirSync(join(tmp, 'duplicate'));
    writeFileSync(join(tmp, 'duplicate', carriers[0].name), 'test'); assert.throws(() => verifyCarrierFiles(tmp, carriers));
  } finally { rmSync(tmp, { recursive: true, force: true }); }
  const assets = carriers.map((r) => ({ name: r.name, size: r.bytes, digest: `sha256:${r.sha256}`, state: 'uploaded' }));
  verifyRemoteAssets(assets, carriers);
  for (const change of [{ digest: `sha256:${hash}` }, { size: 5 }, { state: 'new' }, { name: 'other' }])
    assert.throws(() => verifyRemoteAssets([{ ...assets[0], ...change }, assets[1]], carriers));
  assert.throws(() => verifyRemoteAssets(assets.slice(1), carriers));
});
test('same workflow requires exact acceptance success at both release boundaries', () => {
  const workflow = readFileSync(join(ROOT, '.github/workflows/package.yml'), 'utf8');
  const block = (name) => workflow.split(`\n  ${name}:\n`)[1].split(/\n  [a-z_]+:\n/)[0];
  const setup = block('setup'), producer = block('package'), consumer = block('linux_package_acceptance').split('\n').filter((line) => !/^\s*#/.test(line)).join('\n');
  assert.match(setup, /transport\.mjs admit/);
  assert.ok(producer.indexOf('Normalize Linux release asset names') < producer.indexOf('transport.mjs freeze'));
  assert.ok(producer.indexOf('Harden') < producer.indexOf('transport.mjs freeze'));
  assert.ok(producer.indexOf('id: package_artifact') < producer.indexOf('transport.mjs seal'));
  assert.doesNotMatch(consumer, /POLARIS_NATIVE_PAYLOAD_VALIDATION:|POLARIS_SINGBOX_PATH:|light_build:|tauri build|vite|playwright/i);
  assert.match(consumer, /fetch-core\.mjs --platform=linux/);
  assert.match(consumer, /final-package-receipt\.py --platform linux --kind deb/);
  assert.match(consumer, /final-package-receipt\.py --platform linux --kind AppImage/);
  assert.match(consumer, /linux\.py run/);
  assert.ok(consumer.indexOf('linux.py run') < consumer.indexOf('transport.mjs accept'));
  for (const name of ['release_desktop', 'release']) {
    const source = block(name);
    assert.match(source, /needs\.linux_package_acceptance\.result == 'success'/);
    assert.doesNotMatch(source, /needs\.linux_package_acceptance\.result (?:!=|== 'skipped')/);
    assert.match(source, /PACKAGE_ACCEPTED_ARTIFACT_ID: \$\{\{ needs\.linux_package_acceptance\.outputs\.artifact_id \}\}/);
    assert.match(source, /PACKAGE_ACCEPTED_ARTIFACT_DIGEST: \$\{\{ needs\.linux_package_acceptance\.outputs\.artifact_digest \}\}/);
    assert.match(source, /actions: read/);
    assert.match(source, /transport\.mjs release-check/);
  }
  assert.match(block('release'), /PACKAGE_CHECK_REMOTE: 'true'/);
  assert.ok(block('release').lastIndexOf('transport.mjs release-check') < block('release').indexOf('--draft=false'));
  const callInputs = workflow.split('\n  workflow_call:\n')[1].split('\npermissions:')[0];
  assert.doesNotMatch(callInputs, /linux_package_acceptance:|linux_acceptance_candidate:/);
  assert.doesNotMatch(workflow, /unset POLARIS_NATIVE|overwrite: true/);
});
test('bounded Python transport and supervisor/fixture unit contracts (never App execution)', () => {
  for (const file of ['test_linux.py', 'test_fixtures.py', 'test_transport_archive.py'])
    assert.ok(readFileSync(join(ROOT, 'scripts/package-acceptance', file)).length > 0, 'required test suite missing');
  const output = execFileSync(process.platform === 'win32' ? 'python' : 'python3', ['-B', '-m', 'unittest', 'discover',
    '-s', 'scripts/package-acceptance', '-p', 'test_*.py', '-v'], { cwd: ROOT, encoding: 'utf8', timeout: 90_000, maxBuffer: 2 * 1024 ** 2 });
  assert.equal(typeof output, 'string');
});

test('original final receipts must prove actual frozen core and Cronet CAPI/load, not an empty native claim', () => {
  const row = carriers[0];
  const proof = { schema: 'polaris-final-package-proof-v1', artifact: { kind: row.kind, sha256: row.sha256, bytes: row.bytes },
    transport: { candidate: context.candidate, runId: context.runId, attempt: context.attempt },
    relativeCore: row.members.core.path, relativeLibrary: row.members.cronet.path,
    native: { schema: 'polaris-native-payload-proof-v1', input: { core: { sha256: hash }, library: { sha256: hash } },
      actualCore: { started: true, mappedLibrary: { sha256: hash } }, capi: { engineInitialized: true, librarySha256: hash } } };
  verifyNativeProof(proof, row, context);
  for (const change of [x => x.native = {}, x => x.native.actualCore.started = false,
    x => x.native.actualCore.mappedLibrary.sha256 = '0'.repeat(64), x => x.native.capi.engineInitialized = false,
    x => x.transport.attempt = '1', x => x.native.input.core.sha256 = '0'.repeat(64)]) {
    const changed = clone(proof); change(changed); assert.throws(() => verifyNativeProof(changed, row, context));
  }
});


test('case-specific mandatory execution facts cannot be replaced by generic passed labels', () => {
  for (const [key, value] of Object.entries(evidence)) for (const field of Object.keys(value)) {
    const row = clone(report); delete row.carriers[0].cases[key].evidence[field];
    assert.throws(() => verifyRunEvidence(row, freeze), `${key}.${field} must be present`);
  }
  for (const [key, field] of [['default_dashboard','observed'], ['tray_quit','normalProductExit'],
    ['same_home_restart','sameHome'], ['rpc_contract','cliRegenerationAckRejected']]) {
    const row = clone(report); row.carriers[0].cases[key].evidence[field] = false;
    assert.throws(() => verifyRunEvidence(row, freeze));
  }
  const row = clone(report); row.carriers[0].cases.rpc_contract.evidence.methods.pop();
  assert.throws(() => verifyRunEvidence(row, freeze));
  for (const mutate of [p => { delete p.operation.active; delete p.terminalRetry.active; },
    p => { p.operation.active.effectiveDigest = 'invalid'; p.terminalRetry.active.effectiveDigest = 'invalid'; },
    p => { p.terminalRetry.transactionId = 'different'; }]) {
    const changed = clone(report), proof = changed.carriers[0].cases.gui_durability_status_retry.evidence;
    // Separate records, as emitted by the independent real sideclient responses.
    proof.terminalRetry = clone(proof.terminalRetry); mutate(proof);
    assert.throws(() => verifyRunEvidence(changed, freeze));
  }

});

const observer = readFileSync(join(ROOT, 'scripts/package-acceptance/linux.py'), 'utf8').match(/OBSERVER = r'''\n([\s\S]*?)\n'''/)[1];
function bridge() {
  const callbacks = new Map(); let next = 1;
  const register = (fn) => { const id = next++; callbacks.set(id, data => { callbacks.delete(id); return fn(data); }); return id; };
  const internals = {};
  const run = (id, data) => { const callback = callbacks.get(id); return callback && callback(data); };
  const invoke = (cmd, payload) => new Promise((resolve, reject) => {
    const success = register(value => { callbacks.delete(failure); resolve(value); });
    const failure = register(value => { callbacks.delete(success); reject(value); });
    queueMicrotask(() => run(cmd === 'fail' ? failure : success, payload));
  });
  for (const [key, value] of Object.entries({ invoke, callbacks, runCallback: run, transformCallback: register }))
    Object.defineProperty(internals, key, { value }); // Real immutable descriptors.
  const window = { __TAURI_INTERNALS__: internals };
  return { window, callbacks, invoke, run, install: () => vm.runInNewContext(`(function(){${observer}})()`, { window, Map }) };
}
const reply = { success: true, data: { status: 'hotApplied', ok: true, pendingApply: false,
  restartRequired: false, reasons: [], transactionId: 'actual-transaction', secret: 'never-record' } };
test('immutable native bridge observer preserves async resolve/reject, once deletion and original values', async () => {
  const b = bridge(); assert.equal(b.install(), true);
  assert.equal(b.window.__TAURI_INTERNALS__.invoke, b.invoke);
  assert.equal(Object.getOwnPropertyDescriptor(b.window.__TAURI_INTERNALS__, 'invoke').writable, false);
  assert.equal(await b.invoke('actual', reply), reply);
  assert.equal(b.callbacks.size, 0);
  assert.equal(b.window.__packageObservation.rows.length, 1);
  assert.equal(b.window.__packageObservation.rows[0].source, 'original-native-callback');
  assert.equal(b.window.__packageObservation.rows[0].transactionId, 'actual-transaction');
  assert.ok(!JSON.stringify(b.window.__packageObservation).includes('never-record'));
  const failure = { secret: 'never-record-error' };
  await assert.rejects(b.invoke('fail', failure), error => error === failure);
  assert.equal(b.callbacks.size, 0); assert.equal(b.window.__packageObservation.rows.length, 1);
  assert.equal(b.run(999, reply), undefined);
  const receiver = {}; const args = [reply, 'unchanged'];
  b.callbacks.set(1000, function(...actual) { assert.equal(this, receiver); assert.deepEqual(actual, args); return 42; });
  assert.equal(b.callbacks.get(1000).apply(receiver, args), 42);
  b.callbacks.set(1001, () => { throw failure; });
  assert.throws(() => b.run(1001, null), error => error === failure);
  assert.throws(b.install); // No silent duplicate hook.
});
test('callback observation is shape-limited, monotonic and bounded; immutable incompatibility fails', async () => {
  const b = bridge(); b.install();
  await b.invoke('read-config', { success: true, data: { status: 'saved', secret: 'private' } });
  assert.equal(b.window.__packageObservation.rows.length, 0);
  for (let i = 0; i < 520; i++) await b.invoke('actual', reply);
  const observed = b.window.__packageObservation;
  assert.equal(observed.cursor, 520); assert.equal(observed.rows.length, 512);
  assert.equal(observed.rows[0].sequence, 9); assert.equal(observed.rows.at(-1).sequence, 520);
  assert.equal(observed.failed, false);
  const bad = bridge(); Object.preventExtensions(bad.callbacks); assert.throws(bad.install);
  const wrong = { __TAURI_INTERNALS__: { invoke: () => {}, callbacks: new Map() } };
  assert.throws(() => vm.runInNewContext(`(function(){${observer}})()`, { window: wrong, Map }));
});

test('both Linux consumers prove installed drivers without unsupported version probes', () => {
  const workflow = readFileSync(join(ROOT, '.github/workflows/package.yml'), 'utf8');
  for (const name of ['linux_package_acceptance', 'linux_tun_package_acceptance']) {
    const body = workflow.split(`\n  ${name}:\n`)[1].split(/\n  [a-z][a-z_]+:\n/)[0];
    assert.match(body, /cargo install --root "\$RUNNER_TEMP\/polaris-package-automation" --version '=2\.0\.5' --locked tauri-driver/);
    assert.match(body, /tooling\.py prepare --root "\$RUNNER_TEMP\/polaris-package-automation" --output "\$RUNNER_TEMP\/polaris-package-automation\/receipt\.json"/);
    assert.doesNotMatch(body, /(?:tauri-driver|WebKitWebDriver) --(?:version|help)/);
    assert.match(body, /Record bounded workflow diagnostic scope \(not acceptance\)\n        if: always\(\)/);
    const scope = body.split('Record bounded workflow diagnostic scope (not acceptance)')[1].split('      - name:')[0];
    for (const id of ['checkout', 'admission', 'native_tools', 'test_tools', 'protoc', 'core_go', 'core_receipts', 'transport', 'native_receipt', 'rpc_client', 'runtime', 'verification']) {
      assert.match(body, new RegExp(`        id: ${id}\\n`));
      assert.ok(scope.includes(`steps.${id}.outcome`), `${name}: missing diagnostic ${id}`);
    }
    assert.doesNotMatch(scope, /toJSON|continue-on-error|run\.json|accepted\.json/);
    assert.match(body, /if-no-files-found: error/);
    const lane = name === 'linux_package_acceptance' ? 'plain' : 'tun';
    assert.ok(scope.includes(`diagnostic_scope.py --lane ${lane} --kind `));
    if (lane === 'tun') assert.ok(scope.includes('steps.binding.outcome'));
  }
  const harness = readFileSync(join(ROOT, 'scripts/package-acceptance/linux.py'), 'utf8');
  assert.doesNotMatch(harness, /\[driver,\s*['"]--version['"]\]/);
  for (const suite of ['test_tooling.py', 'test_diagnostic_scope.py'])
    assert.ok(readFileSync(join(ROOT, 'scripts/package-acceptance', suite)).length > 0);
});


test('read-only auth preflight is one existing setup job and default DAG is byte-preserved', () => {
  const workflow = readFileSync(join(ROOT, '.github/workflows/package.yml'), 'utf8');
  const names = [...workflow.slice(workflow.indexOf('\njobs:\n')).matchAll(/^  ([a-z_]+):$/gm)].map(m => m[1]);
  assert.deepEqual(names, ['setup','ci','ui','desktop_core','package','linux_package_acceptance',
    'linux_tun_package_acceptance','core_wire','release_desktop','android_release','release']);
  for (const name of names.slice(1)) {
    const block = workflow.split(`\n  ${name}:\n`)[1].split(/\n  [a-z_]+:\n/)[0];
    assert.match(block, /^    if: (?:>-\n      )?inputs\.linux_auth_preflight_only != true/m, name);
  }
  const setup = workflow.split('\n  setup:\n')[1].split('\n  ci:\n')[0];
  assert.match(setup, /runs-on: ubuntu-22\.04/);
  assert.match(setup, /if: always\(\) && inputs\.linux_auth_preflight_only == true\n        uses: actions\/upload-artifact@v7/);
  assert.match(setup, /if-no-files-found: error/);
  assert.match(setup, /test -z "\$\(git status --porcelain --untracked-files=all\)"/);
  assert.match(setup, /python3 -B scripts\/package-acceptance\/diagnostic_scope\.py --auth-preflight-only/);
  assert.doesNotMatch(workflow.split('  workflow_call:')[1].split('\npermissions:')[0], /linux_auth_preflight_only/);
  // Reverse only the added default-false guards/input/steps/ref selection. The
  // complete resulting workflow must equal the reviewed f20ce source hash:
  // dependencies, matrices, original conditions (including always), and bodies.
  // N5 adds only one opt-in choice and two exact descriptions. Reverse those
  // literals first; retain the original f20ce byte hash rather than widening it.
  let projected = workflow
    .replace('          - temporary-existing-runner-password-and-sudo-group\n', '')
    .replace("        description: 'Temporary password/group modes require separate exact-SHA approval; existing session remains default'",
      "        description: 'Temporary password mode requires separate explicit approval for this exact dispatch'")
    .replace("        description: 'Leave empty unless the exact selected password/relock or password/sudo-membership/relock/removal action was approved'",
      "        description: 'Leave empty unless this exact temporary runner password and relock action was explicitly approved'");
  projected = projected.replace(/      linux_auth_preflight_only:\n        description: [^\n]+\n        type: boolean\n        default: false\n/, '');
  projected = projected.replace('inputs.linux_auth_preflight_only && github.sha || ', '');
  projected = projected.replace(/^        if: inputs\.linux_auth_preflight_only == true \|\| \((.*)\)$/gm, '        if: $1');
  projected = projected.replace(/^        if: inputs\.linux_auth_preflight_only != true && \((.*)\)$/gm, '        if: $1');
  projected = projected.replace('        id: resolve\n        if: inputs.linux_auth_preflight_only != true\n', '        id: resolve\n');
  projected = projected.replace(/\n      - name: Read-only exact-source account group preflight\n[\s\S]*?          retention-days: 7\n\n/, '');
  projected = projected.replace(/^    if: inputs\.linux_auth_preflight_only != true && \((.*)\)$/gm, '    if: $1');
  projected = projected.replace(/^    if: >-\n      inputs\.linux_auth_preflight_only != true && \(([\s\S]*?)\)\n(?=    [^ ])/gm, '    if: >-\n      $1\n');
  projected = projected.replace('  android_release:\n    if: inputs.linux_auth_preflight_only != true\n', '  android_release:\n');
  assert.equal(sha256(projected), '0983b4a8580cd9e31b09899d30818928a72dbf53fc60523da9a235ebd3e3a995');
});
