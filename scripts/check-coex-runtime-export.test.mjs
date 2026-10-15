import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  TEST_COMMAND, EXPORT_TEST, SOURCE_PATHS, DECODER_PATHS, STATE_KEYS, canonical, sha256, stateDifferences,
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
  writeFileSync(fake, `import { appendFileSync, writeFileSync } from 'node:fs';\nappendFileSync(process.env.FAKE_INVOCATIONS, JSON.stringify({ args: process.argv.slice(2), exportPath: process.env.POLARIS_COEX_RUNTIME_FIXTURE ?? null }) + '\\n');\nif (!process.env.NO_EXPORT) writeFileSync(process.env.POLARIS_COEX_RUNTIME_FIXTURE, process.env.EXPORT_BYTES);\nif (!process.env.ZERO_TESTS) console.log(${JSON.stringify(passedLine.trim())});\nprocess.exitCode = Number(process.env.FAKE_EXIT || '0');\n`);
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
  assert.equal(result.status, 'passed');
  assert.equal(result.newSerializerExecution, true);
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
    'cargo-test-execution.json', 'coex-runtime-current.json', 'state-diff.json', 'verification.json']) assert.ok(existsSync(join(f.directory, path)), path);
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
  assert.deepEqual(result.stateDifferences, [{ path: '$["latest"]["binding"]["sessionId"]["value"]', kind: 'changed', expected: '3', actual: '4' }]);
  assert.deepEqual(readFileSync(f.fixturePath), f.historicalBytes);
});
test('canonical equality alone cannot pass states rejected by the actual TypeScript consumer', async t => {
  const invalid = states(); invalid.latest.schemaVersion = 999;
  const f = fixture(t, { EXPORT_BYTES: JSON.stringify({ provenance: 'controlled fixture', states: invalid }) }, invalid);
  assert.equal(await capture(f.root, f.script, f.env), 0);
  await assert.rejects(verify(f.root, f.env), /actual decodeCoexRuntimeState rejected/);
  const result = JSON.parse(readFileSync(join(f.directory, 'verification.json')));
  assert.deepEqual(result.stateDifferences, []);
  assert.equal(result.originalStatesCanonicalSha256, result.currentStatesCanonicalSha256);
  assert.equal(result.decoder.exportName, 'decodeCoexRuntimeState');
  assert.equal(result.decoder.status, 'failed');
  assert.equal(result.decoder.results.length, 6);
  assert.equal(result.decoder.results.find(row => row.name === 'latest').status, 'failed');
  assert.deepEqual(readFileSync(f.fixturePath), f.historicalBytes);
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
  const block = workflow.slice(workflow.indexOf('      - name: Test\n'), workflow.indexOf('      - name: Verify current Coex serializer evidence'));
  assert.match(block, /^        run: cargo test --workspace --no-fail-fast$/m);
  assert.match(block, /^        shell: node scripts\/check-coex-runtime-export\.mjs capture \{0\}$/m);
  assert.doesNotMatch(block, /^        shell:.*\$\{\{/m);
  assert.match(block, /COEX_EVIDENCE_DIR: \$\{\{ steps\.coex_prepare\.outputs\.directory \}\}/);
  const tail = workflow.slice(workflow.indexOf('      - name: Verify current Coex serializer evidence'));
  assert.equal((tail.match(/always\(\)/g) ?? []).length, 2);
  assert.match(tail, /run: node scripts\/check-coex-runtime-export\.mjs verify/);
  assert.match(tail, /uses: actions\/upload-artifact@v7/);
  assert.match(tail, /if-no-files-found: error/);
  assert.doesNotMatch(block + tail, /continue-on-error|\|\| true/);
});
