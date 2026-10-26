#!/usr/bin/env node
// Bounded validation of exact candidate or committed formal inputs; no publication authority.
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { appendFileSync, closeSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadReleaseContract, verifyPackagedRelease } from './release-consumer.mjs';
import { isUpstreamRelease } from './upstream-release.mjs';

export const TARGETS = Object.freeze({
  linux: { host: 'linux', arch: 'x64', target: 'linux', core: 'resources/linux/sing-box', library: 'resources/linux/libcronet.so' },
  win: { host: 'win32', arch: 'x64', target: 'windows', core: 'resources/win/sing-box.exe', library: 'resources/win/libcronet.dll' },
  'mac-x64': { host: 'darwin', arch: 'x64', target: 'macos-x64', core: 'resources/mac-x64/sing-box' },
  'mac-arm64': { host: 'darwin', arch: 'arm64', target: 'macos-arm64', core: 'resources/mac-arm64/sing-box' },
});
const COMMAND_TIMEOUT_MS = 20 * 60_000;
const SERIES_TIMEOUT_MS = 150 * 60_000;
const MAX_LOG_BYTES = 32 * 1024 * 1024;
const EXCLUDED_DHCP = '⚠ 跳过 dhcp 起核门：不触网前置只在 Linux 上可证';
const requireGate = (ok, message) => { if (!ok) throw Error(message); };
const sha256 = (value) => createHash('sha256').update(value).digest('hex');

// A strict formal contract is selected only by committed manifest bytes. The
// historical v1 release and temporary desktop default keep their original routes.
export function loadFormalGateSelection(root, manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json')))) {
  requireGate(!(Object.hasOwn(manifest, 'desktopTemporary') && Object.hasOwn(manifest, 'desktopRelease')),
    'One exclusive desktop gate selection required');
  if (Object.hasOwn(manifest, 'desktopTemporary') || !Object.hasOwn(manifest, 'desktopRelease')) return null;
  const contract = loadReleaseContract(root, manifest);
  if (!isUpstreamRelease(contract)) return null;
  requireGate(manifest.desktopRelease.contractPath === 'scripts/desktop-core/release-contract.json',
    'Formal gates require the compiled canonical upstream contract path');
  return contract;
}

