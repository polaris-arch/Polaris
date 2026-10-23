import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { sha256 } from './package-acceptance/transport.mjs';
import { selectedCoreManifest } from './lib/core-identity.mjs';
import { AUTH_MODE, AUTH_PHRASE, CASES, KERNEL, VERSION, authorizationFacts, credentialAuthorization,
  stamp, tunSelection, unstamped, verifyAuthenticationAdmission, verifyAuthorization, verifyHelper, verifyKernel, verifyReport } from './package-acceptance/tun_transport.mjs';
const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const sha = 'a'.repeat(40), tree = 'b'.repeat(40), hash = 'c'.repeat(64);
const env = { GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: 'polaris-arch/Polaris', GITHUB_REPOSITORY_ID: '1347028220',
  GITHUB_SHA: sha, GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_REF: 'refs/heads/reviewed', GITHUB_ACTOR: 'reviewer',
  GITHUB_TRIGGERING_ACTOR: 'reviewer', GITHUB_RUN_ID: '120', GITHUB_RUN_ATTEMPT: '1',
  GITHUB_WORKFLOW_REF: 'polaris-arch/Polaris/.github/workflows/package.yml@refs/heads/reviewed',
  PACKAGE_ACCEPTANCE_PLATFORM: 'linux', PACKAGE_ACCEPTANCE_REQUESTED: 'true', PACKAGE_ACCEPTANCE_CANDIDATE: sha,
  PACKAGE_TUN_REQUESTED: 'true', RUNNER_ENVIRONMENT: 'github-hosted' };
const context = { repository: env.GITHUB_REPOSITORY, repositoryId: env.GITHUB_REPOSITORY_ID,
  candidate: sha, tree, runId: '120', attempt: '1', workflowRef: env.GITHUB_WORKFLOW_REF };
const event = { repository: { full_name: context.repository, id: Number(context.repositoryId) },
  sender: { login: 'reviewer' }, ref: 'reviewed', inputs: { linux_acceptance_candidate: sha,
    linux_tun_acceptance: 'true', linux_package_acceptance: 'true', linux_tun_auth_mode: 'existing-session', linux_tun_credential_authorization: '' } };
const clone = value => structuredClone(value);
const core = { member: 'sing-box', bytes: 4, sha256: hash }, sidecar = { member: 'libcronet.so', bytes: 5, sha256: 'd'.repeat(64) };
const contract = { repository: 'polaris-arch/sing-box-polaris', sourceCommit: KERNEL, version: VERSION, tag: 'v' + VERSION,
  platforms: { linux: { core, sidecar, providerFingerprint: 'f'.repeat(64) } } };
const carrier = { kind: 'deb', name: 'Polaris_1.0.0_amd64-linux.deb', sha256: hash, bytes: 99,
  members: { app: { path: 'usr/bin/polaris', sha256: hash, bytes: 3 },
    core: { path: 'usr/lib/Polaris/resources/linux/sing-box', sha256: core.sha256, bytes: core.bytes },
    cronet: { path: 'usr/lib/Polaris/resources/linux/libcronet.so', sha256: sidecar.sha256, bytes: sidecar.bytes } } };
const inspection = { helper: { path: 'usr/lib/Polaris/resources/linux/polaris-helper', bytes: 6, sha256: hash },
  manifest: { path: 'usr/lib/Polaris/resources/linux/core-manifest.json', bytes: 512, sha256: hash },
  core: carrier.members.core, cronet: carrier.members.cronet, embeddedManifest: selectedCoreManifest(contract, 'linux', '1.0.0', hash) };
const subject = { guiUid: 1001, account: 'runner' };
const facts = authorizationFacts(context, event, env);
const admission = { context, carrier, transportSha256: hash,
  authentication: { mode: facts.mode, execution: facts.execution, subject } };
const cleanup = { state: 'normal', forced: false, unknown: [], watchdogCancelled: true, networkRestored: true,
  productUninstalled: true, privilegedProcessesReaped: true, authenticationWindowClosed: true };
