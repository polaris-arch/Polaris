import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { admit, sha256 } from './package-acceptance/transport.mjs';
import { selectedCoreManifest } from './lib/core-identity.mjs';
import { releaseInstallationContract } from './desktop-core/release-consumer.mjs';
import { AUTH_MODE, AUTH_PHRASE, AUTH_GROUP_MODE, AUTH_GROUP_PHRASE, CASES, KERNEL, VERSION, authorizationFacts, credentialAuthorization,
  stamp, tunSelection, unstamped, verifyAuthenticationAdmission, verifyAuthorization, verifyGroupRecovery, verifyHelper, verifyKernel, verifyReport } from './package-acceptance/tun-transport.mjs';
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
  sender: { login: 'reviewer' }, ref: 'refs/heads/reviewed', inputs: { linux_acceptance_candidate: sha,
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


const transactionBinding = kind => ({schema:'polaris-transaction-binding-v1',runToken:'abcdef123456',
  sourceSha:sha,githubRunId:context.runId,attempt:context.attempt,kind,
  subject:{pid:100,startTime:'123',uid:1001},windowNonce:(kind==='install'?'1':'2').repeat(32)});
const authorization = kind => ({ callback: { envelopeSuccess: true, success: true, installed: kind === 'install', ready: kind === 'install' },
  authorization: { kind, subject: { pid: 100, startTime: '123', uid: 1001 }, authenticationMode: 'existing-session',
    authorizationObserved: true, closed: true, credentialPromptClaimed: false,
    transactionBinding:transactionBinding(kind),auxiliaryIdentityCoverage:'complete',auxiliaryTerminals:[],
    pkexecBirths: [[102, '125']], rootShellBirths: [[102, '125']], owned: [{ uid: 0, ppid: 100, pid: 102, startTime: '125' }] } });
report.auxiliaryIdentityCoverage='complete';
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
test('actual full dispatch ref survives setup admission for both authentication modes', () => {
  // Run the same pure admission chain as the CLI. Only Git reads are stubbed;
  // the selected contract and workflow/event shape are the actual source inputs.
  const git = (_program, args) => {
    if (args[0] === 'status') return '';
    if (args[0] === 'show') return readFileSync(join(ROOT, args[1].slice(5)), 'utf8');
    return args[1] === 'HEAD' ? sha : tree;
  };
  for (const mode of ['existing-session', AUTH_MODE]) {
    const actual = clone(event);
    actual.inputs.linux_tun_auth_mode = mode;
    actual.inputs.linux_tun_credential_authorization = mode === AUTH_MODE ? AUTH_PHRASE : '';
    tunSelection(env);
    const admitted = admit(ROOT, env, git);
    const selected = verifyKernel(releaseInstallationContract(ROOT,
      JSON.parse(readFileSync(join(ROOT, 'src-tauri/core-manifest.json'), 'utf8'))));
    assert.equal(selected.sourceCommit, '332935c310e78e15319d434f33a9103329200931');
    assert.equal(selected.version, '1.15.0-polaris-alpha.12.5');
    assert.equal(admitted.kernelSourceCommit, selected.sourceCommit);
    const facts = authorizationFacts(admitted, actual, env);
    assert.equal(facts.mode, mode);
    assert.equal(facts.execution.sourceSha, sha);
    assert.equal(facts.execution.sourceTree, tree);
    assert.equal(facts.execution.githubRunId, env.GITHUB_RUN_ID);
    assert.equal(facts.execution.githubRunAttempt, env.GITHUB_RUN_ATTEMPT);
    for (const ref of ['reviewed', 'refs/heads/other', 'refs/tags/reviewed'])
      assert.throws(() => authorizationFacts(admitted, { ...actual, ref }, env), /Actual dispatch payload/);
  }
  // Equal basenames are not equal refs in the other direction either.
  const tagEnv = { ...env, GITHUB_REF: 'refs/tags/v1.0.0',
    GITHUB_WORKFLOW_REF: 'polaris-arch/Polaris/.github/workflows/package.yml@refs/tags/v1.0.0' };
  const tagContext = admit(ROOT, tagEnv, git);
  for (const ref of ['v1.0.0', 'refs/heads/v1.0.0'])
    assert.throws(() => authorizationFacts(tagContext, { ...event, ref }, tagEnv), /Actual dispatch payload/);
  assert.equal(authorizationFacts(tagContext, { ...event, ref: tagEnv.GITHUB_REF }, tagEnv).mode, 'existing-session');
});
test('historical 12.4, c6 and 814 or renamed versions cannot satisfy current TUN alpha.12.5 admission', () => {
  assert.equal(KERNEL, '332935c310e78e15319d434f33a9103329200931');
  assert.equal(VERSION, '1.15.0-polaris-alpha.12.5');
  assert.equal(verifyKernel(contract), contract);
  for (const change of [{ sourceCommit: '0cfa24573ff34c26608151068334329268cfdf14' },
    { sourceCommit: '0cfa24573ff34c26608151068334329268cfdf14', version: '1.15.0-polaris-alpha.12.4', tag: 'v1.15.0-polaris-alpha.12.4' },
    { version: '1.15.0-polaris-alpha.12.4', tag: 'v1.15.0-polaris-alpha.12.4' },
    { sourceCommit: 'c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a' },
    { sourceCommit: 'c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a', version: '1.15.0-polaris-alpha.12.2', tag: 'v1.15.0-polaris-alpha.12.2' },
    { sourceCommit: '814da362838e96e54f904836a92c2b0d8c3ae29b' },
    { repository: 'fork/kernel' }, { version: '1.15.0-polaris-alpha.12.1' }, { tag: 'vwrong' }])
    assert.throws(() => verifyKernel({ ...contract, ...change }));
});
test('historical original material stays historical and cannot be relabelled as current TUN input', () => {
  const pendingPath = join(ROOT, 'scripts/desktop-core/pending-release-selection.json');
  const before = readFileSync(pendingPath);
  const pending = JSON.parse(before);
  const previous124 = pending.sourceEvidence.historicalSourceEvidence;
  assert.equal(previous124.actualSourceArchive.name,
    'sing-box-polaris-0cfa24573ff34c26608151068334329268cfdf14.tar.gz');
  assert.equal(previous124.actualSourceArchive.sha256, '47447ab8665433752d1144a7ee23ae07f81cbb885191378173c26100263c2683');
  assert.equal(previous124.originalIndexSha256, '10629e4d325de33124b496e1021f9d097710a497e632071c16f006c324f7afc4');
  const c6 = previous124.historicalSourceEvidence;
  assert.equal(c6.actualSourceArchive.name,
    'sing-box-polaris-c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a.tar.gz');
  assert.equal(c6.actualSourceArchive.sha256, '7a734c45a7765ec45fe389f09976808c5a7a6e7caadd6689b840889e6335d775');
  const historicalPath = join(ROOT, pending.historicalContractPath);
  const original = readFileSync(historicalPath);
  const historical = JSON.parse(original), unchanged = clone(historical);
  assert.throws(() => verifyKernel(historical));
  assert.deepEqual(historical, unchanged);
  assert.deepEqual(readFileSync(historicalPath), original);
  assert.deepEqual(readFileSync(pendingPath), before);
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
  for (const text of ['transport.mjs fetch', 'final-package-receipt.py', 'tun-transport.mjs prepare',
    'linux_tun.py run', 'tun-transport.mjs finish', 'cargo build --locked -p polaris-singbox-grpc --example package_runtime_probe']) assert.ok(job.includes(text), text);
  assert.ok(job.indexOf('apt-get install') < job.indexOf('linux_tun.py run'));
  assert.doesNotMatch(job.split("\n").filter(line => !line.trimStart().startsWith("#")).join("\n"), /cargo build --release|tauri build|dpkg --install|sysctl|--no-sandbox/);
  assert.match(job, /if: always\(\)/); assert.doesNotMatch(job, /continue-on-error: true/);
  for (const stage of ['draft', 'public']) assert.ok(source.includes(`tun-transport.mjs release-check linux-tun-before-${stage} dist-release`));
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
  // Pure schema integration: no privileged subprocess, account or product action.
  // Use real observer.finish and helper_action bodies; only OS/driver operations
  // are fakes. This guards producer/consumer field drift, not runtime coverage.
  const program = `import json,sys,threading,types,subprocess,tempfile,os
from unittest.mock import Mock,patch
sys.path.insert(0,sys.argv[1])
import root_controller as root
import tun as actual_tun
import linux_tun as gui
import helper_observation as helper
app={'pid':100,'startTime':'123','uid':1001}
results=[]
for kind in ('install','uninstall'):
    controller=types.SimpleNamespace(gui=app,launch={'authenticationMode':'existing-session','runId':'abcdef123456','execution':{'sourceSha':'a'*40,'githubRunId':'120','githubRunAttempt':'1'}},ops=types.SimpleNamespace(proc=lambda pid:None),tun=actual_tun)
    private=tempfile.TemporaryDirectory()
    home=actual_tun.Path(private.name);home.chmod(0o700)
    script=home/('a'*24+'-polaris-helper-'+kind+'.sh')
    script.write_bytes((actual_tun.Path(sys.argv[1])/'fixtures'/('linux-helper-'+kind+'.sh')).read_bytes())
    script.chmod(0o700)
    held=root.HeldScript(script,os.getuid(),home)
    def verify_script(path,expected_kind):
        assert path==str(script) and expected_kind==kind
        return held.verify_linked()
    controller.verify_script=verify_script;controller.auth_note=Mock()
    observer=root.TransactionObserver(controller,kind)
    observer.thread=Mock(); observer.thread.is_alive.return_value=False
    row={'pid':102,'startTime':'125','uid':0,'ppid':100}
    observer.rows={(102,'125'):row}; observer.pkexec={(102,'125'):{'identity':row,'script':str(script),'proof':held.proof}}; observer.shells={(102,'125'):row}
    # Real terminal producer: only whole-directory ENOENT proves sampled absence.
    real_lstat=actual_tun.Path.lstat
    def missing(path):
        if str(path)=='/proc/102':raise FileNotFoundError(2,'synthetic absent directory')
        return real_lstat(path)
    with patch.object(actual_tun.Path,'lstat',autospec=True,side_effect=missing) as terminal_reads:
        closed=observer.finish()
        assert sum(str(call.args[0])=='/proc/102' for call in terminal_reads.call_args_list)==2
    def negative_observer():
        rejected=root.TransactionObserver(controller,kind)
        rejected.thread=observer.thread;rejected.rows=dict(observer.rows)
        rejected.pkexec=dict(observer.pkexec);rejected.shells=dict(observer.shells)
        return rejected
    denied=negative_observer()
    with patch.object(actual_tun.Path,'lstat',side_effect=PermissionError(13,'synthetic denial')):
        try: denied.finish()
        except PermissionError: pass
        else: raise AssertionError('permission failure must not prove terminal closure')
    assert denied.error=='finish-terminal'
    incomplete=negative_observer()
    with patch.object(actual_tun.Path,'lstat',return_value=Mock()), patch.object(actual_tun.Path,'read_text',side_effect=FileNotFoundError(2,'synthetic missing stat')):
        try: incomplete.finish()
        except FileNotFoundError: pass
        else: raise AssertionError('missing proc subfile must not prove terminal closure')
    assert incomplete.error=='finish-terminal' and observer.error is None
    held.close();private.cleanup()
    run=object.__new__(gui.TunCarrierRun)
    run.result={}; run.env={}
    run.app=app; run.driver=Mock(); run.supervisor=Mock(); run.controller=Mock(); run.launch_options={'authenticationMode':'existing-session'};run.transaction_launch=controller.launch
    runtime=root.auth_runtime_observation('existing-session');runtime['kind']=kind
    scanned={**observer.snapshot(),'authenticationRuntime':runtime}
    run.controller.call.side_effect=lambda command,**args: closed if command=='transaction_end' else scanned if command=='transaction_scan' else {'observing':kind,'transactionBinding':observer.binding} if command=='transaction_begin' else {}
    run.text=lambda key:key
    raw={'success':True,'data':{'success':True,'status':{'supported':True,'installed':kind=='install','ready':kind=='install'}}}
    projector='process.stdout.write(JSON.stringify(('+helper.PROJECTOR_JS+')(JSON.parse(process.argv[1]))))'
    projected=json.loads(subprocess.check_output(['node','-e',projector,json.dumps(raw)],text=True))
    helper.validate_projection(projected)
    replies=iter([[],[{'sequence':1,'kind':'helperObservation','observedElapsedMs':10,'projection':projected}]])
    run.tun_observations=lambda:next(replies)
    result=run.helper_action(kind)
    assert run.controller.call.call_args_list[-1].args==('transaction_end',)
    run.supervisor.end_window.assert_called_once_with(closed)
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


test('joint sudo membership authority is separate, default off, exact runner and invocation', () => {
  const approved = clone(event);
  approved.inputs.linux_tun_auth_mode = AUTH_GROUP_MODE;
  approved.inputs.linux_tun_credential_authorization = AUTH_GROUP_PHRASE;
  const facts = authorizationFacts(context, approved, env);
  const lease = credentialAuthorization(facts, {runToken:'abcdef123456', guiUid:1001, account:'runner'}, 1000);
  assert.equal(lease.schema, 'polaris-tun-credential-group-lease-v1');
  assert.equal(lease.authMode, AUTH_GROUP_MODE);
  assert.deepEqual(lease.groupLease, {group:'sudo', account:'runner', guiUid:1001, maxLeaseSeconds:90,
    originalMembership:'absent', restore:'remove-only-added-membership'});
  assert.equal(lease.maxLeaseSeconds, 90);
  assert.equal(lease.expiresUnix - lease.notBeforeUnix, 900);
  assert.equal(lease.groupLease.gid, undefined);
  for (const phrase of ['', AUTH_PHRASE, 'true']) {
    const bad = clone(approved); bad.inputs.linux_tun_credential_authorization = phrase;
    assert.throws(() => authorizationFacts(context, bad, env));
  }
  const wrongMode = clone(approved); wrongMode.inputs.linux_tun_auth_mode = AUTH_MODE;
  assert.throws(() => authorizationFacts(context, wrongMode, env));
  assert.throws(() => authorizationFacts({...context, attempt:'2'}, approved, {...env,GITHUB_RUN_ATTEMPT:'2'}));
  assert.throws(() => credentialAuthorization(facts, {runToken:'abcdef123456',guiUid:1001,account:'other'},1000));
  assert.throws(() => credentialAuthorization(facts, {runToken:'abcdef123456',guiUid:0,account:'root'},1000));
  assert.throws(() => credentialAuthorization(facts, {runToken:'abcdef123456',guiUid:1002,account:'runner'},1000));
  assert.equal(credentialAuthorization(authorizationFacts(context,event,env),{runToken:'abcdef123456',guiUid:1001,account:'runner'},1000),null);
  const old = clone(event); old.inputs.linux_tun_auth_mode=AUTH_MODE; old.inputs.linux_tun_credential_authorization=AUTH_PHRASE;
  const oldLease = credentialAuthorization(authorizationFacts(context,old,env),{runToken:'abcdef123456',guiUid:1001,account:'runner'},1000);
  assert.equal(oldLease.schema,'polaris-tun-credential-lease-v1');
  assert.equal(oldLease.groupLease,undefined);
  const workflow = readFileSync(join(ROOT,'.github/workflows/package.yml'),'utf8');
  assert.match(workflow,/linux_tun_auth_mode:[\s\S]*?default: existing-session/);
  assert.match(workflow,/          - temporary-existing-runner-password-and-sudo-group\n/);
});


test('joint recovery requires real relock and removal of only the added bound membership', () => {
  const row = authorization('install');
  const group = {schema:'polaris-sudo-membership-recovery-v1', account:'runner', guiUid:1001,
    group:'sudo', groupGid:27, originalMembership:'absent', finalMembership:'absent',
    removedOnlyAddedMembership:true, writerStopped:true, maxLeaseSeconds:90};
  const credential = {guiUid:1001, account:'runner', finalPasswordStatus:'L',plaintextPersisted:false,
    emptyPasswordRestored:false,temporaryPasswordHash:'locked until disposable VM destruction'};
  Object.assign(row.authorization,{authenticationMode:AUTH_GROUP_MODE,credentialAuthenticationComplete:true,
    authDescendantsReaped:true,credentialRecovery:credential,jointLease:{credentialRecovery:clone(credential),groupRecovery:group}});
  verifyAuthorization(row,'install',report.execution.app,1001,AUTH_GROUP_MODE,subject);
  for (const [key,value] of Object.entries({schema:'fake',account:'root',guiUid:0,group:'admin',groupGid:0,
    originalMembership:'present',finalMembership:'present',removedOnlyAddedMembership:false,writerStopped:false,maxLeaseSeconds:91})) {
    const bad=clone(row); bad.authorization.jointLease.groupRecovery[key]=value;
    assert.throws(()=>verifyAuthorization(bad,'install',report.execution.app,1001,AUTH_GROUP_MODE,subject));
  }
  for (const mutate of [r=>delete r.authorization.jointLease,
    r=>r.authorization.jointLease.credentialRecovery.finalPasswordStatus='P',
    r=>r.authorization.authDescendantsReaped=false,
    r=>r.authorization.jointLease.groupRecovery.secret='forbidden']) {
    const bad=clone(row); mutate(bad);
    assert.throws(()=>verifyAuthorization(bad,'install',report.execution.app,1001,AUTH_GROUP_MODE,subject));
  }
  assert.throws(()=>verifyAuthorization(row,'install',report.execution.app,1001,AUTH_MODE,subject));
  assert.throws(()=>verifyGroupRecovery({...group,groupGid:true},1001,'runner'));
});

test('joint report requires pending preparation and final same-GID recovery for both transactions', () => {
  const actual=clone(report), admitted=clone(admission);
  admitted.authentication.mode=AUTH_GROUP_MODE;
  const {fingerprint, ...bindingFacts}=actual.binding;
  actual.binding=stamp({...bindingFacts,admission:admitted});
  actual.bindingSha256=sha256(JSON.stringify(actual.binding,null,2)+'\n');
  const group={schema:'polaris-sudo-membership-recovery-v1',account:'runner',guiUid:1001,group:'sudo',groupGid:451,
    originalMembership:'absent',finalMembership:'absent',removedOnlyAddedMembership:true,writerStopped:true,maxLeaseSeconds:90};
  const credential={guiUid:1001,account:'runner',finalPasswordStatus:'L',plaintextPersisted:false,
    emptyPasswordRestored:false,temporaryPasswordHash:'locked until disposable VM destruction'};
  for(const key of ['helperInstall','helperUninstall']) Object.assign(actual[key].authorization,
    {authenticationMode:AUTH_GROUP_MODE,credentialAuthenticationComplete:true,authDescendantsReaped:true,
      credentialRecovery:clone(credential),jointLease:{credentialRecovery:clone(credential),groupRecovery:clone(group)}});
  actual.rootPreparation={guarded:true,authentication:'pending-real-product-authorization',
    authenticationReadiness:{authenticated:false,groupMembership:'pending-guarded-lease'}};
  actual.authenticationClosed={groupRecovery:clone(group)};
  verifyReport(actual,admitted,hash,AUTH_GROUP_MODE);
  for(const mutate of [r=>delete r.authenticationClosed,r=>r.authenticationClosed.groupRecovery.groupGid=27,
    r=>r.helperUninstall.authorization.jointLease.groupRecovery.groupGid=27,
    r=>r.rootPreparation.authenticationReadiness.authenticated=true,
    r=>r.rootPreparation.authenticationReadiness.groupMembership='ready',
    r=>r.rootPreparation.guarded=false]) {
    const bad=clone(actual);mutate(bad);assert.throws(()=>verifyReport(bad,admitted,hash,AUTH_GROUP_MODE));
  }
});

test('auxiliary terminals remain separate incomplete evidence with exact binding and strong ancestry', () => {
  const row=authorization('install'), a=row.authorization;
  Object.assign(a.owned[0],{state:'S',exe:'/bin/bash',exeDev:1,exeInode:2,capEff:0,capAmb:0,
    namespaces:{net:1,mnt:2,pid:3,cgroup:4},cgroup:'0::/'});
  a.auxiliaryIdentityCoverage='incomplete';
  a.auxiliaryTerminals=[{schema:'polaris-unverified-auxiliary-terminal-v1',binding:clone(a.transactionBinding),
    inventory:{pid:103,startTime:'126',ppid:102},parent:{pid:102,startTime:'125'},anchor:{pid:102,startTime:'125'},
    capture:{phase:'inputs-before',probe:'cmdline-empty',error:'contract'},
    terminal:{pid:103,startTime:'126',state:'absent',scope:'sampled-terminal-unverified-execution'},
    priorStrongOwned:false,identityVerified:false,signalAuthority:false}];
  verifyAuthorization(row,'install',report.execution.app,1001,'existing-session',subject,context);
  for (const mutate of [
    r=>r.authorization.auxiliaryTerminals[0].binding.attempt='2',
    r=>r.authorization.auxiliaryTerminals[0].binding.windowNonce='f'.repeat(32),
    r=>r.authorization.auxiliaryTerminals[0].signalAuthority=true,
    r=>r.authorization.auxiliaryTerminals[0].identityVerified=true,
    r=>r.authorization.auxiliaryTerminals[0].terminal.state='reaped',
    r=>r.authorization.auxiliaryTerminals[0].capture.probe='raw-secret',
    r=>r.authorization.auxiliaryTerminals[0].capture.phase='owned-parent',
    r=>r.authorization.auxiliaryTerminals[0].capture.error='eacces',
    r=>r.authorization.auxiliaryTerminals[0].parent.startTime='999',
    r=>r.authorization.auxiliaryTerminals[0].anchor.pid=999,
    r=>r.authorization.auxiliaryIdentityCoverage='complete',
    r=>r.authorization.owned.push({...r.authorization.auxiliaryTerminals[0].inventory,uid:0}),
    r=>r.authorization.auxiliaryTerminals.push(clone(r.authorization.auxiliaryTerminals[0])),
    r=>delete r.authorization.owned[0].namespaces,
    r=>delete r.authorization.owned[0].exe,
    r=>delete r.authorization.owned[0].exeInode,
    r=>delete r.authorization.owned[0].ppid,
    r=>r.callback.success=false
  ]) {
    const bad=clone(row);mutate(bad);
    assert.throws(()=>verifyAuthorization(bad,'install',report.execution.app,1001,'existing-session',subject,context));
  }
});