// The reusable formal route retains the original CI caller's event/ref. Direct
// manual candidate dispatch still has its separate fixed-branch admission.
export function formalWorkflowContext(candidate, env = process.env) {
  requireGate(env.GITHUB_ACTIONS === 'true' && env.GITHUB_REPOSITORY === 'polaris-arch/Polaris'
    && env.GITHUB_REPOSITORY_ID === '1347028220' && /^[a-f0-9]{40}$/.test(candidate ?? '')
    && candidate === env.GITHUB_SHA && (!env.CORE_CANDIDATE || env.CORE_CANDIDATE === candidate)
    && ['push', 'pull_request', 'merge_group', 'workflow_dispatch'].includes(env.GITHUB_EVENT_NAME)
    && /^refs\/(heads|tags|pull)\/.+/.test(env.GITHUB_REF ?? '')
    && ['ci.yml', 'package.yml'].some((name) => env.GITHUB_WORKFLOW_REF === `${env.GITHUB_REPOSITORY}/.github/workflows/${name}@${env.GITHUB_REF}`)
    && /^[1-9][0-9]*$/.test(env.GITHUB_RUN_ID ?? '') && /^[1-9][0-9]*$/.test(env.GITHUB_RUN_ATTEMPT ?? '')
    && (!Object.hasOwn(env, 'POLARIS_NATIVE_DNS_FORMAL') || env.POLARIS_NATIVE_DNS_FORMAL === '1')
    && !Object.hasOwn(env, 'POLARIS_NATIVE_DNS_CANDIDATE')
    && !Object.hasOwn(env, 'POLARIS_NATIVE_PAYLOAD_VALIDATION')
    && !Object.hasOwn(env, 'POLARIS_SINGBOX_PATH') && env.POLARIS_NO_KERNEL_RUN !== '1',
  'Exact committed formal App/caller/run context required');
  return { mode: 'formal-default', repository: env.GITHUB_REPOSITORY, repositoryId: env.GITHUB_REPOSITORY_ID,
    candidate, runId: env.GITHUB_RUN_ID, attempt: env.GITHUB_RUN_ATTEMPT,
    workflowRef: env.GITHUB_WORKFLOW_REF, ref: env.GITHUB_REF };
}
export function formalGateContext(root, env = process.env, run = execFileSync) {
  const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json')));
  const contract = loadFormalGateSelection(root, manifest);
  requireGate(contract, 'Committed strict upstream formal release required');
  const context = formalWorkflowContext(env.GITHUB_SHA, env);
  // Match loadReleaseContract's explicit 8 MiB bound while retaining the
  // complete committed-byte comparison; Node's default is only 1 MiB.
  const options = { cwd: root, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024, env: { ...env } };
  for (const key of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE']) delete options.env[key];
  requireGate(String(run('git', ['rev-parse', 'HEAD'], options)).trim() === context.candidate
    && String(run('git', ['status', '--porcelain', '--untracked-files=no'], options)).trim() === '',
  'Formal App checkout differs or tracked source is dirty');
  const tree = String(run('git', ['rev-parse', 'HEAD^{tree}'], options)).trim();
  requireGate(/^[a-f0-9]{40}$/.test(tree), 'Actual formal App tree required');
  for (const path of ['src-tauri/core-manifest.json', manifest.desktopRelease.contractPath])
    requireGate(String(run('git', ['show', `HEAD:${path}`], options)) === readFileSync(join(root, path), 'utf8'),
      'Formal source selection is not the committed checkout');
  return { ...context, tree, contractSha256: manifest.desktopRelease.contractSha256,
    kernelSourceCommit: contract.sourceCommit, kernelSourceTree: contract.sourceTree,
    releaseId: contract.releaseId, tag: contract.tag, tagObject: contract.tagObject };
}
// All four carriers are inputs to host gates. --platform selects the native
// run/finalize plan, not a subset of this before/after payload proof.
export function verifyFormalGatePayloads(root, env = process.env) {
  const context = formalGateContext(root, env);
  const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json')));
  const contract = loadReleaseContract(root, manifest), payloads = {};
  for (const [platform, item] of Object.entries(TARGETS)) {
    const receipt = verifyPackagedRelease(root, manifest, platform, join(root, item.core));
    const row = contract.platforms[platform];
    payloads[platform] = { path: item.core, core: row.core, sidecar: row.sidecar,
      goVersion: row.goVersion, buildID: row.buildID, receiptFingerprint: receipt.fingerprint,
      contractSha256: context.contractSha256 };
  }
  return { context, payloads };
}

export function gatePlan(key, root = process.cwd()) {
  const target = TARGETS[key];
  requireGate(target, `Unsupported native platform: ${key}`);
  const cargo = (id, pkg, kind, name, exact) => ({
    id, program: 'cargo', args: ['test', '--locked', '-p', pkg, kind, name,
      ...(exact ? [exact] : []), '--', ...(exact ? ['--exact'] : []), '--nocapture', '--test-threads=1'],
    rustTests: true,
  });
  const plan = [
    ...['core_dep_fingerprint', 'core_build_matrix', 'core_schema_surface',
      'kernel_accepts_outbounds', 'kernel_accepts_full_config', 'network_profile_runtime',
      'subscription_update_guard_runtime', 'mesh_inbound_runtime', 'managed_mesh_emission_runtime']
      .map((name) => cargo(name, 'polaris-config-engine', '--test', name)),
    cargo('tailscale_login_kernel_check', 'polaris-mesh', '--test', 'tailscale_login_kernel_check'),
    cargo('bundled_core_wire', 'polaris-singbox-grpc', '--test', 'bundled_core_wire', 'vendored_proto_matches_every_bundled_core'),
    { id: 'native_clash_mode', program: 'cargo', rustTests: true,
      args: ['test', '--locked', '-p', 'polaris-singbox-grpc', '--test', 'native_clash_mode',
        'native_core_lists_and_switches_the_two_compiled_modes', '--', '--exact', '--ignored', '--nocapture', '--test-threads=1'] },
    ...['local_import_round_trips_through_the_bundled_core',
      'remote_subscription_protocol_parity_round_trips_through_the_bundled_core',
      'remote_openvpn_static_key_mode_omits_tls_and_checks_with_core'].map((name) => ({
      id: name, program: 'cargo', rustTests: true,
      args: ['test', '--locked', '-p', 'polaris-net-stack', '--lib', `singbox_import::tests::${name}`,
        '--', '--exact', '--nocapture', '--test-threads=1'],
    })),
  ];
  if (target.library) plan.push({
    id: 'native_cronet_payload', program: key === 'win' ? 'python' : 'python3', rustTests: false, proofRequired: true,
    args: ['scripts/native-payload-proof.py', '--platform', key, '--core', join(root, target.core),
      '--library', join(root, target.library), '--output', join(root, 'native-dns-evidence', key, 'native-cronet.json')],
  });
  return plan;
}

// One existing crate test has a Linux-only fallback. Supply its exact already
// verified native payload only to that child; formal Cronet proof and App
// lifecycle continue to reject a process-wide core-path override.
export function gateEnvironment(gate, key, root, env) {
  requireGate(TARGETS[key], `Unsupported native platform: ${key}`);
  if (env.POLARIS_NATIVE_DNS_FORMAL !== '1') return { ...env };
  requireGate(!Object.hasOwn(env, 'POLARIS_SINGBOX_PATH'), 'Formal gates must not inherit a core path override');
  if (gate.id !== 'native_clash_mode') return { ...env };
  const expected = gatePlan(key, root).find((item) => item.id === 'native_clash_mode');
  requireGate(gate.program === expected.program && JSON.stringify(gate.args) === JSON.stringify(expected.args),
    'Exact native clash-mode command required for its host-path override');
  return { ...env, POLARIS_SINGBOX_PATH: join(root, TARGETS[key].core) };
}

export function classifyResult(gate, raw, key) {
  const text = raw.output ?? '';
  const summaries = [...text.matchAll(/test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;/g)];
  const testsPassed = summaries.reduce((sum, match) => sum + Number(match[1]), 0);
  const testsIgnored = summaries.reduce((sum, match) => sum + Number(match[3]), 0);
  const exclusions = gate.id === 'network_profile_runtime' && key !== 'linux'
    && text.includes(EXCLUDED_DHCP) ? [EXCLUDED_DHCP] : [];
  const visible = exclusions.length ? text.replaceAll(EXCLUDED_DHCP, '') : text;
  // libtest may prefix nocapture output with "test name ... " on the same line.
  const skipped = /(?:⚠|⛔)[^\n]*(?:跳过|skip)|NOT_EXECUTED|\bnot executed:/i.test(visible);
  const passed = raw.status === 0 && !raw.error && !raw.timedOut && !raw.cleanupUncertain
    && !raw.interrupted
    && !skipped && (!gate.rustTests || (testsPassed > 0 && testsIgnored === 0))
    && (!gate.proofRequired || raw.proofValid === true);
  return { id: gate.id, state: passed ? 'passed' : 'failed', exitCode: raw.status ?? null,
    testsPassed: gate.rustTests ? testsPassed : null, testsIgnored: gate.rustTests ? testsIgnored : null,
    platformExclusions: exclusions, skippedEvidence: skipped, timedOut: Boolean(raw.timedOut),
    cleanupUncertain: Boolean(raw.cleanupUncertain), interrupted: Boolean(raw.interrupted), error: raw.error ?? null,
    logSha256: raw.logSha256 ?? null, logBytes: raw.logBytes ?? null,
    ...(gate.proofRequired ? { proofValid: raw.proofValid === true } : {}) };
}

// Tests inject an in-memory executor. The CLI never accepts an alternate executor.
export async function collectGates(plan, key, execute, checkpoint = () => {}) {
  const results = [];
  let stopped = false;
  for (const gate of plan) {
    let result;
    if (stopped) result = { id: gate.id, state: 'blocked', error: 'Runner interrupted or earlier child cleanup is uncertain' };
    else {
      try { result = classifyResult(gate, await execute(gate), key); }
      catch (error) { result = { id: gate.id, state: 'failed', error: String(error.message ?? error) }; }
    }
    results.push(result);
    stopped ||= Boolean(result.cleanupUncertain || result.interrupted);
    checkpoint(results);
  }
  return results;
}

export function executeCommand(gate, { root, env, directory, timeout, captureStdout = false }) {
  const logPath = join(directory, `${gate.id}.log`);
  writeFileSync(logPath, `${JSON.stringify({ program: gate.program, args: gate.args })}\n`);
  if (timeout <= 0) return Promise.resolve({ status: null, error: 'Overall candidate gate deadline exhausted', output: '' });
  return new Promise((done) => {
    const fd = openSync(logPath, 'a');
    let bytes = 0, timedOut = false, error = null, cleanupUncertain = false, settled = false, interrupted = false;
    const stdout = [];
    const child = spawn(gate.program, gate.args, { cwd: root, env, shell: false,
      detached: process.platform !== 'win32', stdio: ['ignore', 'pipe', 'pipe'] });
    const terminate = (reason) => {
      error ||= reason;
      if (!child.pid) return;
      try {
        if (process.platform === 'win32') {
          const killed = spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { encoding: 'utf8', timeout: 10_000 });
          cleanupUncertain ||= killed.status !== 0;
        } else process.kill(-child.pid, 'SIGKILL');
      } catch (killError) { cleanupUncertain ||= killError.code !== 'ESRCH'; }
    };
    const timer = setTimeout(() => { timedOut = true; terminate('Gate deadline exceeded'); }, timeout);
    const hardTimer = setTimeout(() => { cleanupUncertain = true; terminate('Child did not settle after termination'); finish(null); }, timeout + 30_000);
    const interrupt = () => { interrupted = true; terminate('Runner interrupted'); };
    process.once('SIGTERM', interrupt);
    process.once('SIGINT', interrupt);
    function finish(status) {
      if (settled) return;
      settled = true;
      clearTimeout(timer); clearTimeout(hardTimer);
      process.removeListener('SIGTERM', interrupt); process.removeListener('SIGINT', interrupt);
      closeSync(fd);
      const output = readFileSync(logPath);
      done({ processId: child.pid ?? null, status, error, timedOut, cleanupUncertain, interrupted, output: output.toString('utf8'),
        ...(captureStdout ? { stdout: Buffer.concat(stdout).toString('utf8') } : {}),
        logSha256: sha256(output), logBytes: output.length });
    }
    const consume = (chunk) => {
      if (settled) return;
      bytes += chunk.length;
      if (bytes > MAX_LOG_BYTES) { terminate('Gate log budget exceeded'); return; }
      appendFileSync(fd, chunk);
      process.stdout.write(chunk);
    };
    child.stdout.on('data', (chunk) => {
      if (captureStdout && !settled && bytes + chunk.length <= MAX_LOG_BYTES) stdout.push(chunk);
      consume(chunk);
    });
    child.stderr.on('data', consume);
    child.once('error', (cause) => { error = cause.message; });
    child.once('close', (status) => finish(status));
  });
}

