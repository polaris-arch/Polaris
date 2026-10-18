#!/usr/bin/env node
/** Current-commit Coex export evidence, using the existing full workspace test exactly once.
 * No dependencies, fixture rewriting, native networking, extra cargo invocation, or state normalization.
 */
import { createHash } from 'node:crypto';
import { execFileSync, spawn } from 'node:child_process';
import { appendFileSync, existsSync, lstatSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { registerHooks } from 'node:module';

export const TEST_COMMAND = 'cargo test --workspace --no-fail-fast';
export const EXPORT_TEST = 'runtime::proxy::coex_observer::tests::original_hook_ready_drives_worker_event_and_state';
export const STATE_KEYS = ['crashed', 'latest', 'staleBusy', 'stopped', 'windowsPartial', 'windowsUnavailable'];
export const SOURCE_PATHS = [
  'src-tauri/src/commands/coexistence_snapshot.rs',
  'src-tauri/src/runtime/proxy.rs',
  'src-tauri/src/runtime/proxy/coex_observer.rs',
  'src-tauri/src/runtime/proxy/coex_observer/tests/mod.rs',
  'src-tauri/src/runtime/proxy/hot_switch.rs',
  'src-tauri/src/runtime/proxy/lifecycle.rs',
  'src-tauri/src/runtime/proxy/network_monitor.rs',
  'src-tauri/src/runtime/proxy/recovery.rs',
  'src-tauri/src/runtime/proxy/startup.rs',
];
export const DECODER_PATHS = [
  'ui/src/contracts/coex-runtime.ts',
  'ui/src/contracts/coex-snapshot.ts',
  'ui/src/contracts/coex-windows.ts',
];
const FIXTURE = 'ui/src/contracts/coex-runtime.rust.fixture.json';
export const sha256 = value => createHash('sha256').update(value).digest('hex');
const record = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const requireThat = (condition, message) => { if (!condition) throw new Error(message); };
export function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (record(value)) return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonical(value[key])}`).join(',')}}`;
  return JSON.stringify(value);
}
export function stateDifferences(expected, actual, path = '$') {
  if (canonical(expected) === canonical(actual)) return [];
  if (record(expected) && record(actual)) {
    return [...new Set([...Object.keys(expected), ...Object.keys(actual)])].sort().flatMap(key => {
      const child = `${path}[${JSON.stringify(key)}]`;
      if (!Object.hasOwn(expected, key)) return [{ path: child, kind: 'added', actual: actual[key] }];
      if (!Object.hasOwn(actual, key)) return [{ path: child, kind: 'missing', expected: expected[key] }];
      return stateDifferences(expected[key], actual[key], child);
    });
  }
  if (Array.isArray(expected) && Array.isArray(actual)) {
    return Array.from({ length: Math.max(expected.length, actual.length) }, (_, index) => {
      const child = `${path}[${index}]`;
      if (index >= expected.length) return [{ path: child, kind: 'added', actual: actual[index] }];
      if (index >= actual.length) return [{ path: child, kind: 'missing', expected: expected[index] }];
      return stateDifferences(expected[index], actual[index], child);
    }).flat();
  }
  return [{ path, kind: 'changed', expected, actual }];
}
// This is a narrow eligibility policy for the controlled CI export, not a product
// clock guarantee. The real decoder intentionally permits Unix display-clock rollback.
export const TEMPORAL_POLICY_VERSION = 'coex-ci-acquisition-unix-v1';
export const TEMPORAL_STATES = ['latest', 'staleBusy', 'windowsPartial', 'windowsUnavailable'];
const UNIX_FIELDS = ['startedAtUnixMillis', 'endedAtUnixMillis'];
export const TEMPORAL_PATHS = TEMPORAL_STATES.flatMap(state => UNIX_FIELDS.map(field =>
  `$["${state}"]["report"]["value"]["acquisition"]["${field}"]`));
