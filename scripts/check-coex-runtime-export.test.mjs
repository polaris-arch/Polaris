import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  TEST_COMMAND, EXPORT_TEST, SOURCE_PATHS, DECODER_PATHS, STATE_KEYS, canonical, sha256, stateDifferences,
  TEMPORAL_POLICY_VERSION, TEMPORAL_STATES, TEMPORAL_PATHS, compareExportStates,
  validateStates, validateSnapshot, validateExecution, workspaceInvocation, prepare, capture, verify,
} from './check-coex-runtime-export.mjs';

const REPO_ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const historicalStates = JSON.parse(readFileSync(join(REPO_ROOT, 'ui/src/contracts/coex-runtime.rust.fixture.json'), 'utf8')).states;
const states = () => structuredClone(historicalStates);
const decoderHashes = () => Object.fromEntries(DECODER_PATHS.map(path => [path, sha256(readFileSync(join(REPO_ROOT, path)))]));
const hashes = () => Object.fromEntries(SOURCE_PATHS.map(path => [path, sha256(path)]));
const snapshot = () => ({ commit: 'a'.repeat(40), tree: 'b'.repeat(40), trackedStatus: '', sourceSha256: hashes(), decoderSourceSha256: decoderHashes() });
const execution = () => ({ command: TEST_COMMAND, exitCode: 0, signal: null, logError: null, startedAtMs: 100, finishedAtMs: 200 });
const passedLine = `test ${EXPORT_TEST} ... ok\n`;

test('canonical comparison preserves every value and array position; only object key order is irrelevant', () => {
  assert.equal(canonical({ b: [1, null], a: { z: false } }), canonical({ a: { z: false }, b: [1, null] }));
  for (const [expected, actual] of [[['1'], [1]], [[1, 2], [2, 1]], [{ x: null }, {}], [{ status: 'unknown' }, { status: 'known' }]]) {
    assert.notEqual(canonical(expected), canonical(actual));
    assert.ok(stateDifferences(expected, actual).length > 0);
  }
  assert.deepEqual(stateDifferences({ rows: [{ value: '3' }] }, { rows: [{ value: '4' }] }),
    [{ path: '$["rows"][0]["value"]', kind: 'changed', expected: '3', actual: '4' }]);
});
test('all six states and all nine current sources are mandatory with no extra keys', () => {
  validateStates(states(), 'fixture');
  for (const key of STATE_KEYS) { const value = states(); delete value[key]; assert.throws(() => validateStates(value, 'fixture')); }
  assert.throws(() => validateStates({ ...states(), extra: {} }, 'fixture'));
  assert.throws(() => validateStates({ ...states(), latest: null }, 'fixture'));
  validateSnapshot(snapshot(), 'a'.repeat(40), hashes());
  for (const key of SOURCE_PATHS) { const value = snapshot(); delete value.sourceSha256[key]; assert.throws(() => validateSnapshot(value, value.commit, hashes())); }
  for (const key of DECODER_PATHS) { const value = snapshot(); delete value.decoderSourceSha256[key]; assert.throws(() => validateSnapshot(value, value.commit, hashes())); }
  for (const value of [
    { ...snapshot(), commit: 'c'.repeat(40) }, { ...snapshot(), tree: '' },
    { ...snapshot(), trackedStatus: ' M source.rs' },
    { ...snapshot(), sourceSha256: { ...hashes(), extra: '1' } },
    { ...snapshot(), sourceSha256: { ...hashes(), [SOURCE_PATHS[0]]: '0'.repeat(64) } },
  ]) assert.throws(() => validateSnapshot(value, 'a'.repeat(40), hashes()));
  assert.throws(() => validateSnapshot(snapshot(), '', hashes()));
});
test('zero/ignored/failed/duplicate export tests, nonzero cargo, failed capture and stale exports fail closed', () => {
  validateExecution(execution(), `\x1b[32m${passedLine.trim()}\x1b[0m\n`, 90, 150.9);
  for (const log of ['', 'running 0 tests\ntest result: ok. 0 passed\n', passedLine.replace(' ... ok', ' ... ignored'),
    passedLine.replace(' ... ok', ' ... FAILED'), passedLine + passedLine]) {
    assert.throws(() => validateExecution(execution(), log, 90, 150));
  }
  for (const change of [{ exitCode: 7 }, { signal: 'SIGTERM' }, { logError: 'disk full' },
    { command: 'cargo test -p polaris filter' }, { startedAtMs: 80 }, { finishedAtMs: 99 }]) {
    assert.throws(() => validateExecution({ ...execution(), ...change }, passedLine, 90, 150));
  }
  for (const time of [99, 201, NaN]) assert.throws(() => validateExecution(execution(), passedLine, 90, time));
});