export function summaryFor(key, env, plan, results = [], failure = null) {
  const complete = results.length === plan.length && plan.every((gate, index) =>
    results[index]?.id === gate.id && results[index].state === 'passed');
  return {
    schema: 'polaris-native-dns-runtime-gates-v1', platform: key,
    selection: env.POLARIS_NATIVE_DNS_FORMAL === '1' ? 'formal-default' : 'manual-candidate',
    candidate: env.CORE_CANDIDATE ?? null, runId: env.GITHUB_RUN_ID ?? null,
    attempt: env.GITHUB_RUN_ATTEMPT ?? null, workflowRef: env.GITHUB_WORKFLOW_REF ?? null,
    planSha256: sha256(JSON.stringify(plan)), requiredGates: plan.map((gate) => gate.id),
    gates: results, mandatoryGatesPassed: complete && !failure, failure,
    appAccepted: false, packagedAcceptance: false, publicationEligible: false,
    sealedInputCompatibility: 'unvalidated; historical B609 profile remains unchanged',
    nativePolicyBehavior: 'ordinary gates only; fresh cross-repository policy tests and App acceptance remain separate',
  };
}

async function main() {
  const [mode, platformArg, ...extra] = process.argv.slice(2);
  requireGate(['run', 'finalize', 'verify-formal'].includes(mode) && platformArg?.startsWith('--platform=') && extra.length === 0,
    'Usage: native-dns-gates.mjs run|finalize|verify-formal --platform=linux|win|mac-x64|mac-arm64 (verify-formal checks all four payloads; --platform selects run/finalize host gates)');
  const key = platformArg.slice('--platform='.length), root = process.cwd(), target = TARGETS[key];
  requireGate(target, `Unsupported native platform: ${key}`);
  if (mode === 'verify-formal') { console.log(JSON.stringify(verifyFormalGatePayloads(root))); return; }
  const plan = gatePlan(key, root), directory = join(root, 'native-dns-evidence', key);
  mkdirSync(directory, { recursive: true });
  const summaryPath = join(directory, 'summary.json');
  const save = (summary) => writeFileSync(summaryPath, `${JSON.stringify(summary, null, 2)}\n`);
  if (mode === 'finalize') {
    if (!existsSync(summaryPath)) save(summaryFor(key, process.env, plan, [], 'Validation did not start; inspect workflow setup/acquisition failure'));
    const summary = JSON.parse(readFileSync(summaryPath, 'utf8'));
    requireGate(summary.schema === 'polaris-native-dns-runtime-gates-v1' && summary.platform === key
      && summary.candidate === process.env.CORE_CANDIDATE && summary.runId === process.env.GITHUB_RUN_ID
      && summary.attempt === process.env.GITHUB_RUN_ATTEMPT
      && summary.selection === (process.env.POLARIS_NATIVE_DNS_FORMAL === '1' ? 'formal-default' : 'manual-candidate'), 'Summary belongs to another candidate/run/attempt');
    if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY,
      `### Native DNS candidate: ${key}\n\nMandatory ordinary gates: ${summary.mandatoryGatesPassed ? 'passed' : 'failed or incomplete'}. App/package acceptance: false. Sealed B609 path: unvalidated.\n\n`
      + summary.gates.map((gate) => `- ${gate.id}: ${gate.state}\n`).join('') + (summary.failure ? `\n${summary.failure}\n` : ''));
    return;
  }
  let results = [], failure = null, inputEvidence = null;
  save(summaryFor(key, process.env, plan, results, 'Validation is still in progress'));
  try {
    requireGate(process.env.POLARIS_NO_KERNEL_RUN !== '1', 'Local no-kernel boundary forbids candidate execution');
    requireGate(process.platform === target.host && process.arch === target.arch, 'Native runner architecture differs');
    const { candidateContext, verifyCandidatePayload, loadCandidate } = await import('./native-dns-candidate.mjs');
    const formal = process.env.POLARIS_NATIVE_DNS_FORMAL === '1';
    const context = formal ? formalGateContext(root) : candidateContext(process.env.CORE_CANDIDATE, process.env, root);
    const binding = formal ? loadFormalGateSelection(root) : loadCandidate(root);
    const verifyAll = async () => {
      if (formal) return verifyFormalGatePayloads(root).payloads;
      const payloads = {};
      for (const [platform, item] of Object.entries(TARGETS)) {
        const { row, receipt } = await verifyCandidatePayload(root, platform, join(root, item.core));
        payloads[platform] = { path: item.core, core: row.core, sidecar: row.sidecar,
          goVersion: row.goVersion, buildID: row.buildID, bindingSha256: receipt.bindingSha256 };
      }
      return payloads;
    };
    const before = await verifyAll();
    inputEvidence = { schema: 'polaris-native-dns-runtime-inputs-v1', context,
      kernelSourceCommit: binding.sourceCommit, kernelSourceTree: binding.sourceTree,
      releaseId: binding.releaseId, tag: binding.tag, tagObject: binding.tagObject,
      before, after: null, appAccepted: false };
    const saveInputs = () => writeFileSync(join(directory, 'input-verification.json'), `${JSON.stringify(inputEvidence, null, 2)}\n`);
    saveInputs();
    const deadline = Date.now() + SERIES_TIMEOUT_MS;
    const env = { ...process.env, POLARIS_REQUIRE_KERNEL_GATE: '1', POLARIS_KERNEL_GATE_TARGET: target.target,
      ...(formal ? {} : { POLARIS_SINGBOX_PATH: join(root, target.core) }), POLARIS_TEST_CORE: join(root, target.core), CARGO_TERM_COLOR: 'never' };
    results = await collectGates(plan, key, async (gate) => {
      const raw = await executeCommand(gate, { root, env: gateEnvironment(gate, key, root, env), directory,
        timeout: Math.min(COMMAND_TIMEOUT_MS, deadline - Date.now()) });
      if (gate.proofRequired && raw.status === 0) {
        try {
          const proof = JSON.parse(readFileSync(join(directory, 'native-cronet.json'), 'utf8'));
          raw.proofValid = proof.schema === 'polaris-native-payload-proof-v1'
            && proof.transport?.candidate === env.CORE_CANDIDATE
            && String(proof.transport?.runId) === env.GITHUB_RUN_ID
            && String(proof.transport?.attempt) === env.GITHUB_RUN_ATTEMPT
            && proof.input?.candidate === env.CORE_CANDIDATE;
        } catch (error) { raw.error = `Missing/invalid native proof: ${error.message}`; }
      }
      return raw;
    }, (partial) => {
      results = [...partial]; save(summaryFor(key, process.env, plan, results, 'Validation is still in progress'));
    });
    inputEvidence.after = await verifyAll();
    requireGate(JSON.stringify(inputEvidence.before) === JSON.stringify(inputEvidence.after), 'Candidate payload identity changed during validation');
    saveInputs();
  } catch (error) { failure = String(error.message ?? error); }
  const summary = summaryFor(key, process.env, plan, results, failure);
  if (inputEvidence) summary.inputVerificationSha256 = sha256(readFileSync(join(directory, 'input-verification.json')));
  save(summary);
  if (!summary.mandatoryGatesPassed) process.exitCode = 1;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