export function compareExportStates(historical, current, execution) {
  const differences = stateDifferences(historical, current);
  const temporalPaths = new Set(TEMPORAL_PATHS);
  const nonTemporalDifferences = differences.filter(diff => !temporalPaths.has(diff.path) || diff.kind !== 'changed');
  const result = {
    policyVersion: TEMPORAL_POLICY_VERSION,
    scope: 'controlled CI export evidence only; no product clock monotonicity claim',
    allowedPaths: [...TEMPORAL_PATHS], status: 'failed', stateDifferences: differences,
    acceptedTemporalDiffs: [], rejectedDifferences: [], errors: [],
  };
  try {
    validateStates(historical, 'historical states');
    validateStates(current, 'current states');
    requireThat(execution && Number.isSafeInteger(execution.startedAtMs) && execution.startedAtMs >= 0
      && Number.isSafeInteger(execution.finishedAtMs) && execution.finishedAtMs >= execution.startedAtMs,
    'temporal policy requires a safe, ordered execution window');
    const low = BigInt(execution.startedAtMs); const high = BigInt(execution.finishedAtMs);
    result.executionWindow = { startedAtMs: execution.startedAtMs, finishedAtMs: execution.finishedAtMs };
    const uint64 = (value, label) => {
      requireThat(typeof value === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(value)
        && BigInt(value) <= 18446744073709551615n, `${label}: noncanonical uint64 decimal string`);
      return BigInt(value);
    };
    const inspect = (states, label, withinWindow) => {
      const acquisitions = {};
      for (const name of TEMPORAL_STATES) {
        const report = states[name].report;
        requireThat(record(report) && report.status === 'known' && record(report.value)
          && record(report.value.acquisition), `${label}.${name}: missing known acquisition`);
        const acquisition = report.value.acquisition;
        const values = UNIX_FIELDS.map(field => {
          const value = uint64(acquisition[field], `${label}.${name}.${field}`);
          if (withinWindow) requireThat(value >= low && value <= high, `${name}.${field}: outside this execution window`);
          return value;
        });
        requireThat(values[0] <= values[1], `${label}.${name}: acquisition start follows end`);
        acquisitions[name] = { raw: acquisition, start: values[0], end: values[1] };
      }
      requireThat(canonical(acquisitions.latest.raw) === canonical(acquisitions.staleBusy.raw),
        `${label}: latest/staleBusy must retain the same acquisition`);
      // The unchanged export test finishes latest, then samples Windows partial,
      // then Windows unavailable. Equal milliseconds are allowed, inverted order is not.
      requireThat(acquisitions.latest.end <= acquisitions.windowsPartial.start
        && acquisitions.windowsPartial.end <= acquisitions.windowsUnavailable.start,
      `${label}: controlled acquisition sequence is reversed`);
    };
    inspect(historical, 'historical', false);
    inspect(current, 'current', true);
    result.acceptedTemporalDiffs = differences.filter(diff => temporalPaths.has(diff.path) && diff.kind === 'changed');
    result.validatedConstraints = ['eight canonical uint64 strings', 'all eight in this execution window',
      'each start <= end', 'latest/staleBusy identical acquisition', 'latest then Windows partial then Windows unavailable'];
  } catch (error) { result.errors.push(String(error)); }
  // Every raw row has one outcome. A failed shared temporal qualification rejects
  // all otherwise-allowlisted rows, retaining its exact constraint failure reasons.
  const accepted = new Set(result.acceptedTemporalDiffs);
  result.rejectedDifferences = differences.filter(diff => !accepted.has(diff)).map(diff => ({
    ...diff,
    ...(!temporalPaths.has(diff.path) ? { reason: 'outside exact temporal allowlist' }
      : diff.kind !== 'changed' ? { reason: 'structural change at temporal path' }
        : { reason: 'shared temporal qualification failed', constraintErrors: [...result.errors] }),
  }));
  if (nonTemporalDifferences.length) result.errors.push('non-temporal or structural field differences remain');
  if (!result.errors.length) result.status = 'passed';
  return result;
}

