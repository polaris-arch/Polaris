#!/usr/bin/env node
// TUN is an explicit, separate diagnostic of the same final carrier. It never
// mints the baseline publication receipt or impersonates a formal producer.
import { execFileSync } from 'node:child_process';
import { lstatSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { userInfo } from 'node:os';
import { fileURLToPath } from 'node:url';
import { admit, apiJson, checkRun, download, runtimeBytes, selection, sha256, verifyCarrierFiles, verifyFreeze, verifyNativeProof } from './transport.mjs';
import { canonical } from '../desktop-core/source-graph.mjs';
import { releaseInstallationContract } from '../desktop-core/release-consumer.mjs';
import { coreIdentityId, selectedCoreManifest } from '../lib/core-identity.mjs';

export const KERNEL = 'c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a';
export const AUTH_MODE = 'temporary-existing-runner-password';
export const AUTH_PHRASE = 'AUTHORIZE TEMPORARY RUNNER PASSWORD AND RELOCK FOR THIS RUN';
export const VERSION = '1.15.0-polaris-alpha.12.2';
export const CASES = ['default_graph', 'tun_data_plane', 'graph_apply', 'daily_switch',
  'auto_preserves', 'manual_preserve', 'manual_disconnect', 'uncovered_target_restart', 'synthetic_route', 'normal_cleanup'];
const demand = (ok, reason) => { if (!ok) throw Error(reason); };
const same = (a, b, reason) => demand(canonical(a) === canonical(b), reason);
const hex = s => typeof s === 'string' && /^[0-9a-f]{64}$/.test(s);
const read = p => { const path = regular(p); demand(lstatSync(path).size <= 16 * 1024 ** 2, 'TUN JSON input bound'); return JSON.parse(readFileSync(path, 'utf8')); };
export const stamp = value => ({ ...value, fingerprint: sha256(canonical(value)) });
export function unstamped(value, schema) {
  const { fingerprint, ...facts } = value ?? {};
  demand(facts.schema === schema && hex(fingerprint) && fingerprint === sha256(canonical(facts)), 'TUN evidence fingerprint/schema differs');
  return facts;
}
function regular(path) {
  const full = resolve(path);
  for (let at = full;; at = dirname(at)) {
    const st = lstatSync(at);
    demand(!st.isSymbolicLink() && (at === full ? st.isFile() : st.isDirectory()), 'TUN input must be regular and unlinked');
    if (dirname(at) === at) break;
  }
  return full;
}
function write(path, value) {
  mkdirSync(dirname(resolve(path)), { recursive: true, mode: 0o700 });
  writeFileSync(path, JSON.stringify(value, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
}
export function tunSelection(env = process.env) {
  selection(env);
  demand(env.GITHUB_ACTIONS === 'true' && env.GITHUB_REPOSITORY === 'polaris-arch/Polaris'
    && env.GITHUB_EVENT_NAME === 'workflow_dispatch' && env.PACKAGE_TUN_REQUESTED === 'true'
    && env.PACKAGE_ACCEPTANCE_REQUESTED === 'true'
    && /^[0-9a-f]{40}$/.test(env.PACKAGE_ACCEPTANCE_CANDIDATE ?? '')
    && env.PACKAGE_ACCEPTANCE_CANDIDATE === env.GITHUB_SHA,
  'TUN requires explicit direct manual Package selection and exact source SHA');
  demand(env.RUNNER_ENVIRONMENT === undefined || env.RUNNER_ENVIRONMENT === 'github-hosted', 'Self-hosted TUN forbidden');
  return { sourceScope: 'same-final-carrier', publicationEligible: false };
}
export function authorizationFacts(context, event, env = process.env) {
  tunSelection(env);
  demand(event?.repository?.full_name === context.repository
    && String(event.repository.id) === context.repositoryId
    && event.sender?.login === env.GITHUB_ACTOR && /^[A-Za-z0-9-]{1,39}$/.test(env.GITHUB_ACTOR ?? '')
    && event.ref === env.GITHUB_REF.replace(/^refs\/(heads|tags)\//, '')
    && event.inputs?.linux_acceptance_candidate === context.candidate
    && event.inputs?.linux_tun_acceptance === 'true' && event.inputs?.linux_package_acceptance === 'true',
  'Actual dispatch payload/repository/ref/actor/source differs');
  const mode = event.inputs?.linux_tun_auth_mode || 'existing-session';
  const phrase = event.inputs?.linux_tun_credential_authorization || '';
  demand(['existing-session', AUTH_MODE].includes(mode), 'Unknown authentication mode');
  if (mode === AUTH_MODE) demand(phrase === AUTH_PHRASE && context.attempt === '1'
      && (!env.GITHUB_TRIGGERING_ACTOR || env.GITHUB_TRIGGERING_ACTOR === env.GITHUB_ACTOR),
    'Exact current-run credential action authorization required; rerun cannot renew it');
  else demand(phrase === '', 'Unused credential authorization forbidden');
  return { mode, phrase, execution: { repository: context.repository, sourceSha: context.candidate,
    sourceTree: context.tree, githubRunId: context.runId, githubRunAttempt: context.attempt,
    actor: env.GITHUB_ACTOR, workflowRef: context.workflowRef, eventSha256: sha256(canonical(event)) } };
}
export function credentialAuthorization(facts, subject, now = Math.floor(Date.now() / 1000)) {
  demand(/^[a-f0-9]{12}$/.test(subject?.runToken ?? '') && Number.isSafeInteger(subject.guiUid) && subject.guiUid > 0
    && /^[a-z_][a-z0-9_-]{0,31}$/.test(subject.account ?? '') && Number.isSafeInteger(now) && now > 0,
  'Exact observed GUI account and invocation required');
  if (facts.mode === 'existing-session') return null;
  demand(facts.mode === AUTH_MODE && facts.phrase === AUTH_PHRASE, 'Credential lease permission missing');
  return { schema: 'polaris-tun-credential-lease-v1', phrase: AUTH_PHRASE, authMode: AUTH_MODE,
    execution: facts.execution, runToken: subject.runToken, guiUid: subject.guiUid, account: subject.account,
    transactions: ['install', 'uninstall'], notBeforeUnix: now, expiresUnix: now + 900, maxLeaseSeconds: 90 };
}
export function verifyAuthenticationAdmission(admission, facts) {
  const saved = admission.authentication;
  demand(saved?.mode === facts.mode, 'Frozen authentication mode differs');
  same(saved.execution, facts.execution, 'Frozen actual event/actor/run authorization differs');
  demand(Number.isSafeInteger(saved.subject?.guiUid) && saved.subject.guiUid > 0
    && /^[a-z_][a-z0-9_-]{0,31}$/.test(saved.subject.account ?? ''), 'Frozen observed ordinary account missing');
  return saved.subject;
}
export function verifyKernel(contract) {
  demand(contract?.repository === 'polaris-arch/sing-box-polaris' && contract.sourceCommit === KERNEL
    && contract.version === VERSION && contract.tag === 'v' + VERSION,
  'Complete genuine c6 alpha.12.2 formal materials required; old 814 is not a substitute');
  return contract;
}
export function verifyHelper(inspection, carrier, contract, appVersion) {
  demand(appVersion === '1.0.0', 'Unexpected App version');
  const parent = dirname(carrier.members.core.path);
  for (const [role, name] of [['helper', 'polaris-helper'], ['manifest', 'core-manifest.json']]) {
    const row = inspection?.[role];
    demand(row && row.path === `${parent}/${name}` && hex(row.sha256)
      && Number.isSafeInteger(row.bytes) && row.bytes > 0 && row.bytes <= (role === 'manifest' ? 65536 : 128 * 1024 ** 2),
    'Unique adjacent original helper/manifest required');
  }
  same(inspection.core, carrier.members.core, 'Inspected core differs from frozen carrier');
  same(inspection.cronet, carrier.members.cronet, 'Inspected Cronet differs from frozen carrier');
  const original = inspection.embeddedManifest;
  const expected = selectedCoreManifest(contract, 'linux', appVersion, inspection.helper.sha256);
  same(original, expected, 'Packaged selected helper/core identity differs from actual formal material');
  demand(original.id === coreIdentityId(original.core), 'Selected identity digest differs');
  for (const [role, name] of [['core', 'sing-box'], ['cronet', 'libcronet.so']]) {
    const file = original.core.files.find(row => row.name === name);
    demand(file && file.sha256 === carrier.members[role].sha256 && file.bytes === carrier.members[role].bytes,
      'Installed identity does not name exact frozen member bytes');
  }
  return { helper: inspection.helper, manifest: inspection.manifest, embeddedManifest: original };
}
function birth(value) {
  demand(value && Number.isSafeInteger(value.pid) && value.pid > 1 && /^[1-9][0-9]*$/.test(value.startTime), 'Actual process birth required');
  return [value.pid, value.startTime];
}
function processIdentity(value) {
  birth(value);
  demand(['uid', 'ppid', 'exeDev', 'exeInode', 'capAmb', 'capEff'].every(k => Number.isSafeInteger(value[k]) && value[k] >= 0)
    && value.ppid > 0 && value.exeInode > 0 && typeof value.cgroup === 'string' && value.cgroup.length > 0,
  'Complete actual process identity required');
  same(Object.keys(value.namespaces ?? {}).sort(), ['cgroup', 'mnt', 'net', 'pid'], 'Actual process namespaces missing');
  demand(Object.values(value.namespaces).every(n => Number.isSafeInteger(n) && n > 0), 'Namespace inode required');
  return Object.fromEntries(['pid', 'startTime', 'uid', 'ppid', 'exeDev', 'exeInode', 'capAmb', 'capEff', 'namespaces', 'cgroup'].map(k => [k, value[k]]));
}
function executableIdentity(executable, identity) {
  demand(executable?.dev === identity.exeDev && executable.inode === identity.exeInode && hex(executable.sha256),
    'Executable object differs from observed process');
}
export function verifyAuthorization(row, kind, app, uid, mode, subject) {
  const a = row?.authorization;
  demand(row?.callback?.envelopeSuccess === true && row.callback.success === true
    && row.callback.installed === (kind === 'install') && row.callback.ready === (kind === 'install')
    && a?.kind === kind && a.authenticationMode === mode && a.subject?.uid === uid
    && a.authorizationObserved === true && a.closed === true && a.credentialPromptClaimed === false,
  'Original GUI helper action and real authorization transaction required');
  same(birth(a.subject), birth(app), 'Authorization subject differs from actual GUI');
  demand(a.pkexecBirths?.length === 1 && a.rootShellBirths?.length === 1, 'One original pkexec and root shell transition required');
  same(a.pkexecBirths[0], a.rootShellBirths[0], 'Root shell was not the same observed pkexec birth');
  demand(Array.isArray(a.owned) && a.owned.some(p => p.uid === 0 && p.ppid === app.pid
    && canonical(birth(p)) === canonical(a.rootShellBirths[0])), 'Original root shell lineage missing');
  if (mode === AUTH_MODE) demand(a.credentialAuthenticationComplete === true && a.authDescendantsReaped === true
    && a.credentialRecovery?.guiUid === uid && a.credentialRecovery.finalPasswordStatus === 'L'
    && subject?.guiUid === uid && a.credentialRecovery.account === subject.account
    && a.credentialRecovery.emptyPasswordRestored === false
    && a.credentialRecovery.temporaryPasswordHash === 'locked until disposable VM destruction'
    && a.credentialRecovery.plaintextPersisted === false,
  'Actual private credential exchange, same account relock and auth descendants closure required');
  else demand(mode === 'existing-session' && a.credentialAuthenticationComplete !== true, 'Authentication scope differs');
}
export function verifyReport(report, admission, admissionSha256, authMode = 'existing-session') {
  demand(report?.schema === 'polaris-linux-tun-acceptance-v1' && report.result === 'success'
    && report.publicationEligible === false && report.scope === 'same-final-carrier'
    && report.transportSha256 === admission.transportSha256 && report.admissionSha256 === admissionSha256
    && report.carrierKind === admission.carrier.kind && report.carrierSha256 === admission.carrier.sha256
    && hex(report.bindingSha256) && report.blocked?.length === 0 && report.invalidCases?.length === 0
    && report.missingCases?.length === 0, 'TUN report is incomplete, failed, blocked or different bytes');
  same(report.context, admission.context, 'TUN report source/run/attempt differs');
  same(Object.keys(report.cases ?? {}).sort(), [...CASES].sort(), 'Every real TUN case required');
  for (const row of Object.values(report.cases)) demand(row?.status === 'passed' && row.evidence, 'TUN case skipped or missing');
  const binding = report.binding;
  unstamped(binding, 'polaris-linux-tun-bound-v1');
  demand(sha256(JSON.stringify(binding, null, 2) + '\n') === report.bindingSha256, 'Executed helper binding bytes differ');
  same(binding.admission, admission, 'Executed helper admission differs');
  demand(binding.publicationEligible === false, 'TUN binding cannot substitute release receipt');
  const execution = report.execution, actual = execution?.rootAttestation;
  const subject = admission.authentication?.subject;
  demand(admission.authentication?.mode === authMode && Number.isSafeInteger(subject?.guiUid)
    && subject.guiUid > 0 && subject.guiUid === execution?.uid
    && /^[a-z_][a-z0-9_-]{0,31}$/.test(subject.account ?? ''), 'Executed account differs from frozen admission');
  demand(execution?.uid > 0 && execution.defaultSandbox === true && execution.originalEntry === true
    && execution.startedViaHelper === true && execution.automation === 'tauri-driver 2.0.5'
    && execution.app?.sha256 === admission.carrier.members.app.sha256
    && execution.core?.sha256 === admission.carrier.members.core.sha256
    && actual?.executable?.sha256 === admission.carrier.members.core.sha256
    && actual.identity?.pid === execution.core.pid && actual.identity.startTime === execution.core.startTime
    && actual.identity.uid === execution.uid && actual.identity.capAmb === 0x3400 && actual.identity.capEff === 0x3400
    && actual.identity.cgroup === '0::/system.slice/polaris-helper.service'
    && report.helperIdentity?.identity?.uid === 0 && report.helperIdentity.identity.cgroup === actual.identity.cgroup
    && report.helperIdentity.executable?.sha256 === binding.helperBinding.helper.sha256,
  'Actual original App/helper/core birth, default sandbox or frozen ELF identity differs');
  const gui = report.rootGuiBinding;
  same(processIdentity(actual.identity), processIdentity(execution.core), 'Actual core identity differs from executed core');
  const helper = report.helperIdentity;
  processIdentity(helper.identity); processIdentity(gui?.identity);
  executableIdentity(actual.executable, actual.identity); executableIdentity(helper.executable, helper.identity);
  demand(actual.identity.ppid === helper.identity.pid, 'Core was not the registered helper child');
  same(actual.identity.namespaces, helper.identity.namespaces, 'Core/helper namespace differs');
  same(actual.identity.namespaces, gui.identity.namespaces, 'Core escaped original GUI system namespaces');

  same(birth(gui?.identity), birth(execution.app), 'Root-bound GUI birth differs');
  demand(gui.identity.uid === execution.uid && gui.identity.capAmb === 0 && gui.identity.capEff === 0
    && Number.isSafeInteger(execution.app.exeDev) && execution.app.exeDev >= 0
    && Number.isSafeInteger(execution.app.exeInode) && execution.app.exeInode > 0
    && gui.identity.exeDev === execution.app.exeDev && gui.identity.exeInode === execution.app.exeInode
    && gui.executable?.sha256 === execution.app.sha256 && gui.executable.dev === execution.app.exeDev
    && gui.executable.inode === execution.app.exeInode && Array.isArray(gui.ancestorBirths)
    && gui.ancestorBirths.length > 1 && canonical(gui.ancestorBirths[0]) === canonical(birth(execution.app)),
  'Root-bound original App ELF inode/hash, ordinary UID or lineage missing');
  const connectivity = report.controlConnectivity;
  for (const position of ['before', 'after']) {
    const c = connectivity?.[position];
    demand(c?.host === 'api.github.com' && c.position === position && c.dnsResolved === true
      && c.httpsStatus === 200 && c.tlsVerified === true && Array.isArray(c.addresses) && c.addresses.length > 0
      && c.addresses.every(a => typeof a === 'string' && a.length > 0 && a.length < 256)
      && Number.isSafeInteger(c.boundedResponseBytes) && c.boundedResponseBytes > 0 && c.boundedResponseBytes <= 8192
      && hex(c.boundedResponseSha256) && /^[1-9][0-9]*$/.test(c.readStartedUnixNs ?? '')
      && /^[1-9][0-9]*$/.test(c.readFinishedUnixNs ?? '')
      && BigInt(c.readFinishedUnixNs) >= BigInt(c.readStartedUnixNs), 'Fresh verified DNS/HTTPS control connectivity missing');
    birth(c.freshProcess);
  }
  demand(canonical(birth(connectivity.before.freshProcess)) !== canonical(birth(connectivity.after.freshProcess))
    && BigInt(connectivity.after.readStartedUnixNs) >= BigInt(connectivity.before.readFinishedUnixNs),
  'Control connectivity was not freshly re-probed after cleanup');
  demand(report.defaultObservation?.emptyHome === true && report.defaultObservation.dashboardEnabled === true,
    'Actual empty HOME default graph observation missing');
  runtimeBytes(report.configIdentity);
  demand(report.configIdentity.pid === execution.core.pid && report.configIdentity.startTime === execution.core.startTime
    && report.cases.default_graph.evidence.actualGraphSha256 === report.configIdentity.sha256,
  'Original runtime bytes are not the observed TUN core graph');
  verifyAuthorization(report.helperInstall, 'install', execution.app, execution.uid, authMode, subject);
  verifyAuthorization(report.helperUninstall, 'uninstall', execution.app, execution.uid, authMode, subject);
  demand(report.off?.stopped === true && canonical(birth(report.off.identity)) === canonical(birth(execution.core)), 'Actual OFF of registered TUN core required');
  demand(report.productUninstalled?.productUninstalled === true, 'Actual original helper uninstall required');
  const quit = report.trayQuit;
  same(birth(quit?.app), birth(execution.app), 'Tray quit App differs');
  same(birth(quit.coreAlreadyStopped), birth(execution.core), 'Tray quit expected stopped core differs');
  demand(quit.cleanExitMarker?.name === 'clean-exit.marker' && quit.cleanExitMarker.bytes === 0
    && quit.cleanExitMarker.sha256 === sha256('') && quit.cleanExitMarker.createdAfterShortcut === true,
  'Original normal tray quit commit marker required');
  const ordinary = report.ordinaryCleanup;
  demand(ordinary?.state === 'normal' && ['forced', 'unknown', 'remaining'].every(k => Array.isArray(ordinary[k]) && !ordinary[k].length),
  'Ordinary GUI/driver descendants not normally reaped');
  const cleanup = report.cleanup;
  demand(cleanup?.state === 'normal' && cleanup.forced === false && Array.isArray(cleanup.unknown) && !cleanup.unknown.length
    && ['watchdogCancelled', 'networkRestored', 'productUninstalled', 'privilegedProcessesReaped', 'authenticationWindowClosed'].every(k => cleanup[k] === true),
  'TUN normal cleanup not proved; forced recovery is not success');
  return report;
}
function verifyCases(root, report) {
  execFileSync('python3', ['-B', '-c',
    "import importlib.util,json,sys; s=importlib.util.spec_from_file_location('tun',sys.argv[1]); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); r=json.load(sys.stdin); [m.validate_case(k,v['evidence']) for k,v in r['cases'].items()]; m.validate_case('normal_cleanup',r['cleanup'])",
    join(root, 'scripts/package-acceptance/tun.py')], { input: JSON.stringify(report), encoding: 'utf8', timeout: 30000, maxBuffer: 1024 ** 2 });
}
function main() {
  const [command, ...args] = process.argv.slice(2), root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
  tunSelection();
  const context = admit(root);
  const input = read(join(root, 'src-tauri/core-manifest.json'));
  const contract = verifyKernel(releaseInstallationContract(root, input));
  const auth = authorizationFacts(context, read(process.env.GITHUB_EVENT_PATH));
  if (command === 'admit') { demand(!args.length, 'admit takes no arguments'); console.log(JSON.stringify(context)); return; }
  if (command === 'authorize') {
    demand(args.length === 1, 'authorize requires admission and bounded stdin subject');
    const admission = read(args[0]); unstamped(admission, 'polaris-linux-tun-admission-v1');
    same(admission.context, context, 'Authorization source/run/attempt differs');
    const raw = readFileSync(0); demand(raw.length <= 4096, 'Authorization subject input bound');
    const subject = JSON.parse(raw), expected = verifyAuthenticationAdmission(admission, auth);
    same({ guiUid: subject.guiUid, account: subject.account }, expected, 'Credential subject differs from prior observed admission');
    const authorization = credentialAuthorization(auth, subject);
    console.log(JSON.stringify({ execution: auth.execution, authenticationMode: auth.mode, authorization })); return;
  }
  if (command === 'prepare') {
    demand(args.length === 5, 'prepare requires transport packages native-proof kind output');
    const [transportPath, packages, nativePath, kind, output] = args;
    const transport = read(transportPath);
    unstamped(transport, 'polaris-linux-package-transport-v1');
    verifyFreeze(transport.freeze, context);
    verifyCarrierFiles(packages, transport.freeze.carriers, true);
    const carrier = transport.freeze.carriers.find(row => row.kind === kind);
    demand(carrier && ['deb', 'AppImage'].includes(kind), 'One genuine frozen carrier required');
    verifyNativeProof(read(nativePath), carrier, context);
    const user = userInfo();
    demand(process.getuid() > 0 && user.uid === process.getuid(), 'Prepare must observe original ordinary runner account');
    const authentication = { mode: auth.mode, execution: auth.execution, subject: { guiUid: user.uid, account: user.username } };
    verifyAuthenticationAdmission({ authentication }, auth);
    write(output, stamp({ schema: 'polaris-linux-tun-admission-v1', context, authentication,
      transportSha256: sha256(readFileSync(transportPath)), carrierArtifact: transport.artifact,
      carrier, kernelCommit: KERNEL, contractSha256: context.contractSha256,
      nativeProofSha256: sha256(readFileSync(nativePath)), sourceScope: 'same-final-carrier', publicationEligible: false }));
    return;
  }
  if (command === 'bind-helper') {
    demand(args.length === 3, 'bind-helper requires admission inspection output');
    const admission = read(args[0]); unstamped(admission, 'polaris-linux-tun-admission-v1');
    same(admission.context, context, 'Helper binding source/run/attempt differs');
    const helperBinding = verifyHelper(read(args[1]), admission.carrier, contract, read(join(root, 'src-tauri/tauri.conf.json')).version);
    write(args[2], stamp({ schema: 'polaris-linux-tun-bound-v1', admission, helperBinding, publicationEligible: false }));
    return;
  }
  if (command === 'finish') {
    demand(args.length === 4, 'finish requires admission packages report transport');
    const admission = read(args[0]); unstamped(admission, 'polaris-linux-tun-admission-v1');
    same(admission.context, context, 'Finished TUN source/run/attempt differs');
    verifyAuthenticationAdmission(admission, auth);
    const report = verifyReport(read(args[2]), admission, sha256(readFileSync(args[0])), auth.mode);
    // Reuse strict Python case predicates, not a second divergent implementation.
    verifyHelper({ ...report.binding.helperBinding, core: admission.carrier.members.core, cronet: admission.carrier.members.cronet }, admission.carrier, contract, '1.0.0');
    verifyCases(root, report);
    // Both bytes stay under the original transport verifier, even for one TUN leg.
    const transport = read(args[3]);
    demand(sha256(readFileSync(args[3])) === admission.transportSha256, 'Executed transport bytes changed');
    unstamped(transport, 'polaris-linux-package-transport-v1'); verifyFreeze(transport.freeze, context);
    demand(transport.freeze.carriers.some(row => canonical(row) === canonical(admission.carrier)), 'Executed carrier absent from original freeze');
    verifyCarrierFiles(args[1], transport.freeze.carriers, true);
    return;
  }
  if (command === 'release-check') {
    demand(args.length === 2 && selection().release && process.env.PACKAGE_TUN_RESULT === 'success',
      'Selected TUN matrix must succeed before either publication phase');
    checkRun(apiJson(`/repos/${context.repository}/actions/runs/${context.runId}`, process.env, execFileSync), context, process.env);
    const response = apiJson(`/repos/${context.repository}/actions/runs/${context.runId}/artifacts?per_page=100`, process.env, execFileSync);
    demand(response.total_count <= 100, 'TUN artifact list truncated');
    const carriers = [];
    for (const kind of ['deb', 'AppImage']) {
      const name = `linux-package-tun-${kind}-${context.candidate}-${context.runId}-${context.attempt}`;
      const matches = response.artifacts.filter(row => row.name === name);
      demand(matches.length === 1, 'Missing/duplicate current-attempt TUN artifact');
      const api = matches[0], destination = join(args[0], kind);
      mkdirSync(dirname(resolve(destination)), { recursive: true });
      download(root, { id: String(api.id), name, sha256: api.digest?.replace(/^sha256:/, '') }, context, destination, process.env, execFileSync);
      const admissionPath = join(destination, 'admitted.json'), admission = read(admissionPath);
      unstamped(admission, 'polaris-linux-tun-admission-v1'); same(admission.context, context, 'TUN accepted source/run/attempt differs');
      verifyAuthenticationAdmission(admission, auth);
      demand(admission.carrier.kind === kind && admission.kernelCommit === KERNEL, 'TUN accepted carrier/kernel differs');
      const report = verifyReport(read(join(destination, 'run.json')), admission, sha256(readFileSync(admissionPath)), auth.mode);
      verifyHelper({ ...report.binding.helperBinding, core: admission.carrier.members.core, cronet: admission.carrier.members.cronet }, admission.carrier, contract, '1.0.0');
      verifyCases(root, report); carriers.push(admission.carrier);
    }
    verifyCarrierFiles(args[1], carriers); return;
  }
  throw Error('Unknown TUN transport command');
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