function assertCompletePartition(result) {
  const rejected = result.rejectedDifferences ?? result.temporalPolicy?.rejectedDifferences ?? [];
  const accepted = result.acceptedTemporalDiffs;
  const rawRejected = rejected.map(({ reason, constraintErrors, ...raw }) => {
    assert.ok(typeof reason === 'string' && reason.length > 0);
    if (reason === 'shared temporal qualification failed') {
      assert.ok(Array.isArray(constraintErrors) && constraintErrors.length > 0);
      assert.ok(constraintErrors.every(error => typeof error === 'string' && error.length > 0));
    }
    return raw;
  });
  const key = row => `${row.path}\0${row.kind}`;
  const acceptedKeys = new Set(accepted.map(key));
  assert.equal(acceptedKeys.size, accepted.length);
  assert.equal(new Set(rawRejected.map(key)).size, rawRejected.length);
  assert.ok(rawRejected.every(row => !acceptedKeys.has(key(row))), 'accepted/rejected must be disjoint');
  assert.deepEqual([...accepted, ...rawRejected].map(canonical).sort(), result.stateDifferences.map(canonical).sort(),
    'every original raw diff must occur exactly once, with its exact original expected/actual values');
}

// These times are controlled checker inputs, not replacement native export evidence.
const controlledWindow = () => ({ ...execution(), startedAtMs: 90, finishedAtMs: 160 });
function currentStates() {
  const value = states();
  for (const [name, start, end] of [['latest', '100', '110'], ['staleBusy', '100', '110'],
    ['windowsPartial', '120', '130'], ['windowsUnavailable', '140', '150']]) {
    Object.assign(value[name].report.value.acquisition, { startedAtUnixMillis: start, endedAtUnixMillis: end });
  }
  return value;
}
function temporalLocation(path) { return JSON.parse(`[${path.slice(1).replaceAll('][', ',').slice(1, -1)}]`); }
function changePath(value, parts, replacement, remove = false) {
  let parent = value;
  for (const key of parts.slice(0, -1)) parent = parent[key];
  if (remove) delete parent[parts.at(-1)]; else parent[parts.at(-1)] = replacement;
}
test('the explicit eight-path policy retains the full raw diff, original values and history identity', () => {
  const original = states(); const current = currentStates();
  const oldBytes = canonical(original); const newBytes = canonical(current);
  const result = compareExportStates(original, current, controlledWindow());
  assertCompletePartition(result);
  assert.equal(TEMPORAL_PATHS.length, 8); assert.equal(new Set(TEMPORAL_PATHS).size, 8);
  assert.equal(result.policyVersion, 'coex-ci-acquisition-unix-v1');
  assert.equal(result.status, 'passed');
  assert.deepEqual(result.stateDifferences, stateDifferences(original, current));
  assert.equal(result.stateDifferences.length, 8);
  assert.deepEqual(result.acceptedTemporalDiffs, result.stateDifferences);
  assert.deepEqual(result.acceptedTemporalDiffs.map(row => row.path).sort(), [...TEMPORAL_PATHS].sort());
  assert.deepEqual(result.rejectedDifferences, []);
  assert.equal(canonical(original), oldBytes); assert.equal(canonical(current), newBytes);
  assert.notEqual(oldBytes, newBytes);
  // A repeated millisecond and a nonzero sampling duration are both legitimate;
  // the gate preserves acquisition order, not the historical duration/equality accident.
  const repeated = currentStates();
  for (const name of TEMPORAL_STATES) Object.assign(repeated[name].report.value.acquisition,
    { startedAtUnixMillis: '100', endedAtUnixMillis: '100' });
  assert.equal(compareExportStates(original, repeated, controlledWindow()).status, 'passed');
});
for (const path of TEMPORAL_PATHS) {
  const parts = temporalLocation(path);
  test(`temporal field ${path} rejects wrong type, noncanonical decimal and uint64 overflow`, () => {
    for (const value of [100, null, true, {}, [], '', '01', '-1', '+100', '1.0', '1e2', ' 100', '18446744073709551616']) {
      const current = currentStates(); changePath(current, parts, value);
      const result = compareExportStates(states(), current, controlledWindow());
      assertCompletePartition(result);
      assert.equal(result.status, 'failed', JSON.stringify(value));
      assert.ok(result.errors.some(error => error.includes('noncanonical uint64')), JSON.stringify(value));
      assert.deepEqual(result.acceptedTemporalDiffs, []);
      assert.deepEqual(result.stateDifferences, stateDifferences(states(), current));
    }
  });
  test(`temporal field ${path} rejects both sides of the actual execution window`, () => {
    for (const value of ['89', '161', '18446744073709551615']) {
      const current = currentStates(); changePath(current, parts, value);
      const result = compareExportStates(states(), current, controlledWindow());
      assertCompletePartition(result);
      assert.equal(result.status, 'failed');
      assert.ok(result.errors.some(error => error.includes('outside this execution window')));
      assert.deepEqual(result.acceptedTemporalDiffs, []);
    }
  });
  test(`temporal field ${path} cannot reverse its acquisition pair or disappear`, () => {
    const current = currentStates();
    const acquisition = current[parts[0]].report.value.acquisition;
    const start = parts.at(-1) === 'startedAtUnixMillis';
    changePath(current, parts, String(BigInt(acquisition[start ? 'endedAtUnixMillis' : 'startedAtUnixMillis']) + (start ? 1n : -1n)));
    const reversed = compareExportStates(states(), current, controlledWindow());
    assertCompletePartition(reversed);
    assert.equal(reversed.status, 'failed');
    assert.ok(reversed.errors.some(error => error.includes('start follows end')));
    assert.deepEqual(reversed.acceptedTemporalDiffs, []);
    const missing = currentStates(); changePath(missing, parts, undefined, true);
    const removed = compareExportStates(states(), missing, controlledWindow());
    assertCompletePartition(removed);
    assert.equal(removed.status, 'failed');
    assert.ok(removed.rejectedDifferences.some(diff => diff.path === path && diff.kind === 'missing'));
    assert.deepEqual(removed.acceptedTemporalDiffs, []);
  });
}
test('latest/stale acquisition identity and sequential Windows sampling remain required', () => {
  for (const name of ['latest', 'staleBusy']) for (const field of ['startedAtUnixMillis', 'endedAtUnixMillis']) {
    const current = currentStates(); current[name].report.value.acquisition[field] = field === 'startedAtUnixMillis' ? '101' : '111';
    const result = compareExportStates(states(), current, controlledWindow());
    assertCompletePartition(result);
    assert.equal(result.status, 'failed');
    assert.ok(result.errors.some(error => error.includes('same acquisition')));
  }
  for (const [name, field, value] of [['windowsPartial', 'startedAtUnixMillis', '109'],
    ['windowsPartial', 'endedAtUnixMillis', '141'], ['windowsUnavailable', 'startedAtUnixMillis', '129']]) {
    const current = currentStates(); current[name].report.value.acquisition[field] = value;
    const result = compareExportStates(states(), current, controlledWindow());
    assertCompletePartition(result);
    assert.equal(result.status, 'failed');
    assert.ok(result.errors.some(error => error.includes('sequence is reversed')));
    assert.deepEqual(result.acceptedTemporalDiffs, []);
  }
});
test('missing, unsafe or reversed execution windows cannot qualify temporal evidence', () => {
  for (const value of [undefined, {}, { startedAtMs: '90', finishedAtMs: 160 }, { startedAtMs: 90.5, finishedAtMs: 160 },
    { startedAtMs: -1, finishedAtMs: 160 }, { startedAtMs: 170, finishedAtMs: 160 },
    { startedAtMs: 90, finishedAtMs: Number.MAX_SAFE_INTEGER + 1 }]) {
    const result = compareExportStates(states(), currentStates(), value);
    assertCompletePartition(result);
    assert.equal(result.status, 'failed'); assert.deepEqual(result.acceptedTemporalDiffs, []);
    assert.ok(result.errors.some(error => error.includes('execution window')));
  }
});
function leaves(value, parts = []) {
  if (value !== null && typeof value === 'object' && Object.keys(value).length) {
    return Object.keys(value).flatMap(key => leaves(value[key], [...parts, Array.isArray(value) ? Number(key) : key]));
  }
  return [{ parts, value }];
}
const jsonPath = parts => '$' + parts.map(part => `[${JSON.stringify(part)}]`).join('');
test('every non-allowlisted leaf stays canonical-strict, including monotonic clocks, bindings and Unknown reasons', () => {
  const original = states(); const base = currentStates();
  const candidates = leaves(base).filter(row => !TEMPORAL_PATHS.includes(jsonPath(row.parts)));
  assert.ok(candidates.length > 100);
  for (const { parts, value } of candidates) {
    const changed = structuredClone(base);
    const replacement = typeof value === 'string' ? `${value}:changed` : typeof value === 'number' ? value + 1
      : typeof value === 'boolean' ? !value : Array.isArray(value) ? ['changed'] : 'changed';
    changePath(changed, parts, replacement);
    const result = compareExportStates(original, changed, controlledWindow());
    assertCompletePartition(result);
    assert.equal(result.status, 'failed', jsonPath(parts));
    assert.ok(result.rejectedDifferences.length > 0, jsonPath(parts));
    assert.deepEqual(result.stateDifferences, stateDifferences(original, changed));
  }
  for (const name of STATE_KEYS) {
    const changed = currentStates(); changed[name].unreviewedField = 'new';
    const result = compareExportStates(original, changed, controlledWindow());
    assertCompletePartition(result);
    assert.equal(result.status, 'failed');
    assert.ok(result.rejectedDifferences.some(diff => diff.path === `$["${name}"]["unreviewedField"]`));
  }
  const changed = currentStates(); changed.latest.report.value.acquisition.anotherAtUnixMillis = '100';
  assert.equal(compareExportStates(original, changed, controlledWindow()).status, 'failed');
});