function exactKeys(value, keys, label) {
  requireThat(record(value), `${label}: expected object`);
  requireThat(canonical(Object.keys(value).sort()) === canonical([...keys].sort()), `${label}: incomplete or extra keys`);
}
export function validateStates(value, label) {
  exactKeys(value, STATE_KEYS, label);
  for (const key of STATE_KEYS) requireThat(record(value[key]), `${label}.${key}: expected state object`);
}
export function validateSnapshot(snapshot, expectedSha, compatibility) {
  requireThat(/^[a-f0-9]{40}$/.test(expectedSha), 'GITHUB_SHA is missing or invalid');
  requireThat(snapshot.commit === expectedSha, 'checkout does not match GITHUB_SHA');
  requireThat(/^[a-f0-9]{40}$/.test(snapshot.tree), 'invalid git tree identity');
  requireThat(snapshot.trackedStatus === '', 'tracked checkout differs from its commit');
  exactKeys(snapshot.sourceSha256, SOURCE_PATHS, 'source snapshot');
  exactKeys(snapshot.decoderSourceSha256, DECODER_PATHS, 'decoder source snapshot');
  exactKeys(compatibility, SOURCE_PATHS, 'fixture sourceCompatibility');
  requireThat(canonical(snapshot.sourceSha256) === canonical(compatibility), 'current nine-source hashes differ from fixture sourceCompatibility');
}
export function validateExecution(execution, log, startedAtMs, exportMtimeMs) {
  requireThat(execution.command === TEST_COMMAND, 'workspace test command changed');
  requireThat(execution.exitCode === 0 && execution.signal === null && execution.logError === null, 'workspace test or evidence capture failed');
  requireThat(Number.isFinite(startedAtMs) && execution.startedAtMs >= startedAtMs
    && execution.finishedAtMs >= execution.startedAtMs, 'invalid execution time/ordering');
  requireThat(Number.isFinite(exportMtimeMs) && Math.floor(exportMtimeMs) >= execution.startedAtMs
    && Math.floor(exportMtimeMs) <= execution.finishedAtMs, 'export was not created during this workspace test');
  // Keep raw streams; remove ANSI only from this libtest-line check.
  const plain = log.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '');
  const needle = `test ${EXPORT_TEST} ... ok`;
  const completed = plain.split(/\r?\n/).filter(line => line.trim() === needle);
  requireThat(completed.length === 1, 'export test must run and pass exactly once (zero, ignored, duplicate or failed is not evidence)');
}
function jsonFile(path) { return JSON.parse(readFileSync(path, 'utf8')); }
function writeJson(path, value) { writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`, { flag: 'wx' }); }
function sourceSnapshot(root) {
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
  return {
    commit: git('rev-parse', 'HEAD'), tree: git('rev-parse', 'HEAD^{tree}'),
    trackedStatus: git('status', '--porcelain=v1', '--untracked-files=no'),
    fixtureSha256: sha256(readFileSync(join(root, FIXTURE))),
    sourceSha256: Object.fromEntries(SOURCE_PATHS.map(path => [path, sha256(readFileSync(join(root, path)))])),
    decoderSourceSha256: Object.fromEntries(DECODER_PATHS.map(path => [path, sha256(readFileSync(join(root, path)))])),
  };
}
function evidenceDirectory(env) {
  requireThat(env.COEX_EVIDENCE_DIR && env.RUNNER_TEMP, 'missing evidence directory or RUNNER_TEMP');
  const base = realpathSync(env.RUNNER_TEMP);
  const directory = realpathSync(env.COEX_EVIDENCE_DIR);
  const child = relative(base, directory);
  requireThat(child && !child.startsWith(`..${sep}`) && child !== '..' && !isAbsolute(child), 'evidence directory must be inside RUNNER_TEMP');
  return directory;
}
export function prepare(root, env) {
  requireThat(env.RUNNER_TEMP && env.GITHUB_OUTPUT, 'prepare requires RUNNER_TEMP and GITHUB_OUTPUT');
  const preparedAtMs = Date.now();
  const directory = mkdtempSync(join(realpathSync(env.RUNNER_TEMP), 'polaris-coex-'));
  const exportPath = join(directory, 'coex-runtime-current.json');
  // Make even a failed preflight's diagnostics reachable by the always-upload step.
  requireThat(!/[\r\n]/.test(directory), 'invalid evidence path');
  appendFileSync(env.GITHUB_OUTPUT, `directory=${directory}\nexport_path=${exportPath}\n`);
  try {
    requireThat(!existsSync(exportPath), 'fresh export path already exists');
    const fixture = jsonFile(join(root, FIXTURE));
    validateStates(fixture.states, 'historical states');
    const snapshot = sourceSnapshot(root);
    validateSnapshot(snapshot, env.GITHUB_SHA, fixture.sourceCompatibility?.sourceSha256);
    requireThat(sha256(canonical(fixture.states)) === fixture.sourceCompatibility.originalStatesCanonicalSha256,
      'historical states digest changed');
    writeJson(join(directory, 'historical-fixture.json'), fixture);
    writeJson(join(directory, 'before.json'), {
      ...snapshot, preparedAtMs, exportPath, root: realpathSync(root),
      github: { sha: env.GITHUB_SHA, runId: env.GITHUB_RUN_ID, runAttempt: env.GITHUB_RUN_ATTEMPT, job: env.GITHUB_JOB,
        runUrl: `${env.GITHUB_SERVER_URL}/${env.GITHUB_REPOSITORY}/actions/runs/${env.GITHUB_RUN_ID}` },
    });
  } catch (error) {
    writeJson(join(directory, 'prepare-error.json'), { error: String(error) });
    throw error;
  }
  return directory;
}
/** Static GitHub custom shell dispatch; no workflow expression in step.shell. */
export function workspaceInvocation(platform, scriptPath, script) {
  requireThat(script.trim() === TEST_COMMAND, 'capture accepts only the unchanged full workspace test command');
  if (platform === 'Windows') return { command: 'pwsh', args: ['-command',
    `$ErrorActionPreference = 'stop'\n${script}\nif ((Test-Path -LiteralPath variable:\\LASTEXITCODE)) { exit $LASTEXITCODE }`] };
  requireThat(platform === 'Linux' || platform === 'macOS', 'unsupported runner platform');
  return { command: 'bash', args: ['--noprofile', '--norc', '-e', '-o', 'pipefail', scriptPath] };
}
async function runUncaptured(invocation, root, env) {
  const childEnv = { ...env };
  delete childEnv.POLARIS_COEX_RUNTIME_FIXTURE;
  const child = spawn(invocation.command, invocation.args, { cwd: root, env: childEnv, stdio: 'inherit' });
  let error;
  child.on('error', caught => { error = caught; });
  const completion = await new Promise(done => child.on('close', (code, signal) => done({ code, signal })));
  if (error) throw error;
  return completion.code === 0 && completion.signal === null ? 0 : (completion.code || 1);
}

/** Load the actual checked-in TypeScript consumer. Node 26 strips types natively;
 * only the existing two extensionless relative imports need an explicit resolution hook.
 * No generated decoder, substitute schema, package installation or source rewrite is used.
 */
export async function decodeCurrentStates(root, states) {
  validateStates(states, 'current decoder input');
  const urls = DECODER_PATHS.map(path => pathToFileURL(join(root, path)).href);
  const hashes = () => Object.fromEntries(DECODER_PATHS.map(path => [path, sha256(readFileSync(join(root, path)))]));
  const sourceSha256 = hashes();
  const edges = new Map([
    [urls[0] + '|./coex-snapshot', urls[1]],
    [urls[1] + '|./coex-windows', urls[2]],
  ]);
  const hooks = registerHooks({ resolve(specifier, context, nextResolve) {
    const target = edges.get(context.parentURL + '|' + specifier);
    return nextResolve(target ?? specifier, context);
  } });
  let decodeCoexRuntimeState;
  try { ({ decodeCoexRuntimeState } = await import(urls[0])); }
  finally { hooks.deregister(); }
  requireThat(typeof decodeCoexRuntimeState === 'function', 'actual runtime decoder export missing');
  const results = STATE_KEYS.map(name => {
    try {
      const raw = states[name];
      const decoded = decodeCoexRuntimeState(structuredClone(raw));
      requireThat(canonical(decoded) === canonical(raw), `decoder changed or dropped fields in ${name}`);
      return { name, status: 'passed', inputSha256: sha256(canonical(raw)), decodedSha256: sha256(canonical(decoded)) };
    } catch (error) { return { name, status: 'failed', error: String(error) }; }
  });
  requireThat(canonical(hashes()) === canonical(sourceSha256), 'decoder sources changed while loading/decoding');
  return { entry: DECODER_PATHS[0], exportName: 'decodeCoexRuntimeState', nodeVersion: process.version,
    sourceSha256, results, status: results.every(result => result.status === 'passed') ? 'passed' : 'failed' };
}

export async function capture(root, scriptPath, env) {
  const script = readFileSync(scriptPath, 'utf8');
  const invocation = workspaceInvocation(env.RUNNER_OS, scriptPath, script);
  if (env.RUNNER_OS !== 'Linux') return runUncaptured(invocation, root, env);
  const directory = evidenceDirectory(env);
  const before = jsonFile(join(directory, 'before.json'));
  requireThat(before.root === realpathSync(root), 'capture root differs from prepared checkout');
  requireThat(!existsSync(before.exportPath), 'export already exists before workspace test');
  const snapshot = sourceSnapshot(root);
  requireThat(canonical(snapshot) === canonical({ commit: before.commit, tree: before.tree, trackedStatus: before.trackedStatus,
    fixtureSha256: before.fixtureSha256, sourceSha256: before.sourceSha256, decoderSourceSha256: before.decoderSourceSha256 }), 'checkout changed before workspace test');
  writeFileSync(join(directory, 'workflow-test-command.sh'), script, { flag: 'wx' });
  for (const name of ['cargo-test.log', 'cargo-test.stdout.log', 'cargo-test.stderr.log']) writeFileSync(join(directory, name), '', { flag: 'wx' });
  const startedAtMs = Date.now();
  let logError = null;
  const child = spawn(invocation.command, invocation.args, {
    cwd: root, env: { ...env, POLARIS_COEX_RUNTIME_FIXTURE: before.exportPath }, stdio: ['ignore', 'pipe', 'pipe'],
  });
  const copy = (chunk, name, terminal) => {
    try { appendFileSync(join(directory, name), chunk); appendFileSync(join(directory, 'cargo-test.log'), chunk); terminal.write(chunk); }
    catch (error) { logError ??= String(error); child.kill('SIGTERM'); }
  };
  child.stdout.on('data', chunk => copy(chunk, 'cargo-test.stdout.log', process.stdout));
  child.stderr.on('data', chunk => copy(chunk, 'cargo-test.stderr.log', process.stderr));
  child.on('error', error => { logError ??= String(error); });
  const completion = await new Promise(resolveCompletion => child.on('close', (exitCode, signal) => resolveCompletion({ exitCode, signal })));
  const execution = { command: TEST_COMMAND, scriptSha256: sha256(script), startedAtMs, finishedAtMs: Date.now(), ...completion, logError };
  writeJson(join(directory, 'cargo-test-execution.json'), execution);
  return completion.exitCode === 0 && completion.signal === null && logError === null ? 0 : (completion.exitCode || 1);
}
export async function verify(root, env) {
  const directory = evidenceDirectory(env);
  const errors = [];
  const result = { status: 'failed', newSerializerExecution: false, errors, stateDifferences: [], acceptedTemporalDiffs: [] };
  const attempt = operation => { try { return operation(); } catch (error) { errors.push(String(error)); return undefined; } };
  const before = attempt(() => jsonFile(join(directory, 'before.json')));
  const historical = attempt(() => jsonFile(join(directory, 'historical-fixture.json')));
  const after = attempt(() => sourceSnapshot(root));
  if (after) { writeJson(join(directory, 'after.json'), after); result.after = after; }
  if (before && historical && after) attempt(() => {
    requireThat(before.root === realpathSync(root), 'verify root differs from prepared checkout');
    requireThat(before.github.sha === env.GITHUB_SHA && before.github.runId === env.GITHUB_RUN_ID
      && before.github.runAttempt === env.GITHUB_RUN_ATTEMPT && before.github.job === env.GITHUB_JOB, 'CI invocation identity changed');
    validateSnapshot(after, env.GITHUB_SHA, historical.sourceCompatibility?.sourceSha256);
    for (const key of ['commit', 'tree', 'trackedStatus', 'fixtureSha256', 'sourceSha256', 'decoderSourceSha256']) {
      requireThat(canonical(before[key]) === canonical(after[key]), `before/after ${key} changed`);
    }
    requireThat(after.fixtureSha256 === sha256(readFileSync(join(root, FIXTURE))), 'fixture source changed');
  });
  const execution = attempt(() => jsonFile(join(directory, 'cargo-test-execution.json')));
  const log = attempt(() => readFileSync(join(directory, 'cargo-test.stdout.log'), 'utf8'));
  if (execution) result.execution = execution;
  if (log !== undefined) result.stdoutSha256 = sha256(log);
  const exported = before && attempt(() => {
    requireThat(before.exportPath === join(directory, 'coex-runtime-current.json'), 'unexpected export path');
    const stat = lstatSync(before.exportPath);
    requireThat(stat.isFile() && !stat.isSymbolicLink(), 'export is not a fresh regular file');
    const bytes = readFileSync(before.exportPath);
    result.exportSha256 = sha256(bytes);
    result.exportMtimeMs = stat.mtimeMs;
    return JSON.parse(bytes.toString('utf8'));
  });
  if (before && execution && log !== undefined && exported) attempt(() => {
    validateExecution(execution, log, before.preparedAtMs, result.exportMtimeMs);
    result.newSerializerExecution = true;
  });
  if (historical && exported) attempt(() => {
    // Classify all raw rows before shape/provenance checks can reject this export.
    const comparison = compareExportStates(historical.states, exported.states, execution);
    const { stateDifferences: rawDifferences, acceptedTemporalDiffs, ...temporalPolicy } = comparison;
    result.stateDifferences = rawDifferences;
    result.acceptedTemporalDiffs = acceptedTemporalDiffs;
    result.temporalPolicy = temporalPolicy;
    validateStates(exported.states, 'current states');
    validateStates(historical.states, 'historical states');
    requireThat(typeof exported.provenance === 'string' && exported.provenance.length > 0, 'missing native export provenance');
    result.originalStatesCanonicalSha256 = sha256(canonical(historical.states));
    result.currentStatesCanonicalSha256 = sha256(canonical(exported.states));
    requireThat(comparison.status === 'passed', `current serialized states violate ${TEMPORAL_POLICY_VERSION}: ${comparison.errors.join('; ')}`);
  });
  if (exported && before && after) {
    try {
      result.decoder = await decodeCurrentStates(root, exported.states);
      requireThat(canonical(result.decoder.sourceSha256) === canonical(before.decoderSourceSha256)
        && canonical(result.decoder.sourceSha256) === canonical(after.decoderSourceSha256), 'decoder sources differ from prepared/post-test identity');
      requireThat(result.decoder.status === 'passed', 'actual decodeCoexRuntimeState rejected current exported states');
      const finalSnapshot = sourceSnapshot(root);
      requireThat(canonical(finalSnapshot) === canonical(after), 'checkout changed while decoding current export');
    } catch (error) { errors.push(String(error)); }
  }
  if (!result.decoder && !errors.length) errors.push('decoder execution is required');
  writeJson(join(directory, 'state-diff.json'), result.stateDifferences);
  writeJson(join(directory, 'accepted-temporal-diff.json'), result.acceptedTemporalDiffs);
  if (!errors.length && result.newSerializerExecution && result.decoder?.status === 'passed') result.status = 'passed';
  writeJson(join(directory, 'verification.json'), result);
  requireThat(result.status === 'passed', `Coex export verification failed: ${errors.join('; ')}`);
  return result;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [mode, scriptPath] = process.argv.slice(2);
    if (mode === 'prepare') prepare(process.cwd(), process.env);
    else if (mode === 'capture') process.exitCode = await capture(process.cwd(), scriptPath, process.env);
    else if (mode === 'verify') await verify(process.cwd(), process.env);
    else throw new Error('usage: check-coex-runtime-export.mjs prepare|capture <workflow-script>|verify');
  } catch (error) { console.error(String(error)); process.exitCode = 1; }
}