const report = { schema: 'polaris-linux-tun-acceptance-v1', result: 'success', publicationEligible: false,
  scope: 'same-final-carrier', transportSha256: hash, admissionSha256: hash, bindingSha256: hash,
  carrierKind: carrier.kind, carrierSha256: carrier.sha256, context, blocked: [], invalidCases: [], missingCases: [],
  cleanup, cases: Object.fromEntries(CASES.map(name => [name, { status: 'passed', evidence: {} }])) };

const binding = stamp({ schema: 'polaris-linux-tun-bound-v1', admission,
  helperBinding: { helper: inspection.helper, manifest: inspection.manifest, embeddedManifest: inspection.embeddedManifest }, publicationEligible: false });
Object.assign(report, { binding, bindingSha256: sha256(JSON.stringify(binding, null, 2) + '\n'),
  execution: { uid: 1001, defaultSandbox: true, originalEntry: true, startedViaHelper: true, automation: 'tauri-driver 2.0.5',
    app: { sha256: hash, pid: 100, startTime: '123', exeDev: 42, exeInode: 500 }, core: { sha256: hash, pid: 101, startTime: '124' },
    rootAttestation: { executable: { sha256: hash }, identity: { pid: 101, startTime: '124', uid: 1001,
      capAmb: 0x3400, capEff: 0x3400, cgroup: '0::/system.slice/polaris-helper.service' } } },
  rootGuiBinding: { identity: { pid: 100, startTime: '123', uid: 1001, capAmb: 0, capEff: 0, exeDev: 42, exeInode: 500 },
    executable: { dev: 42, inode: 500, sha256: hash }, ancestorBirths: [[100, '123'], [90, '120']] },
  controlConnectivity: Object.fromEntries(['before', 'after'].map((position, i) => [position, {
    host: 'api.github.com', position, dnsResolved: true, httpsStatus: 200, tlsVerified: true, addresses: ['140.82.112.5'],
    boundedResponseBytes: 8192, boundedResponseSha256: hash, readStartedUnixNs: String(1000 + i * 100),
    readFinishedUnixNs: String(1001 + i * 100), freshProcess: { pid: 201 + i, startTime: String(2000 + i) } }])),
  helperIdentity: { identity: { uid: 0, cgroup: '0::/system.slice/polaris-helper.service' }, executable: { sha256: hash } },
  off: { stopped: true, identity: { pid: 101, startTime: '124' } }, productUninstalled: { productUninstalled: true },
  trayQuit: { app: { pid: 100, startTime: '123' }, coreAlreadyStopped: { pid: 101, startTime: '124' },
    cleanExitMarker: { name: 'clean-exit.marker', bytes: 0, sha256: sha256(''), createdAfterShortcut: true } },
  ordinaryCleanup: { state: 'normal', forced: [], remaining: [], unknown: [] },
  defaultObservation: { emptyHome: true, dashboardEnabled: true },
  configIdentity: { sha256: hash, bytes: 100, pid: 101, startTime: '124', stableDoubleRead: true,
    sameInodeAndMetadata: true, dashboardEnabled: true, dashboardDetourMatchesFinal: true,
    readStartedUnixNs: '1800000000000000000', readFinishedUnixNs: '1800000000000000001',
    originalBytesRetention: 'private ephemeral runner only; not uploaded or cross-VM archived' } });
report.cases.default_graph.evidence.actualGraphSha256 = hash;
const namespaces = { net: 40, mnt: 41, pid: 42, cgroup: 43 };
Object.assign(report.rootGuiBinding.identity, { ppid: 90, namespaces, cgroup: '0::/user.slice/runner' });
Object.assign(report.execution.rootAttestation.identity, { ppid: 99, exeDev: 42, exeInode: 600, namespaces });
Object.assign(report.execution.rootAttestation.executable, { dev: 42, inode: 600 });
Object.assign(report.execution.core, report.execution.rootAttestation.identity);
Object.assign(report.helperIdentity.identity, { pid: 99, startTime: '122', ppid: 1,
  exeDev: 42, exeInode: 550, capEff: 0x1fffff, capAmb: 0, namespaces });