// These are controlled checker fixtures, NOT native serializer evidence: a private temporary
// Git repository and fake cargo exercise actual prepare/capture/verify I/O without compiling.
function fixture(t, options = {}, originalStates = states()) {
  const temporary = mkdtempSync(join(tmpdir(), 'coex-checker-unit-'));
  t.after(() => rmSync(temporary, { recursive: true, force: true }));
  const root = join(temporary, 'repo'); mkdirSync(root);
  for (const path of SOURCE_PATHS) { mkdirSync(dirname(join(root, path)), { recursive: true }); writeFileSync(join(root, path), path); }
  // Copy the real current consumer and its two real imports. No fake/second decoder.
  for (const path of DECODER_PATHS) { mkdirSync(dirname(join(root, path)), { recursive: true }); writeFileSync(join(root, path), readFileSync(join(REPO_ROOT, path))); }
  writeFileSync(join(root, 'ui/package.json'), JSON.stringify({ type: 'module' }));
  const historical = { states: originalStates, producer: { historical: true }, sourceCompatibility: { sourceSha256: hashes(), originalStatesCanonicalSha256: sha256(canonical(originalStates)) } };
  const fixturePath = join(root, 'ui/src/contracts/coex-runtime.rust.fixture.json');
  mkdirSync(dirname(fixturePath), { recursive: true }); writeFileSync(fixturePath, JSON.stringify(historical));
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
  git('init', '-q'); git('add', '.'); git('-c', 'user.name=Checker Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'controlled fixture');
  const output = join(temporary, 'github-output'); writeFileSync(output, '');
  const bin = join(temporary, 'bin'); mkdirSync(bin);
  writeFileSync(join(bin, 'cargo'), `#!/usr/bin/env bash\nexec "${process.execPath}" "$FAKE_CARGO_SCRIPT" "$@"\n`, { mode: 0o700 });
  const fake = join(temporary, 'fake-cargo.mjs');
  writeFileSync(fake, `import { appendFileSync, writeFileSync } from 'node:fs';\nappendFileSync(process.env.FAKE_INVOCATIONS, JSON.stringify({ args: process.argv.slice(2), exportPath: process.env.POLARIS_COEX_RUNTIME_FIXTURE ?? null }) + '\\n');\nif (!process.env.NO_EXPORT) { const payload = JSON.parse(process.env.EXPORT_BYTES); const now = String(Date.now()); for (const name of ${JSON.stringify(['latest','staleBusy','windowsPartial','windowsUnavailable'])}) { const a = payload.states[name].report.value.acquisition; a.startedAtUnixMillis = now; a.endedAtUnixMillis = now; } writeFileSync(process.env.POLARIS_COEX_RUNTIME_FIXTURE, JSON.stringify(payload)); }\nif (!process.env.ZERO_TESTS) console.log(${JSON.stringify(passedLine.trim())});\nprocess.exitCode = Number(process.env.FAKE_EXIT || '0');\n`);
  const env = { ...process.env, RUNNER_OS: 'Linux', RUNNER_TEMP: temporary, GITHUB_OUTPUT: output, GITHUB_SHA: git('rev-parse', 'HEAD'),
    GITHUB_RUN_ID: 'fixture-run', GITHUB_RUN_ATTEMPT: '1', GITHUB_JOB: 'fixture-job', GITHUB_SERVER_URL: 'https://github.com', GITHUB_REPOSITORY: 'fixture/fixture',
    PATH: `${bin}:${process.env.PATH}`, FAKE_CARGO_SCRIPT: fake, FAKE_INVOCATIONS: join(temporary, 'invocations.jsonl'), EXPORT_BYTES: JSON.stringify({ provenance: 'controlled checker fixture only', states: states() }), ...options };
  const directory = prepare(root, env); env.COEX_EVIDENCE_DIR = directory;
  const script = join(temporary, 'workflow.sh'); writeFileSync(script, `${TEST_COMMAND}\n`);
  return { root, env, directory, script, fixturePath, historicalBytes: readFileSync(fixturePath) };
}
test('actual checker path retains evidence, passes controlled matching output, and never rewrites historical fixture', async t => {
  const f = fixture(t);
  assert.equal(await capture(f.root, f.script, f.env), 0);
  const result = await verify(f.root, f.env);
  assertCompletePartition(result);
  assert.equal(result.status, 'passed');
  assert.equal(result.newSerializerExecution, true);
  assert.equal(result.temporalPolicy.policyVersion, TEMPORAL_POLICY_VERSION);
  assert.equal(result.temporalPolicy.status, 'passed');
  assert.equal(result.stateDifferences.length, 8);
  assert.deepEqual(result.acceptedTemporalDiffs, result.stateDifferences);
  assert.notEqual(result.originalStatesCanonicalSha256, result.currentStatesCanonicalSha256);
  assert.equal(result.decoder.entry, DECODER_PATHS[0]);
  assert.equal(result.decoder.exportName, 'decodeCoexRuntimeState');
  assert.equal(result.decoder.status, 'passed');
  assert.deepEqual(result.decoder.results.map(row => row.name), STATE_KEYS);
  assert.ok(result.decoder.results.every(row => row.status === 'passed' && row.inputSha256 === row.decodedSha256));
  assert.deepEqual(result.decoder.sourceSha256, decoderHashes());
  const invocations = readFileSync(f.env.FAKE_INVOCATIONS, 'utf8').trim().split('\n').map(line => JSON.parse(line));
  assert.deepEqual(invocations, [{ args: ['test', '--workspace', '--no-fail-fast'], exportPath: join(f.directory, 'coex-runtime-current.json') }]);
  assert.deepEqual(readFileSync(f.fixturePath), f.historicalBytes);
  for (const path of ['before.json', 'after.json', 'cargo-test.log', 'cargo-test.stdout.log', 'cargo-test.stderr.log',
    'cargo-test-execution.json', 'coex-runtime-current.json', 'state-diff.json', 'accepted-temporal-diff.json', 'verification.json']) assert.ok(existsSync(join(f.directory, path)), path);
});
for (const options of [{ NO_EXPORT: '1' }, { ZERO_TESTS: '1' }, { FAKE_EXIT: '7' }]) {
  test(`actual checker rejects ${JSON.stringify(options)} and preserves failure artifacts`, async t => {
    const f = fixture(t, options);
    const exit = await capture(f.root, f.script, f.env);
    assert.equal(exit, options.FAKE_EXIT ? 7 : 0);
    await assert.rejects(verify(f.root, f.env));
    const result = JSON.parse(readFileSync(join(f.directory, 'verification.json')));
    assert.equal(result.status, 'failed');
    assert.ok(result.errors.length > 0);
    if (options.FAKE_EXIT) {
      assert.equal(result.execution.exitCode, 7);
      assert.equal(result.newSerializerExecution, false);
      assert.equal(result.temporalPolicy.status, 'passed');
      assert.equal(result.decoder.status, 'passed');
      assert.equal(result.acceptedTemporalDiffs.length, 8);
      assert.ok(result.errors.some(error => error.includes('workspace test or evidence capture failed')));
    }
    assert.ok(existsSync(join(f.directory, 'cargo-test-execution.json')));
    assert.ok(existsSync(join(f.directory, 'state-diff.json')));
  });
}
test('actual checker preserves full field mismatch, and rejects a changed source/checkout', async t => {
  const current = states(); current.latest.binding.sessionId.value = '4';
  const f = fixture(t, { EXPORT_BYTES: JSON.stringify({ provenance: 'controlled fixture', states: current }) });
  await capture(f.root, f.script, f.env);
  writeFileSync(join(f.root, SOURCE_PATHS[0]), 'changed after test');
  await assert.rejects(verify(f.root, f.env));
  const result = JSON.parse(readFileSync(join(f.directory, 'verification.json')));
  assert.equal(result.status, 'failed');
  assert.ok(result.errors.some(error => error.includes('checkout')));
  assert.equal(result.stateDifferences.length, 9);
  assert.equal(result.acceptedTemporalDiffs.length, 8);
  assert.deepEqual(result.temporalPolicy.rejectedDifferences, [{ path: '$["latest"]["binding"]["sessionId"]["value"]', kind: 'changed', expected: '3', actual: '4', reason: 'outside exact temporal allowlist' }]);
  assertCompletePartition(result);
  assert.deepEqual(readFileSync(f.fixturePath), f.historicalBytes);
});
test('allowed temporal differences cannot pass matching schema-invalid states through the real TypeScript consumer', async t => {
  const invalid = states(); invalid.latest.schemaVersion = 999;
  const f = fixture(t, { EXPORT_BYTES: JSON.stringify({ provenance: 'controlled fixture', states: invalid }) }, invalid);
  assert.equal(await capture(f.root, f.script, f.env), 0);
  await assert.rejects(verify(f.root, f.env), /actual decodeCoexRuntimeState rejected/);
  const result = JSON.parse(readFileSync(join(f.directory, 'verification.json')));
  assert.equal(result.stateDifferences.length, 8);
  assert.deepEqual(result.acceptedTemporalDiffs, result.stateDifferences);
  assert.equal(result.temporalPolicy.status, 'passed');
  assert.deepEqual(result.temporalPolicy.rejectedDifferences, []);
  assert.equal(result.decoder.exportName, 'decodeCoexRuntimeState');
  assert.equal(result.decoder.status, 'failed');
  assert.equal(result.decoder.results.length, 6);
  assert.equal(result.decoder.results.find(row => row.name === 'latest').status, 'failed');
  assert.deepEqual(readFileSync(f.fixturePath), f.historicalBytes);
});
for (const shape of ['missing-crashed', 'null-crashed']) test(`malformed ${shape} retains a complete rejected classification`, async t => {
  const current = states();
  if (shape === 'missing-crashed') delete current.crashed; else current.crashed = null;
  const f = fixture(t, { EXPORT_BYTES: JSON.stringify({ provenance: 'controlled fixture', states: current }) });
  assert.equal(await capture(f.root, f.script, f.env), 0);
  await assert.rejects(verify(f.root, f.env));
  const result = JSON.parse(readFileSync(join(f.directory, 'verification.json')));
  assert.equal(result.status, 'failed');
  assert.equal(result.temporalPolicy.status, 'failed');
  assert.equal(result.stateDifferences.length, 9);
  assert.deepEqual(result.acceptedTemporalDiffs, []);
  assert.equal(result.temporalPolicy.rejectedDifferences.length, 9);
  assertCompletePartition(result);
  assert.deepEqual(JSON.parse(readFileSync(join(f.directory, 'state-diff.json'))), result.stateDifferences);
  assert.deepEqual(JSON.parse(readFileSync(join(f.directory, 'accepted-temporal-diff.json'))), []);
  assert.deepEqual(readFileSync(f.fixturePath), f.historicalBytes);
});
test('missing provenance still preserves the full temporal partition without making the export pass', async t => {
  const f = fixture(t, { EXPORT_BYTES: JSON.stringify({ provenance: '', states: states() }) });
  assert.equal(await capture(f.root, f.script, f.env), 0);
  await assert.rejects(verify(f.root, f.env), /missing native export provenance/);
  const result = JSON.parse(readFileSync(join(f.directory, 'verification.json')));
  assert.equal(result.status, 'failed');
  assert.equal(result.temporalPolicy.status, 'passed');
  assert.equal(result.stateDifferences.length, 8);
  assertCompletePartition(result);
});
test('the static custom shell routes each platform to one original command and preserves PowerShell native exit', () => {
  for (const platform of ['Linux', 'macOS']) assert.deepEqual(workspaceInvocation(platform, '/tmp/command', `${TEST_COMMAND}\n`),
    { command: 'bash', args: ['--noprofile', '--norc', '-e', '-o', 'pipefail', '/tmp/command'] });
  const windows = workspaceInvocation('Windows', 'C:\\Temp\\command', `${TEST_COMMAND}\n`);
  assert.equal(windows.command, 'pwsh');
  assert.equal(windows.args[0], '-command');
  assert.equal(windows.args[1].split(TEST_COMMAND).length - 1, 1);
  assert.ok(windows.args[1].startsWith("$ErrorActionPreference = 'stop'\n"));
  assert.ok(windows.args[1].endsWith('if ((Test-Path -LiteralPath variable:\\LASTEXITCODE)) { exit $LASTEXITCODE }'));
  for (const platform of ['Linux', 'macOS', 'Windows']) assert.throws(() => workspaceInvocation(platform, '/tmp/command', 'cargo test filter'));
  assert.throws(() => workspaceInvocation('other', '/tmp/command', TEST_COMMAND));
});
for (const exitCode of [0, 7]) test(`non-Linux dispatch executes once, suppresses export env, and preserves exit ${exitCode}`, async t => {
  const f = fixture(t, { NO_EXPORT: '1', FAKE_EXIT: String(exitCode) });
  const env = { ...f.env, RUNNER_OS: 'macOS', POLARIS_COEX_RUNTIME_FIXTURE: '/must-not-export' };
  assert.equal(await capture(f.root, f.script, env), exitCode);
  const calls = readFileSync(f.env.FAKE_INVOCATIONS, 'utf8').trim().split('\n').map(line => JSON.parse(line));
  assert.deepEqual(calls, [{ args: ['test', '--workspace', '--no-fail-fast'], exportPath: null }]);
  assert.equal(existsSync(join(f.directory, 'cargo-test-execution.json')), false);
  assert.equal(existsSync(join(f.directory, 'coex-runtime-current.json')), false);
});
test('capture refuses a pre-existing export or a filtered command without running cargo', async t => {
  const f = fixture(t);
  writeFileSync(f.script, 'cargo test -p polaris\n');
  await assert.rejects(capture(f.root, f.script, f.env), /unchanged full workspace/);
  writeFileSync(f.script, `${TEST_COMMAND}\n`);
  const before = JSON.parse(readFileSync(join(f.directory, 'before.json')));
  writeFileSync(before.exportPath, f.env.EXPORT_BYTES);
  await assert.rejects(capture(f.root, f.script, f.env), /already exists/);
  assert.equal(existsSync(join(f.directory, 'cargo-test-execution.json')), false);
});
test('workflow keeps the original Test command and makes verification and artifact upload unconditional after preparation', () => {
  const root = join(dirname(fileURLToPath(import.meta.url)), '..');
  const workflow = readFileSync(join(root, '.github/workflows/ci.yml'), 'utf8');
  // Scope these invariants to the ordinary test job, not later independent jobs.
  const testJob = workflow.split('\n  test:\n')[1]?.split(/\n {2}[a-z][a-z0-9_-]*:\n/)[0];
  assert.ok(testJob, 'ordinary test job must exist');
  const block = testJob.slice(testJob.indexOf('      - name: Test\n'), testJob.indexOf('      - name: Verify current Coex serializer evidence'));
  assert.match(block, /^        run: cargo test --workspace --no-fail-fast$/m);
  assert.match(block, /^        shell: node scripts\/check-coex-runtime-export\.mjs capture \{0\}$/m);
  assert.doesNotMatch(block, /^        shell:.*\$\{\{/m);
  assert.match(block, /COEX_EVIDENCE_DIR: \$\{\{ steps\.coex_prepare\.outputs\.directory \}\}/);
  const tail = testJob.slice(testJob.indexOf('      - name: Verify current Coex serializer evidence'));
  assert.equal((tail.match(/always\(\)/g) ?? []).length, 2);
  assert.match(tail, /run: node scripts\/check-coex-runtime-export\.mjs verify/);
  assert.match(tail, /uses: actions\/upload-artifact@v7/);
  assert.match(tail, /if-no-files-found: error/);
  assert.doesNotMatch(block + tail, /continue-on-error|\|\| true/);
});