Object.assign(report.helperIdentity.executable, { dev: 42, inode: 550 });


const authorization = kind => ({ callback: { envelopeSuccess: true, success: true, installed: kind === 'install', ready: kind === 'install' },
  authorization: { kind, subject: { pid: 100, startTime: '123', uid: 1001 }, authenticationMode: 'existing-session',
    authorizationObserved: true, closed: true, credentialPromptClaimed: false,
    pkexecBirths: [[102, '125']], rootShellBirths: [[102, '125']], owned: [{ uid: 0, ppid: 100, pid: 102, startTime: '125' }] } });
report.helperInstall = authorization('install'); report.helperUninstall = authorization('uninstall');

test('TUN is separately selected direct dispatch and never a publication receipt', () => {
  assert.equal(tunSelection(env).publicationEligible, false);
  for (const changed of [ { GITHUB_EVENT_NAME: 'push' }, { PACKAGE_TUN_REQUESTED: 'false' },
    { PACKAGE_ACCEPTANCE_REQUESTED: 'false' }, { PACKAGE_ACCEPTANCE_CANDIDATE: '' },
    { PACKAGE_ACCEPTANCE_CANDIDATE: 'e'.repeat(40) }, { GITHUB_WORKFLOW_REF: 'foreign' },
    { GITHUB_REPOSITORY: 'fork/Polaris' }, { GITHUB_REF: 'refs/pull/2/merge' },
    { PACKAGE_ACCEPTANCE_LIGHT: 'true' }, { PACKAGE_ACCEPTANCE_NATIVE: 'true' },
    { PACKAGE_ACCEPTANCE_SKIP_QUALITY: 'true' }, { PACKAGE_ACCEPTANCE_BUNDLE: 'fake' },
    { RUNNER_ENVIRONMENT: 'self-hosted' } ]) assert.throws(() => tunSelection({ ...env, ...changed }));
});
test('old formal 814 or renamed version cannot satisfy TUN c6 admission', () => {
  assert.equal(verifyKernel(contract), contract);
  for (const change of [{ sourceCommit: '814da362838e96e54f904836a92c2b0d8c3ae29b' },
    { repository: 'fork/kernel' }, { version: '1.15.0-polaris-alpha.12.1' }, { tag: 'vwrong' }])
    assert.throws(() => verifyKernel({ ...contract, ...change }));
});
test('credential permission comes from exact actual dispatch, not a GUI boolean', () => {
  assert.equal(credentialAuthorization(authorizationFacts(context, event, env), { runToken: 'abcdef123456', guiUid: 1001, account: 'runner' }, 1000), null);
  const approved = clone(event); approved.inputs.linux_tun_auth_mode = AUTH_MODE; approved.inputs.linux_tun_credential_authorization = AUTH_PHRASE;
  const facts = authorizationFacts(context, approved, env);
  const lease = credentialAuthorization(facts, { runToken: 'abcdef123456', guiUid: 1001, account: 'runner' }, 1000);
  assert.deepEqual(lease.transactions, ['install', 'uninstall']); assert.equal(lease.maxLeaseSeconds, 90);
  assert.equal(lease.expiresUnix, 1900); assert.deepEqual(lease.execution, facts.execution);
  assert.deepEqual(verifyAuthenticationAdmission(admission, authorizationFacts(context, event, env)), subject);
  for (const mutate of [v => delete v.authentication, v => v.authentication.subject.guiUid = 0,
    v => v.authentication.execution.actor = 'foreign', v => v.authentication.execution.eventSha256 = hash,
    v => v.authentication.mode = AUTH_MODE, v => delete v.authentication.subject.account]) {
    const bad = clone(admission); mutate(bad); assert.throws(() => verifyAuthenticationAdmission(bad, authorizationFacts(context, event, env)));
  }
  for (const mutate of [v => v.sender.login = 'other', v => v.repository.id++, v => v.ref = 'main',
    v => v.inputs.linux_acceptance_candidate = '0'.repeat(40), v => v.inputs.linux_tun_acceptance = 'false',
    v => v.inputs.linux_tun_credential_authorization = 'true', v => v.inputs.linux_tun_auth_mode = 'sudo-shortcut']) {
    const bad = clone(approved); mutate(bad); assert.throws(() => authorizationFacts(context, bad, env));
  }
  assert.throws(() => authorizationFacts({ ...context, attempt: '2' }, approved, env));
  assert.throws(() => authorizationFacts(context, approved, { ...env, GITHUB_TRIGGERING_ACTOR: 'other' }));
  assert.throws(() => credentialAuthorization(facts, { runToken: '../x', guiUid: 0, account: 'root' }, 1000));
  const unused = clone(event); unused.inputs.linux_tun_credential_authorization = AUTH_PHRASE;
  assert.throws(() => authorizationFacts(context, unused, env));
});
test('helper identity derives from same carrier and original selected manifest', () => {
  assert.equal(verifyHelper(inspection, carrier, contract, '1.0.0').helper.sha256, hash);
  for (const mutate of [v => v.helper.sha256 = 'e'.repeat(64), v => v.helper.path = '/usr/bin/helper',
    v => v.helper.bytes = 0, v => v.manifest.bytes = 65537, v => v.manifest.path = '../core-manifest.json',
    v => v.core.sha256 = 'e'.repeat(64), v => v.cronet.bytes++, v => v.embeddedManifest.appVersion = '2.0.0',
    v => v.embeddedManifest.core.version = 'old', v => v.embeddedManifest.core.providerFingerprint = hash,
    v => v.embeddedManifest.id = hash]) {
    const bad = clone(inspection); mutate(bad); assert.throws(() => verifyHelper(bad, carrier, contract, '1.0.0'));
  }
});
test('report transport identity and cleanup are fail-closed before strict Python cases', () => {
  assert.equal(verifyReport(report, admission, hash), report);
  for (const mutate of [v => v.result = 'BLOCKED', v => v.result = 'cancelled', v => v.publicationEligible = true,
    v => v.transportSha256 = 'd'.repeat(64), v => v.admissionSha256 = 'd'.repeat(64), v => v.carrierKind = 'AppImage',
    v => v.carrierSha256 = 'd'.repeat(64), v => v.context.attempt = '2', v => v.blocked.push('authentication'),
    v => delete v.cases.graph_apply, v => v.cases.graph_apply.status = 'skipped', v => v.cleanup.forced = true,
    v => v.cleanup.unknown.push('root-birth'), v => v.cleanup.networkRestored = false,
    v => v.cleanup.authenticationWindowClosed = false, v => v.cleanup.privilegedProcessesReaped = false,
    v => v.execution.core.sha256 = 'e'.repeat(64), v => v.execution.rootAttestation.identity.capAmb = 0,
    v => v.defaultObservation.emptyHome = false, v => v.configIdentity.dashboardEnabled = false,
    v => v.binding.admission.context.attempt = '2', v => v.bindingSha256 = hash,
    v => delete v.helperInstall, v => delete v.helperUninstall, v => delete v.trayQuit,
    v => v.helperInstall.authorization.rootShellBirths = [[999, '999']],
    v => v.helperUninstall.authorization.subject.pid++, v => v.helperUninstall.authorization.closed = false,
    v => v.ordinaryCleanup.forced.push(102), v => v.off.stopped = false,
    v => delete v.rootGuiBinding, v => v.rootGuiBinding.identity.uid = 0,
    v => v.rootGuiBinding.identity.pid++, v => v.rootGuiBinding.identity.exeInode++,
    v => v.rootGuiBinding.executable.sha256 = 'e'.repeat(64), v => v.rootGuiBinding.executable.inode++,
    v => v.rootGuiBinding.ancestorBirths = [],
    v => delete v.helperIdentity.identity.pid, v => delete v.helperIdentity.identity.startTime,
    v => v.helperIdentity.executable.inode++, v => v.execution.core.ppid++,
    v => v.execution.rootAttestation.identity.ppid++, v => v.execution.rootAttestation.executable.inode++,
    v => { delete v.execution.core.namespaces; delete v.execution.rootAttestation.identity.namespaces; },
    v => { delete v.execution.core.exeDev; delete v.execution.rootAttestation.identity.exeDev; },
    v => v.rootGuiBinding.identity.namespaces = { ...v.rootGuiBinding.identity.namespaces, net: 99 }, v => delete v.execution.app.exeDev,
    v => delete v.controlConnectivity, v => delete v.controlConnectivity.after,
    v => v.controlConnectivity.after.dnsResolved = false, v => v.controlConnectivity.after.tlsVerified = false,
    v => v.controlConnectivity.after.httpsStatus = 302, v => v.controlConnectivity.after.addresses = [],
    v => v.controlConnectivity.after.freshProcess = v.controlConnectivity.before.freshProcess,
    v => v.controlConnectivity.after.readStartedUnixNs = '1', v => delete v.controlConnectivity.after.boundedResponseSha256]) {
    const bad = clone(report); mutate(bad); assert.throws(() => verifyReport(bad, admission, hash));
  }
});
test('temporary credential permission is never substituted for actual authorization/relock', () => {
  const row = authorization('install'); row.authorization.authenticationMode = AUTH_MODE;
  assert.throws(() => verifyAuthorization(row, 'install', report.execution.app, 1001, AUTH_MODE, subject));
  Object.assign(row.authorization, { credentialAuthenticationComplete: true, authDescendantsReaped: true,
    credentialRecovery: { guiUid: 1001, account: 'runner', finalPasswordStatus: 'L', plaintextPersisted: false,
      emptyPasswordRestored: false, temporaryPasswordHash: 'locked until disposable VM destruction' } });
  verifyAuthorization(row, 'install', report.execution.app, 1001, AUTH_MODE, subject);
  for (const mutate of [r => r.authorization.credentialRecovery.finalPasswordStatus = 'P',
    r => r.authorization.credentialRecovery.guiUid = 0, r => r.authorization.authDescendantsReaped = false,
    r => r.authorization.credentialAuthenticationComplete = false,
    r => r.authorization.credentialRecovery.account = 'other', r => r.authorization.credentialRecovery.emptyPasswordRestored = true,
    r => delete r.authorization.credentialRecovery.temporaryPasswordHash, r => delete r.authorization.credentialRecovery.plaintextPersisted]) {
    const bad = clone(row); mutate(bad); assert.throws(() => verifyAuthorization(bad, 'install', report.execution.app, 1001, AUTH_MODE, subject));
  }
});
test('TUN evidence fingerprints cannot bless mutated manifest data', () => {
  const v = stamp({ schema: 'test', x: 1 }); assert.equal(unstamped(v, 'test').x, 1);
  assert.throws(() => unstamped({ ...v, x: 2 }, 'test')); assert.throws(() => unstamped(v, 'other'));
});
test('workflow closes both exact carriers, original verifiers, GUI and cleanup entrypoints', () => {
  const source = readFileSync(join(ROOT, '.github/workflows/package.yml'), 'utf8');
  const job = source.split('\n  linux_tun_package_acceptance:\n')[1]?.split('\n  # ══ 四平台 wire')[0];
  assert.ok(job); assert.match(job, /kind: \[deb, AppImage\]/); assert.match(job, /fail-fast: false/);
  assert.match(job, /runs-on: ubuntu-22\.04/); assert.match(job, /inputs\.linux_tun_acceptance == true/);
  for (const text of ['transport.mjs fetch', 'final-package-receipt.py', 'tun_transport.mjs prepare',
    'linux_tun.py run', 'tun_transport.mjs finish', 'cargo build --locked -p polaris-singbox-grpc --example package_runtime_probe']) assert.ok(job.includes(text), text);
  assert.ok(job.indexOf('apt-get install') < job.indexOf('linux_tun.py run'));
  assert.doesNotMatch(job.split("\n").filter(line => !line.trimStart().startsWith("#")).join("\n"), /cargo build --release|tauri build|dpkg --install|sysctl|--no-sandbox/);
  assert.match(job, /if: always\(\)/); assert.doesNotMatch(job, /continue-on-error: true/);
  for (const stage of ['draft', 'public']) assert.ok(source.includes(`tun_transport.mjs release-check linux-tun-before-${stage} dist-release`));
  assert.equal(source.match(/inputs\.linux_tun_acceptance != true \|\| needs\.linux_tun_package_acceptance.result == 'success'/g)?.length, 2);
  assert.equal(source.match(/needs: \[[^\n]+linux_tun_package_acceptance\]/g)?.length, 2);
});
test('all TUN pure/mock safety contracts are required by ordinary source CI', () => {
  for (const name of ['test_tun.py', 'test_root_controller.py', 'test_linux_tun.py']) {
    execFileSync('python3', ['-B', join(ROOT, 'scripts/package-acceptance', name), '-v'], { cwd: ROOT, timeout: 120000, maxBuffer: 4 * 1024 ** 2 });
  }
});
test('root probe only adds read-only TUN names without changing original six method inventory', () => {
  const source = readFileSync(join(ROOT, 'crates/singbox-grpc/examples/package_runtime_probe.rs'), 'utf8');
  assert.match(source, /root-read-only-tun-probe-required/); assert.match(source, /non-root-probe-required/);
  assert.match(source, /"tun-capabilities" \| "tun-status"/);
  assert.match(source, /production-core-capabilities-differ/); assert.match(source, /listener-not-owned-by-core/);
  assert.match(source, /const METHODS: \[&str; 6\]/);
});

test('actual controller transaction schema survives actual GUI adapter and Node gate', () => {
  // Pure schema integration: no subprocess, privilege, account or product action.
  // Use real observer.finish and helper_action bodies; only OS/driver operations
  // are fakes. This guards producer/consumer field drift, not runtime coverage.
  const program = `import json,sys,threading,types
from unittest.mock import Mock
sys.path.insert(0,sys.argv[1])
import root_controller as root
import linux_tun as gui
app={'pid':100,'startTime':'123','uid':1001}
results=[]
for kind in ('install','uninstall'):
    controller=types.SimpleNamespace(gui=app,launch={'authenticationMode':'existing-session'},ops=types.SimpleNamespace(proc=lambda pid:None))
    observer=root.TransactionObserver(controller,kind)
    observer.thread=Mock(); observer.thread.is_alive.return_value=False
    row={'pid':102,'startTime':'125','uid':0,'ppid':100}
    observer.rows={(102,'125'):row}; observer.pkexec={(102,'125'):row}; observer.shells={(102,'125'):row}
    closed=observer.finish()
    run=object.__new__(gui.TunCarrierRun)
    run.app=app; run.driver=Mock(); run.supervisor=Mock(); run.controller=Mock()
    run.controller.call.side_effect=lambda command,**args: closed if command=='transaction_end' else {}
    run.text=lambda key:key
    replies=iter([[],[{'sequence':1,'kind':'helper','envelopeSuccess':True,'success':True,'installed':kind=='install','ready':kind=='install'}]])
    run.tun_observations=lambda:next(replies)
    result=run.helper_action(kind)
    assert run.controller.call.call_args_list[-1].args==('transaction_end',)
    run.supervisor.end_window.assert_called_once_with(closed['owned'])
    results.append(result)
print(json.dumps(results))`;
  const rows = JSON.parse(execFileSync('python3', ['-B', '-c', program, join(ROOT, 'scripts/package-acceptance')],
    { encoding: 'utf8', timeout: 10000, maxBuffer: 1024 ** 2 }));
  for (const [index, kind] of ['install', 'uninstall'].entries()) {
    verifyAuthorization(rows[index], kind, report.execution.app, 1001, 'existing-session', subject);
    for (const mutate of [r => delete r.authorization.subject, r => delete r.authorization.owned,
      r => delete r.callback.envelopeSuccess, r => r.authorization.closed = false]) {
      const bad = clone(rows[index]); mutate(bad);
      assert.throws(() => verifyAuthorization(bad, kind, report.execution.app, 1001, 'existing-session', subject));
    }
  }
});
