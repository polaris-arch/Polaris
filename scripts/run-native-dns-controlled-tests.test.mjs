import { spawn as spawnInertNode } from 'node:child_process';
import assert from 'node:assert/strict';
import { createHash, randomBytes } from 'node:crypto';
import { createServer } from 'node:net';
import test from 'node:test';
import { mkdtempSync, mkdirSync, readdirSync, rmSync, symlinkSync, unlinkSync, writeFileSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { readFileSync } from 'node:fs';
import { TESTS, SOURCE_TESTS, BOOTSTRAP_TESTS, contractGroups, runContractTests, selectExecutable, runtimeEnvironment, exactPass, runTests as runCaseTests, withNativeSession, startNativeSession, sessionPreparation, unmetPrerequisite, sessionTeardown, confirmedCleanup, preserveProductionGraph, selectNativeExecutable, nativeCaseRecord, exactNativePass, bindNativeRuntime, nativeRuntimeSummary } from './run-native-dns-controlled-tests.mjs';

// Inert synthetic records below declare every observation explicitly. Real
// pipe regressions later exercise startNativeSession with actual Node children.
function fixtureSessionEnd(processId, status, stdout, stderr, action) {
  return { processId, status, signal: null, exitObserved: true, exitStatus: status, exitSignal: null,
    closeObserved: true, processError: null, captureError: null, timedOut: false, interrupted: false,
    control: { action, payloadBytes: action === 'teardown' ? 9 : 0, attempted: true, finished: true, closed: true, error: null },
    stdout: Buffer.from(stdout), stderr: Buffer.from(stderr),
    outputState: { stdout: { eof: true, error: null }, stderr: { eof: true, error: null } } };
}

// Inert orchestration fixture only. It opens no native endpoint and cannot issue
// a production Check/RunMain receipt; actual case assertions use their own fixtures.
async function fixtureSession(selected, env, directory) {
  const compiled = JSON.parse(readFileSync(env.POLARIS_CORE_IDENTITY_FILE));
  const record = { schema: 'polaris-native-session-v1', state: 'ready', code: 'ready', detail: null,
    processId: 80, owner: { schema: 'polaris-native-session-owner-v1', sessionId: '00000000-0000-4000-8000-000000000001',
      compiledAppBuildId: env.GITHUB_SHA, caller: env.CORE_PLATFORM === 'win' ? 'sS-1-5-21-1000' : 'u1000',
      platform: env.CORE_PLATFORM, coreId: compiled.id, providerFingerprint: compiled.core.providerFingerprint, coreFiles: compiled.core.files,
      helperSha256: compiled.core.helperSha256 ?? null, cleanerSha256: compiled.core.cleanerSha256 ?? null,
      requiredCases: [...TESTS] }, configRoot: join(directory, 'inert-private-session'),
    provider: 'compiled-platform-production-route', registrationScope: 'enforced-by-production-start', executionAuthorized: false, slots: 1,
    maxRetainedBytes: 32 * 1024 * 1024, retainedBytesBefore: 0, required: true, publicationEligible: false };
  return { kind: 'preparation', record, processId: 80, async finish() {
    const teardown = { schema: 'polaris-native-session-teardown-v1', sessionId: record.owner.sessionId,
      processId: 80, startedCases: 6, caseDrainCheckpointsMatched: true, providerIdle: true,
      state: 'retained', directoryState: 'retained', retainedBytes: 2048, maxRetainedBytes: record.maxRetainedBytes,
      retentionReason: 'inert retained fixture', evidenceKind: 'data-coordination-only', registrationChanged: false, providerInstalled: false, publicationEligible: false };
    const stdout = `POLARIS_NATIVE_SESSION ${JSON.stringify(record)}\nPOLARIS_NATIVE_SESSION_TEARDOWN ${JSON.stringify(teardown)}\n`;
    writeFileSync(join(directory, 'native-session.stdout.log'), stdout);
    writeFileSync(join(directory, 'native-session.stderr.log'), '');
    return fixtureSessionEnd(80, 0, stdout, '', 'teardown');
  } };
}
async function runTests(selected, env, directory, execute) {
  if (selected.carrier !== 'normal-production-library') return runCaseTests(selected, env, directory, execute);
  return withNativeSession(selected, env, directory, () => runCaseTests(selected, env, directory, execute), fixtureSession);
}

test('only this invocation’s exact package/library test artifact is selectable', () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'controlled-artifact-')));
  try {
    const directory = join(root, 'target/debug/deps');
    mkdirSync(directory, { recursive: true });
    const exe = join(directory, 'polaris_lib-test'); writeFileSync(exe, 'inert fixture');
    const target = { name: 'polaris_lib', kind: ['cdylib', 'staticlib', 'rlib'], src_path: join(root, 'src-tauri/src/lib.rs') };
    const metadata = { target_directory: join(root, 'target'), packages: [
      { id: 'current-polaris-id', name: 'polaris', manifest_path: join(root, 'src-tauri/Cargo.toml'), targets: [target] },
    ] };
    const row = { reason: 'compiler-artifact', package_id: 'current-polaris-id', profile: { test: true }, features: [], target, executable: exe };
    assert.equal(selectExecutable([row], metadata, root).executable, exe);
    for (const patch of [{ package_id: 'other' }, { profile: { test: false } }, { executable: null },
      { features: undefined }, { features: ['default', 'default'] }, { features: [null] },
      { target: { ...target, kind: ['bin'] } }, { target: { ...target, src_path: '/wrong/lib.rs' } }])
      assert.throws(() => selectExecutable([{ ...row, ...patch }], metadata, root));
    assert.throws(() => selectExecutable([], metadata, root));
    assert.throws(() => selectExecutable([row, row], metadata, root));
    const alias = join(directory, 'alias');
    symlinkSync(exe, alias, process.platform === 'win32' ? 'file' : undefined);
    assert.throws(() => selectExecutable([{ ...row, executable: alias }], metadata, root));
    assert.throws(() => selectExecutable([{ ...row, executable: join(root, 'stale') }], metadata, root));
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('Cargo library paths retain native outputs, sysroot and inherited lookup without outside build-script paths', () => {
  // Exercise each environment-key/delimiter policy with synthetic rooted paths.
  // A Windows cwd contains a drive colon, which cannot be split as a POSIX list.
  // Real host paths (including Windows drive letters) are checked separately below.
  const target = join('/', 'controlled-fixture', 'target');
  const lib = join(target, 'debug/build/native/out'), exe = join(target, 'debug/deps/test');
  const messages = [{ reason: 'build-script-executed', linked_paths: [`native=${lib}`, 'native=/outside'] }];
  for (const [platform, key, sep] of [['linux', 'LD_LIBRARY_PATH', ':'], ['darwin', 'DYLD_FALLBACK_LIBRARY_PATH', ':'], ['win32', 'PATH', ';']]) {
    const env = runtimeEnvironment(messages, target, exe, '/rust/sysroot', { [key]: '/existing', FROZEN: 'yes' }, platform);
    const paths = env[key].split(sep);
    assert.ok(paths.includes(lib)); assert.ok(paths.includes('/rust/sysroot')); assert.ok(paths.includes('/existing'));
    assert.ok(!paths.includes('/outside')); assert.equal(env.FROZEN, 'yes');
  }
});

test('the actual host preserves its native Cargo paths and inherited search entries', () => {
  const target = join(process.cwd(), 'target');
  const lib = join(target, 'debug/build/native/out'), exe = join(target, 'debug/deps/test');
  const sysroot = join(process.cwd(), 'toolchain/lib');
  const inherited = join(process.cwd(), 'existing libraries');
  const key = process.platform === 'win32' ? 'PATH' : process.platform === 'darwin' ? 'DYLD_FALLBACK_LIBRARY_PATH' : 'LD_LIBRARY_PATH';
  const oldKey = process.platform === 'win32' ? 'Path' : key;
  const env = runtimeEnvironment([{ reason: 'build-script-executed', linked_paths: [`native=${lib}`] }],
    target, exe, sysroot, { [oldKey]: inherited });
  const paths = env[key].split(process.platform === 'win32' ? ';' : ':');
  for (const expected of [lib, sysroot, inherited]) assert.ok(paths.includes(expected));
  if (oldKey !== key) assert.equal(Object.hasOwn(env, oldKey), false);
});

const capturedStreams = (stdout, stderr = '') => ({
  stdout, stderr, output: stdout + stderr,
  stdoutBytes: Buffer.byteLength(stdout), stderrBytes: Buffer.byteLength(stderr),
  stdoutSha256: createHash('sha256').update(stdout).digest('hex'),
  stderrSha256: createHash('sha256').update(stderr).digest('hex'),
});
const withStdout = (result, stdout) => ({ ...result, ...capturedStreams(stdout, result.stderr) });

test('exact one-test verdict rejects zero, ignored, failed, duplicated and interrupted results', () => {
  const passed = { status: 0, ...capturedStreams('test result: ok. 1 passed; 0 failed; 0 ignored; 10 filtered out;\n') };
  assert.equal(exactPass(passed), true);
  for (const stdout of ['', passed.stdout.replace('1 passed', '0 passed'), passed.stdout.replace('0 ignored', '1 ignored'), passed.stdout.repeat(2)])
    assert.equal(exactPass(withStdout(passed, stdout)), false);
  for (const patch of [{ status: 1 }, { error: 'failed' }, { timedOut: true }, { cleanupUncertain: true }, { interrupted: true }])
    assert.equal(exactPass({ ...passed, ...patch }), false);
  assert.equal(exactPass({ ...passed, ...capturedStreams('', passed.stdout) }), false);
  assert.equal(exactPass({ status: 0, output: passed.stdout }), false);
  assert.equal(exactPass({ ...passed, ...capturedStreams(passed.stdout, passed.stdout) }), true);
  const workflow = readFileSync(new URL('../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const names = [...workflow.matchAll(/^            (runtime::[^\s]+)$/gm)].map((m) => m[1]);
  assert.deepEqual(names, TESTS);
  const runner = readFileSync(new URL('./run-native-dns-controlled-tests.mjs', import.meta.url), 'utf8');
  assert.equal((runner.match(/'--no-run'/g) ?? []).length, 1);
  const ordinary = runner.slice(runner.indexOf('\nasync function runContractSuite('), runner.indexOf('\nasync function main()'));
  const native = runner.slice(runner.indexOf('\nasync function main()'));
  assert.match(ordinary, /^      args: \['test', '--locked', '-p', packageName, '--lib', '--no-run', '--message-format=json'\] \},$/m);
  assert.match(native, /^    args: \['build', '--locked', '-p', 'polaris', '--features', 'native-production-verification', '--bin', 'native-admission-verifier', '--message-format=json'\] \},$/m);
  assert.match(native, /^  const selected = selectNativeExecutable\(messages, metadata, root\);$/m);
  assert.match(native, /^  await withNativeSession\(selected, env, directory, \(\) => runTests\(selected, env, directory\)\);$/m);
  assert.match(runner, /root: dirname\(selected.manifest\)/);
  for (const [start, end] of [['export async function runTests(', 'export function contractGroups('], ['export async function runContractTests(', 'async function runContractSuite(']]) {
    const body = runner.slice(runner.indexOf(start), runner.indexOf(end));
    assert.equal((body.match(/hash\(selected.executable\) === digest/g) ?? []).length, 2);
  }
});

const syntheticCleanup = (name, processId = 70) => ({
  schema: 'polaris-controlled-cleanup-v1', testName: name, processId, generation: 3,
  shutdownFenced: true, publicationEligible: false,
  custody: { state: 'empty-after-shutdown-fence', runRef: null, corePid: null },
});
const syntheticGraph = () => {
  const configJson = JSON.stringify({ services: [{ type: 'api', dashboard: { path: '/synthetic/dashboard' } }],
    inbounds: [{ type: 'mixed', listen: '127.0.0.1' }], outbounds: [{ type: 'direct' }] });
  return { schema: 'polaris-controlled-production-graph-v1', processId: 70, stage: 'initial-start',
    startSucceeded: true, source: 'actual-runtime-config-path', dashboardEnabled: true,
    dashboardPath: '/synthetic/dashboard', bytes: Buffer.byteLength(configJson),
    sha256: createHash('sha256').update(configJson).digest('hex'), configJson, publicationEligible: false };
};
const syntheticResult = (name, failed = false) => ({
  status: failed ? 1 : 0, processId: 70,
  ...capturedStreams(`POLARIS_CONTROLLED_CLEANUP ${JSON.stringify(syntheticCleanup(name))}\n`
    + (name === TESTS[2] ? `POLARIS_DEFAULT_GRAPH ${JSON.stringify(syntheticGraph())}\n` : '')
    + `test result: ${failed ? 'FAILED. 0 passed; 1 failed' : 'ok. 1 passed; 0 failed'}; 0 ignored;\n`),
});

test('six exact isolated cases aggregate business and graph failures only after positive owned-child cleanup', async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'controlled-processes-')));
  try {
    const executable = join(root, 'fixture'), manifest = join(root, 'Cargo.toml');
    writeFileSync(executable, 'original');
    const selected = { executable, manifest };
    for (const mode of ['pass', 'ordinary-failure', 'bad-graph', 'bad-graph-cleanup', 'bad-graph-count', 'stderr-graph', 'polluted-output',
      'zero', 'unknown-cleanup', 'cleanup-failure', 'wrong-pid', 'wrong-test', 'stderr-cleanup', 'stderr-pass', 'output-only', 'drift']) {
      const directory = join(root, mode); mkdirSync(directory);
      const calls = [];
      writeFileSync(executable, 'original');
      const run = runTests(selected, { FROZEN: 'yes' }, directory, async (command, options) => {
        calls.push(command);
        assert.equal(options.root, root); assert.equal(options.env.FROZEN, 'yes');
        const name = command.args[0], result = syntheticResult(name, mode === 'ordinary-failure');
        if (mode === 'zero') Object.assign(result, capturedStreams(result.stdout.replace('1 passed', '0 passed')));
        if (mode === 'unknown-cleanup') { result.status = 1; Object.assign(result, capturedStreams('test result: FAILED. 0 passed; 1 failed; 0 ignored;\n')); }
        if (mode === 'cleanup-failure') result.cleanupUncertain = true;
        if (mode === 'wrong-pid') result.processId = 71;
        if (mode === 'wrong-test') Object.assign(result, capturedStreams(result.stdout.replace(name, TESTS[1])));
        if (['bad-graph', 'bad-graph-cleanup', 'bad-graph-count'].includes(mode) && name === TESTS[2])
          Object.assign(result, capturedStreams(result.stdout.replace(/POLARIS_DEFAULT_GRAPH [^\n]+\n/, 'POLARIS_DEFAULT_GRAPH {broken-json}\n')));
        if (mode === 'bad-graph-cleanup' && name === TESTS[2]) result.cleanupUncertain = true;
        if (mode === 'bad-graph-count' && name === TESTS[2])
          Object.assign(result, capturedStreams(result.stdout.replace('1 passed', '0 passed')));
        if (mode === 'stderr-graph' && name === TESTS[2]) {
          const graphLine = result.stdout.match(/POLARIS_DEFAULT_GRAPH [^\n]+\n/)[0];
          Object.assign(result, capturedStreams(result.stdout.replace(graphLine, ''), graphLine));
        }
        if (mode === 'polluted-output') {
          Object.assign(result, capturedStreams(result.stdout, 'INFO diagnostics\n' + result.stdout));
          result.output = result.stdout.replace('"configJson":', 'INFO interleaved core stderr\n"configJson":') + result.stderr;
        }
        if (mode === 'stderr-cleanup') {
          const cleanupLine = result.stdout.match(/POLARIS_CONTROLLED_CLEANUP [^\n]+\n/)[0];
          Object.assign(result, capturedStreams(result.stdout.replace(cleanupLine, ''), cleanupLine));
        }
        if (mode === 'stderr-pass') {
          const summary = result.stdout.match(/test result: [^\n]+\n/)[0];
          Object.assign(result, capturedStreams(result.stdout.replace(summary, ''), summary));
        }
        if (mode === 'output-only') return { status: result.status, processId: result.processId, output: result.output };
        if (mode === 'drift') writeFileSync(executable, 'different bytes');
        return result;
      });
      if (['pass', 'polluted-output'].includes(mode)) await run;
      else if (['ordinary-failure', 'bad-graph', 'stderr-graph'].includes(mode))
        await assert.rejects(run, /Controlled business\/evidence failures after confirmed cleanup/);
      else if (['zero', 'stderr-pass', 'bad-graph-count'].includes(mode)) await assert.rejects(run, error => {
        const name = mode === 'bad-graph-count' ? TESTS[2] : TESTS[0];
        assert.equal(error.message, `Controlled invocation did not execute exactly one reviewed case: ${name}`);
        return true;
      });
      else if (mode === 'bad-graph-cleanup') await assert.rejects(run, /owned-child cleanup is unconfirmed/);
      else await assert.rejects(run);
      const expectedCalls = ['pass', 'ordinary-failure', 'bad-graph', 'stderr-graph', 'polluted-output'].includes(mode) ? 6
        : ['bad-graph-cleanup', 'bad-graph-count'].includes(mode) ? 3 : 1;
      assert.equal(calls.length, expectedCalls);
      assert.ok(calls.every((c) => c.program === executable && c.args.slice(1).join(' ') === '--exact --ignored --nocapture --test-threads=1'));
      if (calls.length === 6) assert.deepEqual(calls.map((c) => c.args[0]), TESTS);
      if (mode === 'ordinary-failure') assert.equal((readFileSync(join(directory, 'runtime-status.txt'), 'utf8').match(/cleanup confirmed/g) ?? []).length, 6);
      if (['bad-graph', 'stderr-graph'].includes(mode)) {
        const status = readFileSync(join(directory, 'runtime-status.txt'), 'utf8');
        assert.ok(status.includes(`${TESTS[2]}: failed; owned-child cleanup confirmed`));
        for (const name of TESTS.slice(3)) assert.ok(status.includes(`${name}: passed\n`));
        assert.ok(!readdirSync(directory).includes('default-production-graph.json'));
      }
      if (['bad-graph-cleanup', 'bad-graph-count'].includes(mode))
        assert.ok(!readdirSync(directory).includes('default-production-graph.json'));
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('cleanup requires one affirmative current-process/current-test original-custody record', () => {
  const name = TESTS[0], valid = syntheticResult(name, true);
  assert.equal(confirmedCleanup(valid, name), true);
  for (const patch of [{ processId: null }, { timedOut: true }, { interrupted: true }, { error: 'spawn' },
    { status: null }, capturedStreams(valid.stdout.repeat(2)), capturedStreams(''), { cleanupUncertain: true }]) {
    assert.equal(confirmedCleanup({ ...valid, ...patch }, name), false);
  }
  for (const patch of [{ shutdownFenced: false }, { generation: -1 }, { publicationEligible: true },
    { schema: 'wrong-schema' }, { testName: TESTS[1] }, { processId: 71 }, { custody: { state: 'unknown' } }]) {
    const stdout = `POLARIS_CONTROLLED_CLEANUP ${JSON.stringify({ ...syntheticCleanup(name), ...patch })}\n`;
    assert.equal(confirmedCleanup(withStdout(valid, stdout), name), false);
  }
  for (const state of ['retained-child-exit-observed', 'emergency-owned-child-reaped']) {
    const record = syntheticCleanup(name);
    record.custody = { state, runRef: 'direct-00000000-0000-0000-0000-000000000001', corePid: 81 };
    const result = withStdout(valid, `POLARIS_CONTROLLED_CLEANUP ${JSON.stringify(record)}\n`);
    assert.equal(confirmedCleanup(result, name), true);
    record.custody.runRef = 'rediscovered-by-name';
    assert.equal(confirmedCleanup(withStdout(valid, `POLARIS_CONTROLLED_CLEANUP ${JSON.stringify(record)}\n`), name), false);
  }
  assert.equal(confirmedCleanup({ ...valid, ...capturedStreams('', valid.stdout) }, name), false);
  assert.equal(confirmedCleanup({ status: valid.status, processId: valid.processId, output: valid.stdout }, name), false);
  assert.equal(confirmedCleanup({ ...valid, ...capturedStreams(valid.stdout, valid.stdout) }, name), true);
});

test('cleanup admits only complete LF or CRLF protocol lines anchored at line start', () => {
  const name = TESTS[0], valid = syntheticResult(name);
  const line = `POLARIS_CONTROLLED_CLEANUP ${JSON.stringify(syntheticCleanup(name))}`;
  for (const eol of ['\n', '\r\n']) {
    assert.equal(confirmedCleanup(withStdout(valid, `${line}${eol}`), name), true);
    assert.equal(confirmedCleanup(withStdout(valid, `ordinary diagnostic${eol}${line}${eol}unterminated diagnostic`), name), true);
  }
  for (const stdout of [line, `${line}\r`, line.slice(0, -1), `${line.slice(0, -1)}\n`,
    `prefix ${line}\n`, ` ${line}\n`, 'POLARIS_CONTROLLED_CLEANUP {invalid}\n',
    `${line}\n${line}`, `${line}\nPOLARIS_CONTROLLED_CLEANUP {invalid}\n`])
    assert.equal(confirmedCleanup(withStdout(valid, stdout), name), false);
});

test('default graph stores exact generator bytes, never reconstructs or accepts mismatched evidence', () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'controlled-graph-')));
  try {
    const result = syntheticResult(TESTS[2]);
    assert.equal(preserveProductionGraph(result, TESTS[2], root, 'a'.repeat(64)), true);
    const raw = readFileSync(join(root, 'default-production-graph.json'));
    assert.equal(raw.toString('utf8'), syntheticGraph().configJson);
    assert.equal(createHash('sha256').update(raw).digest('hex'), syntheticGraph().sha256);
    assert.throws(() => preserveProductionGraph(result, TESTS[2], root, 'a'.repeat(64)), /EEXIST/);
    assert.deepEqual(readFileSync(join(root, 'default-production-graph.json')), raw);
    const rejectedBeforeWrite = (bad, name = TESTS[2]) => {
      // Fresh destination: a late EEXIST must never feed a broken admission gate green.
      const empty = mkdtempSync(join(root, 'negative-'));
      assert.throws(() => preserveProductionGraph(bad, name, empty, 'a'.repeat(64)));
      assert.deepEqual(readdirSync(empty), []);
    };
    for (const patch of [{ processId: 71 }, { schema: 'wrong-schema' }, { bytes: 1 }, { sha256: '0'.repeat(64) },
      { dashboardEnabled: false }, { source: 'reconstructed' }, { publicationEligible: true },
      { configJson: '{}' }, { dashboardPath: '/other' }]) {
      const stdout = `POLARIS_DEFAULT_GRAPH ${JSON.stringify({ ...syntheticGraph(), ...patch })}\n`;
      rejectedBeforeWrite(withStdout(result, stdout));
    }
    rejectedBeforeWrite(withStdout(result, result.stdout.repeat(2)));
    rejectedBeforeWrite(result, TESTS[0]);
    for (const mutate of [(g) => { g.services = []; }, (g) => { g.inbounds[0].listen = '0.0.0.0'; },
      (g) => { g.inbounds[0].type = 'tun'; }]) {
      const graph = JSON.parse(syntheticGraph().configJson); mutate(graph);
      const configJson = JSON.stringify(graph), bytes = Buffer.from(configJson);
      const row = { ...syntheticGraph(), configJson, bytes: bytes.length,
        sha256: createHash('sha256').update(bytes).digest('hex') };
      rejectedBeforeWrite(withStdout(result, `POLARIS_DEFAULT_GRAPH ${JSON.stringify(row)}\n`));
    }
    assert.equal(preserveProductionGraph(withStdout(result, ''), TESTS[2], root, 'a'.repeat(64)), false);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('graph evidence requires complete protocol lines and never falls back to stderr or combined output', () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'controlled-graph-lines-')));
  try {
    const result = syntheticResult(TESTS[2]);
    const line = `POLARIS_DEFAULT_GRAPH ${JSON.stringify(syntheticGraph())}`;
    for (const eol of ['\n', '\r\n']) {
      const directory = mkdtempSync(join(root, 'valid-'));
      assert.equal(preserveProductionGraph(withStdout(result, `diagnostic${eol}${line}${eol}unterminated diagnostic`), TESTS[2], directory, 'a'.repeat(64)), true);
      assert.equal(readFileSync(join(directory, 'default-production-graph.json'), 'utf8'), syntheticGraph().configJson);
    }
    for (const stdout of [line, `${line}\r`, line.slice(0, -1), `${line.slice(0, -1)}\n`,
      'POLARIS_DEFAULT_GRAPH {invalid}\n', `${line}\n${line}`, `${line}\nPOLARIS_DEFAULT_GRAPH {invalid}\n`]) {
      const directory = mkdtempSync(join(root, 'invalid-'));
      assert.throws(() => preserveProductionGraph(withStdout(result, stdout), TESTS[2], directory, 'a'.repeat(64)));
      assert.deepEqual(readdirSync(directory), []);
    }
    for (const bad of [withStdout(result, `prefix ${line}\n`), withStdout(result, ` ${line}\n`),
      { ...result, ...capturedStreams('', `${line}\n`) },
      { status: result.status, processId: result.processId, output: `${line}\n` }]) {
      const directory = mkdtempSync(join(root, 'absent-'));
      assert.equal(preserveProductionGraph(bad, TESTS[2], directory, 'a'.repeat(64)), false);
      assert.deepEqual(readdirSync(directory), []);
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('80516-byte production graph survives stderr interleaving in the human combined log', () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'controlled-graph-streams-')));
  try {
    const graph = JSON.parse(syntheticGraph().configJson);
    graph.outbounds[0].tag = '';
    graph.outbounds[0].tag = 'x'.repeat(80_516 - Buffer.byteLength(JSON.stringify(graph)));
    const configJson = JSON.stringify(graph);
    const record = { ...syntheticGraph(), configJson, bytes: Buffer.byteLength(configJson),
      sha256: createHash('sha256').update(configJson).digest('hex') };
    assert.equal(record.bytes, 80_516);
    const stdout = `POLARIS_CONTROLLED_CLEANUP ${JSON.stringify(syntheticCleanup(TESTS[2]))}\n`
      + `POLARIS_DEFAULT_GRAPH ${JSON.stringify(record)}\n`
      + 'test result: ok. 1 passed; 0 failed; 0 ignored;\n';
    const stderr = 'INFO core diagnostic\nPOLARIS_DEFAULT_GRAPH {broken-json}\n'
      + 'POLARIS_CONTROLLED_CLEANUP {broken-json}\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;\n';
    const result = { status: 0, processId: 70, ...capturedStreams(stdout, stderr),
      output: stdout.slice(0, 32_768) + stderr + stdout.slice(32_768) };
    assert.equal(exactPass(result), true);
    assert.equal(confirmedCleanup(result, TESTS[2]), true);
    assert.equal(preserveProductionGraph(result, TESTS[2], root, 'a'.repeat(64)), true);
    const raw = readFileSync(join(root, 'default-production-graph.json'));
    assert.deepEqual(raw, Buffer.from(configJson));
    assert.equal(createHash('sha256').update(raw).digest('hex'), record.sha256);
    const receipt = JSON.parse(readFileSync(join(root, 'default-production-graph.receipt.json'), 'utf8'));
    assert.equal(receipt.bytes, record.bytes); assert.equal(receipt.sha256, record.sha256);
    assert.equal(receipt.executableSha256, 'a'.repeat(64));
  } finally { rmSync(root, { recursive: true, force: true }); }
});


test('ordinary source suites retain thirty-three unique reviewed names and exact build groups', () => {
  assert.equal(BOOTSTRAP_TESTS.length, 2); assert.equal(SOURCE_TESTS.length, 31);
  assert.equal(new Set([...BOOTSTRAP_TESTS, ...SOURCE_TESTS]).size, 33);
  assert.equal(contractGroups(BOOTSTRAP_TESTS).size, 1); assert.equal(contractGroups(SOURCE_TESTS).size, 4);
  assert.deepEqual(Object.fromEntries([...contractGroups(SOURCE_TESTS)].map(([pkg, names]) => [pkg, names.length])), {
    polaris: 16, 'polaris-store': 2, 'polaris-config-engine': 6, 'polaris-net-stack': 7,
  });
  assert.deepEqual([...contractGroups(BOOTSTRAP_TESTS)], [['polaris-helper-client', [
    'manager::tests::bootstrap_identity::bootstrap_identity_render_is_platform_specific',
    'manager::tests::bootstrap_identity::native_bootstrap_descriptor_identity',
  ]]]);
  assert.throws(() => contractGroups([...SOURCE_TESTS]));
  const wf = readFileSync(new URL('../.github/workflows/ci.yml', import.meta.url), 'utf8');
  assert.equal((wf.match(/run-native-dns-controlled-tests.mjs --bootstrap-contracts/g) ?? []).length, 1);
  assert.equal((wf.match(/run-native-dns-controlled-tests.mjs --source-contracts/g) ?? []).length, 1);
});

test('ordinary exact execution keeps per-test one-pass and immutable binary boundaries without replay', async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'ordinary-contract-')));
  try {
    const executable = join(root, 'inert-fixture'), manifest = join(root, 'Cargo.toml');
    for (const mode of ['pass', 'polluted-output', 'stderr-pass', 'output-only', 'zero', 'duplicate', 'failed', 'timeout', 'cleanup', 'drift']) {
      writeFileSync(executable, 'original'); const calls = [];
      const execute = async (command, options) => {
        calls.push(command); assert.equal(options.root, root); assert.equal(options.timeout, 20 * 60_000);
        const row = { status: 0, ...capturedStreams('test result: ok. 1 passed; 0 failed; 0 ignored;\n') };
        if (mode === 'zero') Object.assign(row, capturedStreams(row.stdout.replace('1 passed', '0 passed')));
        if (mode === 'duplicate') Object.assign(row, capturedStreams(row.stdout.repeat(2)));
        if (mode === 'failed') row.status = 1;
        if (mode === 'timeout') row.timedOut = true;
        if (mode === 'cleanup') row.cleanupUncertain = true;
        if (mode === 'polluted-output') Object.assign(row, capturedStreams(row.stdout, row.stdout));
        if (mode === 'stderr-pass') Object.assign(row, capturedStreams('', row.stdout));
        if (mode === 'output-only') return { status: row.status, output: row.output };
        if (mode === 'drift') writeFileSync(executable, 'changed');
        return row;
      };
      const action = () => runContractTests({ executable, manifest }, ['module::one', 'module::two'], {}, root, execute);
      if (['pass', 'polluted-output'].includes(mode)) { await action(); assert.equal(calls.length, 2); }
      else { await assert.rejects(action); assert.equal(calls.length, 1); }
      for (const call of calls) assert.deepEqual(call.args.slice(1), ['--exact', '--nocapture', '--test-threads=1']);
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('ordinary package artifact selection rejects another package, wrong manifest and multiple library outputs', () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'ordinary-artifact-')));
  try {
    mkdirSync(join(root, 'target/debug'), { recursive: true });
    const executable = join(root, 'target/debug/store-test'); writeFileSync(executable, 'inert');
    const target = { name: 'polaris_store', kind: ['lib'], src_path: join(root, 'crates/store/src/lib.rs') };
    const pkg = { id: 'store-id', name: 'polaris-store', manifest_path: join(root, 'crates/store/Cargo.toml'), targets: [target] };
    const metadata = { packages: [pkg], target_directory: join(root, 'target') };
    const row = { reason: 'compiler-artifact', package_id: pkg.id, profile: { test: true }, features: [], target, executable };
    assert.equal(selectExecutable([row], metadata, root, 'polaris-store').packageId, pkg.id);
    assert.throws(() => selectExecutable([], metadata, root, 'polaris-store'));
    assert.throws(() => selectExecutable([row, row], metadata, root, 'polaris-store'));
    assert.throws(() => selectExecutable([row], { ...metadata, packages: [{ ...pkg, manifest_path: join(root, 'other/Cargo.toml') }] }, root, 'polaris-store'));
    assert.throws(() => selectExecutable([row], metadata, root, 'arbitrary-package'));
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('native acceptance selects only the explicit normal-library binary artifact',()=>{
 const root=realpathSync(mkdtempSync(join(tmpdir(),'native-carrier-artifact-')));
 try {
  mkdirSync(join(root,'target/debug'),{recursive:true});const executable=join(root,'target/debug/native-admission-verifier');writeFileSync(executable,'inert artifact');
  const target={name:'native-admission-verifier',kind:['bin'],src_path:join(root,'src-tauri/src/bin/native-admission-verifier.rs')};
  const metadata={target_directory:join(root,'target'),packages:[{id:'app',name:'polaris',manifest_path:join(root,'src-tauri/Cargo.toml'),targets:[target]}]};
  const row={reason:'compiler-artifact',package_id:'app',profile:{test:false,debug_assertions:true},target,executable,features:['native-production-verification']};
  assert.equal(selectNativeExecutable([row],metadata,root).carrier,'normal-production-library');
  for(const rows of [[{...row,profile:{test:true,debug_assertions:true}}],[{...row,profile:{test:false,debug_assertions:false}}],
    [{...row,profile:{test:false}}],[{...row,features:[]}],[row,row],[{...row,target:{...target,src_path:join(root,'other.rs')}}],
    [{...row,package_id:'stale-app'}],[{...row,executable:null}],[{...row,executable:'relative-verifier'}]]){
   assert.throws(()=>selectNativeExecutable(rows,metadata,root));
  }
  const outside=join(root,'outside-verifier');writeFileSync(outside,'outside target');
  assert.throws(()=>selectNativeExecutable([{...row,executable:outside}],metadata,root));
  const alias=join(root,'target/debug/verifier-alias');symlinkSync(executable,alias,process.platform==='win32'?'file':undefined);
  assert.throws(()=>selectNativeExecutable([{...row,executable:alias}],metadata,root));
  assert.throws(()=>selectNativeExecutable([row],{...metadata,packages:[{...metadata.packages[0],manifest_path:join(root,'other/Cargo.toml')}]},root));
 }finally{rmSync(root,{recursive:true,force:true});}
});
test('native result requires one exact case, actual PID, drained custody and normal carrier',()=>{
 const name=TESTS[0],record={schema:'polaris-native-production-case-v1',case:name,processId:42,result:'passed',carrier:'normal-production-library',validationCustodyDrained:true,publicationEligible:false};
 const result=(r)=>({status:0,processId:42,stdout:'POLARIS_NATIVE_CASE '+JSON.stringify(r)+'\n'});
 assert.equal(exactNativePass(result(record),name),true);
 for(const delta of [{case:TESTS[1]},{processId:43},{carrier:'libtest'},{validationCustodyDrained:false},{publicationEligible:true}])assert.equal(nativeCaseRecord(result({...record,...delta}),name),null);
 assert.equal(exactNativePass(result({...record,result:'failed'}),name),false);
 assert.equal(exactNativePass({...result(record),timedOut:true},name),false);
 assert.equal(nativeCaseRecord({...result(record),stdout:result(record).stdout.repeat(2)},name),null);
});

const syntheticNativeResult = (name, failed = false) => ({
  status: failed ? 1 : 0, processId: 70,
  ...capturedStreams(`POLARIS_CONTROLLED_CLEANUP ${JSON.stringify(syntheticCleanup(name))}\n`
    + (name === TESTS[2] ? `POLARIS_DEFAULT_GRAPH ${JSON.stringify(syntheticGraph())}\n` : '')
    + `POLARIS_NATIVE_CASE ${JSON.stringify({ schema: 'polaris-native-production-case-v1', case: name,
      processId: 70, result: failed ? 'failed' : 'passed', carrier: 'normal-production-library',
      validationCustodyDrained: true, publicationEligible: false })}\n`),
});

function writeNativeStreams(directory, name, result) {
  const id = name.split('::').at(-1), output = result.output ?? '';
  writeFileSync(join(directory, `${id}.log`), output);
  writeFileSync(join(directory, `${id}.stdout.log`), result.stdout ?? '');
  writeFileSync(join(directory, `${id}.stderr.log`), result.stderr ?? '');
  return { error: null, timedOut: false, cleanupUncertain: false, interrupted: false, ...result,
    logBytes: Buffer.byteLength(output), logSha256: createHash('sha256').update(output).digest('hex') };
}
function boundNativeFixture(root, directory) {
  const executable = join(root, 'target/debug/native-admission-verifier'), manifest = join(root, 'src-tauri/Cargo.toml');
  mkdirSync(join(root, 'target/debug'), { recursive: true });
  mkdirSync(join(root, 'scripts'), { recursive: true }); mkdirSync(join(root, '.github/workflows'), { recursive: true });
  for (const path of ['scripts/run-native-dns-controlled-tests.mjs', '.github/workflows/ci.yml'])
    writeFileSync(join(root, path), readFileSync(new URL(`../${path}`, import.meta.url)));
  writeFileSync(executable, 'original');
  const target = { name: 'native-admission-verifier', kind: ['bin'], src_path: join(root, 'src-tauri/src/bin/native-admission-verifier.rs') };
  const metadata = { target_directory: join(root, 'target'), packages: [{ id: 'app', name: 'polaris', manifest_path: manifest, targets: [target] }] };
  const artifact = { reason: 'compiler-artifact', package_id: 'app', profile: { test: false, debug_assertions: true }, target, executable,
    features: ['default', 'formal-selection', 'native-production-verification'] };
  const env = { FROZEN: 'yes', GITHUB_REPOSITORY: 'polaris-arch/Polaris', GITHUB_REPOSITORY_ID: '1347028220',
    GITHUB_SHA: 'a'.repeat(40), CORE_CANDIDATE: 'a'.repeat(40), GITHUB_RUN_ID: '123', GITHUB_RUN_ATTEMPT: '1',
    GITHUB_REF: 'refs/heads/fixture', GITHUB_WORKFLOW_REF: 'polaris-arch/Polaris/.github/workflows/ci.yml@refs/heads/fixture',
    CORE_PLATFORM: 'win', POLARIS_NATIVE_DNS_FORMAL: '1', POLARIS_CORE_IDENTITY_FILE: join(root, 'compiled-core.json'), RUNTIME_OUTCOME: 'success' };
  writeFileSync(env.POLARIS_CORE_IDENTITY_FILE, JSON.stringify({ schema: 'polaris-core-manifest-v1', id: 'f'.repeat(64),
    core: { platform: 'win', files: [{ name: 'inert fixture only' }] } }));
  writeFileSync(join(directory, 'context.json'), JSON.stringify({ context: { repository: env.GITHUB_REPOSITORY,
    repositoryId: env.GITHUB_REPOSITORY_ID, candidate: env.GITHUB_SHA, tree: 'b'.repeat(40), runId: env.GITHUB_RUN_ID,
    attempt: env.GITHUB_RUN_ATTEMPT, workflowRef: env.GITHUB_WORKFLOW_REF, ref: env.GITHUB_REF, contractSha256: 'c'.repeat(64) },
    platform: 'win', selection: 'formal-default', kernelSourceCommit: 'd'.repeat(40), kernelSourceTree: 'e'.repeat(40) }));
  writeFileSync(join(directory, 'controlled-cargo-metadata.json'), JSON.stringify(metadata));
  writeFileSync(join(directory, 'controlled-build.stdout.log'), JSON.stringify(artifact) + '\n');
  const selected = selectNativeExecutable([artifact], metadata, root);
  selected.sha256 = createHash('sha256').update('original').digest('hex');
  writeFileSync(join(directory, 'controlled-executable.json'), JSON.stringify({ ...selected, preparationCalls: 1 }));
  const git = (command, args) => {
    assert.equal(command, 'git');
    if (args[0] === 'rev-parse') return args[1] === 'HEAD' ? env.GITHUB_SHA : 'b'.repeat(40);
    if (args[0] === 'status') return '';
    assert.equal(args[0], 'show'); return readFileSync(join(root, args[1].slice(41)), 'utf8');
  };
  const binding = bindNativeRuntime(selected, root, env, directory, git);
  return { selected, env, git, binding, summary: (patch = {}) => nativeRuntimeSummary(directory, { ...env, ...patch }, root, git) };
}

test('normal-library orchestration enforces exact authority and cleanup before six-case business aggregation', async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'native-processes-')));
  try {
    const executable = join(root, 'target/debug/native-admission-verifier');
    const aggregateModes = ['pass', 'business-failure', 'status-failure', 'record-failure', 'bad-graph', 'missing-graph', 'stderr-graph', 'polluted-output'];
    const recordMutations = {
      'wrong-case': { case: TESTS[1] }, 'wrong-native-pid': { processId: 71 },
      'wrong-carrier': { carrier: 'libtest' }, 'undrained': { validationCustodyDrained: false },
      'publishable': { publicationEligible: true }, 'unknown-result': { result: 'unknown' },
    };
    for (const mode of [...aggregateModes, ...Object.keys(recordMutations), 'missing-case', 'duplicate-case', 'malformed-case',
      'stderr-case', 'libtest-only', 'bad-graph-count', 'missing-cleanup', 'wrong-process', 'wrong-cleanup-test', 'stderr-cleanup',
      'cleanup-uncertain', 'timeout', 'interrupted', 'error', 'output-only', 'drift', 'drift-before']) {
      const directory = join(root, mode); mkdirSync(directory);
      const { selected, env } = boundNativeFixture(root, directory);
      writeFileSync(executable, mode === 'drift-before' ? 'changed' : 'original');
      const calls = [];
      const run = runTests(selected, env, directory, async (command, options) => {
        calls.push(command);
        assert.equal(options.root, join(root, 'src-tauri')); assert.equal(options.env.FROZEN, 'yes');
        assert.equal(options.timeout, 20 * 60_000);
        const name = command.args[0], result = syntheticNativeResult(name, mode === 'business-failure');
        const changeNative = patch => {
          const record = JSON.parse(result.stdout.match(/^POLARIS_NATIVE_CASE ([^\n]+)$/m)[1]);
          Object.assign(result, withStdout(result, result.stdout.replace(/^POLARIS_NATIVE_CASE [^\n]+\n/m,
            `POLARIS_NATIVE_CASE ${JSON.stringify({ ...record, ...patch })}\n`)));
        };
        if (recordMutations[mode]) changeNative(recordMutations[mode]);
        if (mode === 'status-failure') result.status = 1;
        if (mode === 'record-failure') changeNative({ result: 'failed' });
        if (mode === 'missing-case' || (mode === 'bad-graph-count' && name === TESTS[2]))
          Object.assign(result, withStdout(result, result.stdout.replace(/^POLARIS_NATIVE_CASE [^\n]+\n/m, '')));
        if (mode === 'duplicate-case') Object.assign(result, withStdout(result,
          result.stdout + result.stdout.match(/^POLARIS_NATIVE_CASE [^\n]+\n/m)[0]));
        if (mode === 'malformed-case') Object.assign(result, withStdout(result,
          result.stdout.replace(/^POLARIS_NATIVE_CASE [^\n]+\n/m, 'POLARIS_NATIVE_CASE {broken-json}\n')));
        if (mode === 'libtest-only') Object.assign(result, withStdout(result,
          result.stdout.replace(/^POLARIS_NATIVE_CASE [^\n]+\n/m, 'test result: ok. 1 passed; 0 failed; 0 ignored;\n')));
        if (mode === 'stderr-case') {
          const line = result.stdout.match(/^POLARIS_NATIVE_CASE [^\n]+\n/m)[0];
          Object.assign(result, capturedStreams(result.stdout.replace(line, ''), line));
        }
        if (['bad-graph', 'missing-graph', 'stderr-graph', 'bad-graph-count'].includes(mode) && name === TESTS[2]) {
          const line = result.stdout.match(/^POLARIS_DEFAULT_GRAPH [^\n]+\n/m)[0];
          Object.assign(result, capturedStreams(result.stdout.replace(line,
            mode === 'bad-graph' || mode === 'bad-graph-count' ? 'POLARIS_DEFAULT_GRAPH {broken-json}\n' : ''),
          mode === 'stderr-graph' ? line : ''));
        }
        if (mode === 'missing-cleanup') Object.assign(result, withStdout(result,
          result.stdout.replace(/^POLARIS_CONTROLLED_CLEANUP [^\n]+\n/m, '')));
        if (mode === 'wrong-process') result.processId = 71;
        if (mode === 'wrong-cleanup-test') Object.assign(result, withStdout(result,
          result.stdout.replace(`"testName":"${name}"`, `"testName":"${TESTS[1]}"`)));
        if (mode === 'stderr-cleanup') {
          const line = result.stdout.match(/^POLARIS_CONTROLLED_CLEANUP [^\n]+\n/m)[0];
          Object.assign(result, capturedStreams(result.stdout.replace(line, ''), line));
        }
        if (mode === 'cleanup-uncertain') result.cleanupUncertain = true;
        if (mode === 'timeout') result.timedOut = true;
        if (mode === 'interrupted') result.interrupted = true;
        if (mode === 'error') result.error = 'failed to execute';
        if (mode === 'polluted-output') {
          Object.assign(result, capturedStreams(result.stdout, result.stdout));
          result.output = 'untrusted interleaved diagnostics\n' + result.stdout + result.stderr;
        }
        if (mode === 'output-only') return writeNativeStreams(directory, name, { status: result.status, processId: result.processId, output: result.output });
        if (mode === 'drift') writeFileSync(executable, 'changed');
        return writeNativeStreams(directory, name, result);
      });
      if (['pass', 'polluted-output'].includes(mode)) await run;
      else if (aggregateModes.includes(mode)) await assert.rejects(run, /Controlled business\/evidence failures after confirmed cleanup/);
      else if (Object.hasOwn(recordMutations, mode) || ['missing-case', 'duplicate-case', 'malformed-case', 'stderr-case', 'libtest-only', 'bad-graph-count'].includes(mode))
        await assert.rejects(run, error => {
          assert.equal(error.message, `Controlled invocation did not execute exactly one reviewed case: ${mode === 'bad-graph-count' ? TESTS[2] : TESTS[0]}`);
          return true;
        });
      else await assert.rejects(run);
      const expectedCalls = aggregateModes.includes(mode) ? 6 : mode === 'bad-graph-count' ? 3 : mode === 'drift-before' ? 0 : 1;
      assert.equal(calls.length, expectedCalls, mode);
      assert.ok(calls.every((command, at) => command.program === executable && command.args.length === 1 && command.args[0] === TESTS[at]), mode);
      if (calls.length === 6) {
        assert.deepEqual(calls.map(command => command.args[0]), TESTS);
        const status = readFileSync(join(directory, 'runtime-status.txt'), 'utf8');
        assert.equal((status.match(/: started\n/g) ?? []).length, 6);
        assert.equal((status.match(/: (?:passed|failed; owned-child cleanup confirmed[^\n]*)\n/g) ?? []).length, 6);
        if (['business-failure', 'status-failure', 'record-failure'].includes(mode))
          assert.equal((status.match(/failed; owned-child cleanup confirmed/g) ?? []).length, 6);
        if (['bad-graph', 'missing-graph', 'stderr-graph'].includes(mode)) {
          assert.ok(status.includes(`${TESTS[2]}: failed; owned-child cleanup confirmed`));
          for (const name of TESTS.slice(3)) assert.ok(status.includes(`${name}: passed\n`));
        }
      }
      if (['bad-graph', 'missing-graph', 'stderr-graph', 'bad-graph-count'].includes(mode))
        assert.ok(!readdirSync(directory).includes('default-production-graph.json'));
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

const receiptFile = (fixture) => join(fixture.directory, 'native-runtime-results.json');
const readReceipt = (fixture) => JSON.parse(readFileSync(receiptFile(fixture), 'utf8'));
const writeReceipt = (fixture, value) => writeFileSync(receiptFile(fixture), JSON.stringify(value));
async function withNativeReceipt(mode, check) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'native-summary-'))), directory = join(root, 'evidence');
  mkdirSync(directory);
  try {
    const fixture = { root, directory, ...boundNativeFixture(root, directory), calls: [] };
    const operation = runTests(fixture.selected, fixture.env, directory, async (command) => {
      const name = command.args[0]; fixture.calls.push(name);
      let result = syntheticNativeResult(name, mode === 'business-failure');
      result = { ...result, ...capturedStreams(result.stdout, 'benign native diagnostic\n') };
      if (mode === 'stack-overflow') result = { status: 1, processId: 70, ...capturedStreams('', "thread 'main' has overflowed its stack\n") };
      if (mode === 'missing-graph' && name === TESTS[2])
        result = withStdout(result, result.stdout.replace(/^POLARIS_DEFAULT_GRAPH [^\n]+\n/m, ''));
      if (mode === 'malformed-graph' && name === TESTS[2])
        result = withStdout(result, result.stdout.replace(/^POLARIS_DEFAULT_GRAPH [^\n]+\n/m, 'POLARIS_DEFAULT_GRAPH {broken}\n'));
      return writeNativeStreams(directory, name, result);
    });
    if (mode === 'pass') await operation;
    else await assert.rejects(operation);
    await check(fixture);
  } finally { rmSync(root, { recursive: true, force: true }); }
}
function replaceCaseStream(fixture, index, channel, bytes) {
  bytes = Buffer.from(bytes);
  const receipt = readReceipt(fixture), stream = receipt.cases[index].streams[channel];
  writeFileSync(join(fixture.directory, stream.file), bytes);
  Object.assign(stream, { bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
  writeReceipt(fixture, receipt);
}

test('native summary R1 accepts six bound normal cases with real cleanup and nonempty diagnostic stderr', async () => {
  await withNativeReceipt('pass', (f) => {
    const value = f.summary();
    assert.equal(value.state, 'passed'); assert.equal(value.requirementState, 'satisfied');
    assert.equal(value.nativeProduction, true); assert.equal(value.nonTest, true);
    assert.deepEqual(value.tests.map((row) => row.name), TESTS);
    assert.ok(value.tests.every((row) => row.state === 'passed' && row.cleanup === 'confirmed' && row.processId === 70));
    assert.deepEqual(f.calls, TESTS);
  });
});
test('native summary R2 never promotes five legacy libtest statuses or six libtest-only streams', async () => {
  await withNativeReceipt('pass', (f) => {
    const original = readReceipt(f);
    unlinkSync(receiptFile(f));
    writeFileSync(join(f.directory, 'runtime-status.txt'), TESTS.slice(0, 5).map((name) => `${name}: passed\n`).join(''));
    assert.equal(f.summary().requirementState, 'open'); assert.equal(f.summary().evidenceState, 'bound');
    writeReceipt(f, original);
    for (let i = 0; i < TESTS.length; i++) replaceCaseStream(f, i, 'stdout', 'test result: ok. 1 passed; 0 failed; 0 ignored;\n');
    assert.equal(f.summary().requirementState, 'open');
    assert.ok(f.summary().tests.every((row) => row.state !== 'passed'));
  });
});
test('native summary R3 requires six exact ordered names including stats, never a five-proxy oracle', async () => {
  assert.deepEqual(TESTS, [
    'runtime::proxy::tests::lifecycle::real_core_full_lifecycle',
    'runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop',
    'runtime::proxy::tests::hot_switch::real_core_hot_switch_keeps_pid',
    'runtime::proxy::tests::hot_switch::real_core_auto_failover_attests_without_applying_saved_debt',
    'runtime::proxy::tests::hot_switch::real_core_hot_switch_failure_waits_for_explicit_restart_confirmation',
    'runtime::stats::tests::real_core_tests::real_core_aggregate_relay_emits_real_frames',
  ]);
  assert.equal(TESTS.length, 6); assert.equal(TESTS[5], 'runtime::stats::tests::real_core_tests::real_core_aggregate_relay_emits_real_frames');
  await withNativeReceipt('pass', (f) => {
    const original = readReceipt(f);
    for (const mutate of [r => r.cases.pop(), r => r.cases.reverse(), r => { r.cases[5].name = TESTS[4]; },
      r => { r.cases[5].name = 'runtime::stats::unreviewed'; }]) {
      const changed = structuredClone(original); mutate(changed); writeReceipt(f, changed);
      assert.equal(f.summary().evidenceState, 'invalid'); assert.equal(f.summary().requirementState, 'open');
    }
    writeReceipt(f, original); assert.equal(f.summary().state, 'passed');
  });
});
test('native summary R4 rejects foreign candidate/source/run/attempt/context and a null compiled authority', async () => {
  await withNativeReceipt('pass', (f) => {
    const original = readReceipt(f);
    for (const [key, value] of [['candidate', '9'.repeat(40)], ['sourceTree', '9'.repeat(40)], ['runId', '124'], ['attempt', '2'],
      ['workflowRef', 'foreign'], ['platform', 'linux']]) {
      const changed = structuredClone(original); changed.binding[key] = value; writeReceipt(f, changed);
      assert.equal(f.summary().requirementState, 'open', key);
    }
    for (const mutate of [r => { r.binding.sourceHashes['scripts/run-native-dns-controlled-tests.mjs'] = '9'.repeat(64); },
      r => { r.binding.compiledManifest.sha256 = '9'.repeat(64); }, r => { r.binding.files.build.sha256 = '9'.repeat(64); }]) {
      const changed = structuredClone(original); mutate(changed); writeReceipt(f, changed);
      assert.equal(f.summary().requirementState, 'open');
    }
    writeReceipt(f, original);
    for (const env of [{ CORE_CANDIDATE: '9'.repeat(40) }, { GITHUB_RUN_ATTEMPT: '2' }, { GITHUB_RUN_ID: '124' }])
      assert.equal(f.summary(env).requirementState, 'open');
    const contextPath = join(f.directory, 'context.json'), bytes = readFileSync(contextPath);
    writeFileSync(contextPath, JSON.stringify({ ...JSON.parse(bytes), kernelSourceCommit: null }));
    assert.equal(f.summary().requirementState, 'open'); writeFileSync(contextPath, bytes);
    const compiled = readFileSync(f.env.POLARIS_CORE_IDENTITY_FILE);
    writeFileSync(f.env.POLARIS_CORE_IDENTITY_FILE, 'null');
    assert.equal(f.summary().requirementState, 'open'); writeFileSync(f.env.POLARIS_CORE_IDENTITY_FILE, compiled);
    assert.equal(f.summary().state, 'passed');
  });
});
test('native summary R5 derives flags only from current normal Cargo selection and actual executable bytes', async () => {
  await withNativeReceipt('pass', (f) => {
    assert.throws(() => bindNativeRuntime({ ...f.selected }, f.root, f.env, f.directory, f.git), /not validated/);
    writeFileSync(f.selected.executable, 'different executable');
    assert.equal(f.summary().requirementState, 'open'); writeFileSync(f.selected.executable, 'original');
    const path = join(f.directory, 'controlled-executable.json'), bytes = readFileSync(path);
    for (const key of ['nativeProduction', 'nonTest', 'debugAssertions']) {
      const changed = JSON.parse(bytes); changed[key] = false; writeFileSync(path, JSON.stringify(changed));
      assert.equal(f.summary().requirementState, 'open', key);
    }
    writeFileSync(path, bytes);
    const cargoPath = join(f.directory, 'controlled-build.stdout.log'), cargoBytes = readFileSync(cargoPath);
    const cargo = JSON.parse(cargoBytes); cargo.profile.test = true; writeFileSync(cargoPath, JSON.stringify(cargo) + '\n');
    assert.equal(f.summary().requirementState, 'open'); writeFileSync(cargoPath, cargoBytes);
    assert.equal(f.summary().state, 'passed');
  });
});
test('native summary R6 reuses exact native decoder, not self-reported PID, receipt.passed or stderr claims', async () => {
  await withNativeReceipt('pass', (f) => {
    const original = readReceipt(f), stdout = readFileSync(join(f.directory, original.cases[0].streams.stdout.file), 'utf8');
    const native = stdout.match(/^POLARIS_NATIVE_CASE ([^\n]+)\n/m)[0];
    const deltas = [{ case: TESTS[1] }, { processId: 71 }, { carrier: 'libtest' }, { result: 'unknown' },
      { validationCustodyDrained: false }, { publicationEligible: true }];
    const inputs = deltas.map(patch => stdout.replace(native, 'POLARIS_NATIVE_CASE ' + JSON.stringify({ ...JSON.parse(native.slice(20)), ...patch }) + '\n'));
    inputs.push(stdout + native, stdout.replace(native, 'POLARIS_NATIVE_CASE {broken}\n'), stdout.replace(native, ''),
      stdout.replace(native, ' ' + native), stdout.replace(native, '\ufeff' + native), stdout.trimEnd());
    for (const changed of inputs) {
      writeReceipt(f, original); replaceCaseStream(f, 0, 'stdout', changed);
      const declared = readReceipt(f); declared.passed = true; declared.cases[0].passed = true; writeReceipt(f, declared);
      assert.notEqual(f.summary().tests[0].state, 'passed');
    }
    writeReceipt(f, original); replaceCaseStream(f, 0, 'stdout', stdout.replace(native, ''));
    replaceCaseStream(f, 0, 'stderr', native);
    assert.notEqual(f.summary().tests[0].state, 'passed');
    writeReceipt(f, original); replaceCaseStream(f, 0, 'stdout', stdout);
    replaceCaseStream(f, 0, 'stderr', 'benign native diagnostic\n');
    assert.equal(f.summary().state, 'passed');
  });
});
test('native summary R7 requires one original same-executor cleanup, even when native verdict says passed', async () => {
  await withNativeReceipt('pass', (f) => {
    const original = readReceipt(f), stdout = readFileSync(join(f.directory, original.cases[0].streams.stdout.file), 'utf8');
    const line = stdout.match(/^POLARIS_CONTROLLED_CLEANUP [^\n]+\n/m)[0];
    for (const changed of [stdout.replace(line, ''), stdout + line, stdout.replace('"processId":70', '"processId":71'),
      stdout.replace('"shutdownFenced":true', '"shutdownFenced":false')]) {
      writeReceipt(f, original); replaceCaseStream(f, 0, 'stdout', changed);
      assert.equal(f.summary().tests[0].state, 'cleanup-unknown');
    }
    writeReceipt(f, original); replaceCaseStream(f, 0, 'stdout', stdout.replace(line, ''));
    replaceCaseStream(f, 0, 'stderr', line);
    assert.equal(f.summary().tests[0].cleanup, 'unknown');
  });
});
test('native summary R8 saves first abort executor and five unexecuted slots before cleanup throws', async () => {
  await withNativeReceipt('stack-overflow', (f) => {
    const receipt = readReceipt(f), value = f.summary({ RUNTIME_OUTCOME: 'failure' });
    assert.equal(receipt.phase, 'aborted'); assert.equal(receipt.cases[0].executor.processId, 70);
    assert.equal(receipt.cases[0].executor.status, 1); assert.equal(receipt.cases[0].streams.stdout.bytes, 0);
    assert.equal(receipt.cases[0].nativeRecord, null); assert.equal(receipt.cases[0].cleanupConfirmed, false);
    assert.equal(value.tests[0].state, 'cleanup-unknown');
    assert.ok(value.tests.slice(1).every(row => row.state === 'unexecuted' && !row.invoked));
    assert.equal(f.calls.length, 1); assert.equal(value.requirementState, 'open');
  });
});
test('native summary R9 preserves six business failures with six affirmative cleanup records', async () => {
  await withNativeReceipt('business-failure', (f) => {
    const value = f.summary({ RUNTIME_OUTCOME: 'failure' });
    assert.equal(f.calls.length, 6); assert.equal(value.requirementState, 'open');
    assert.ok(value.tests.every(row => row.state === 'failed' && row.cleanup === 'confirmed' && row.nativeResult === 'failed'));
  });
});
test('native summary R10 retains graph failures and revalidates actual graph receipt binding', async () => {
  for (const mode of ['missing-graph', 'malformed-graph']) await withNativeReceipt(mode, (f) => {
    const value = f.summary({ RUNTIME_OUTCOME: 'failure' });
    assert.equal(f.calls.length, 6); assert.notEqual(value.tests[2].state, 'passed');
    assert.equal(value.tests[2].cleanup, 'confirmed'); assert.equal(value.requirementState, 'open');
  });
  await withNativeReceipt('pass', (f) => {
    const path = join(f.directory, 'default-production-graph.receipt.json'), bytes = readFileSync(path);
    writeFileSync(path, JSON.stringify({ ...JSON.parse(bytes), executableSha256: '0'.repeat(64) }));
    assert.notEqual(f.summary().tests[2].state, 'passed'); writeFileSync(path, bytes);
    assert.equal(f.summary().state, 'passed');
  });
});
test('native summary R11 verifies streams and executor flags independently of claimed positive rows', async () => {
  await withNativeReceipt('pass', (f) => {
    const original = readReceipt(f), path = join(f.directory, original.cases[0].streams.stdout.file), stdout = readFileSync(path);
    writeFileSync(path, Buffer.concat([stdout, Buffer.from('tampered')]));
    assert.notEqual(f.summary().tests[0].state, 'passed'); writeFileSync(path, stdout);
    replaceCaseStream(f, 0, 'stdout', Buffer.concat([stdout, Buffer.from([0xff])]));
    assert.notEqual(f.summary().tests[0].state, 'passed');
    writeReceipt(f, original); writeFileSync(path, stdout);
    for (const channel of ['log', 'stdout', 'stderr']) for (const mutate of [s => { s.sha256 = '0'.repeat(64); },
      s => { s.bytes++; }, s => { s.file = '../foreign.log'; }]) {
      const changed = structuredClone(original); mutate(changed.cases[0].streams[channel]); writeReceipt(f, changed);
      assert.notEqual(f.summary().tests[0].state, 'passed', channel);
    }
    for (const patch of [{ processId: 71 }, { status: 1 }, { error: 'executor failed' }, { timedOut: true },
      { cleanupUncertain: true }, { interrupted: true }, { timedOut: null }]) {
      const changed = structuredClone(original); Object.assign(changed.cases[0].executor, patch); writeReceipt(f, changed);
      assert.notEqual(f.summary().tests[0].state, 'passed', JSON.stringify(patch));
    }
    writeReceipt(f, original); assert.equal(f.summary().state, 'passed');
  });
});
test('native summary R12 is read-only and never promotes ordinary evidence into other acceptance scopes', async () => {
  await withNativeReceipt('pass', (f) => {
    const before = readFileSync(receiptFile(f)), calls = f.calls.length;
    const value = f.summary({ SYSTEM_FRESH_OUTCOME: 'success', SYSTEM_PREBUILT_OUTCOME: 'success' });
    assert.equal(value.AppAccepted, false); assert.equal(value.packagedAcceptance, false); assert.equal(value.publicationEligible, false);
    assert.equal(f.calls.length, calls); assert.deepEqual(readFileSync(receiptFile(f)), before);
    assert.equal(f.summary({ RUNTIME_OUTCOME: 'failure' }).requirementState, 'open');
    writeFileSync(receiptFile(f), '{partial'); assert.equal(f.summary().evidenceState, 'invalid');
  });
});

test('capability unmet preserves all six required rows and exact binding without invoking a case or granting cleanup', async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'native-prerequisite-'))), directory = join(root, 'evidence');
  mkdirSync(directory);
  try {
    const f = boundNativeFixture(root, directory); let calls = 0, finishes = 0;
    const prerequisite = { schema: 'polaris-native-prerequisite-v1', state: 'unmet', stage: 'capability',
      code: 'admission-unsupported-capability', detail: 'actual production capability refused', processId: 81,
      compiledAppBuildId: f.env.GITHUB_SHA, platform: f.env.CORE_PLATFORM,
      requiredCases: [...TESTS], required: true, publicationEligible: false };
    await assert.rejects(withNativeSession(f.selected, f.env, directory, () => { calls++; }, async () => ({
      kind: 'prerequisite', record: prerequisite, processId: 81, async finish() {
        finishes++; const stdout = `POLARIS_NATIVE_PREREQUISITE ${JSON.stringify(prerequisite)}\n`;
        writeFileSync(join(directory, 'native-session.stdout.log'), stdout);
        writeFileSync(join(directory, 'native-session.stderr.log'), '');
        return fixtureSessionEnd(81, 1, stdout, '', 'eof');
      },
    })), /Required native prerequisite unmet/);
    assert.equal(calls, 0); assert.equal(finishes, 1);
    const summary = f.summary(); assert.equal(summary.state, 'unmet-prerequisite'); assert.equal(summary.requirementState, 'open');
    assert.deepEqual(summary.tests.map(v => v.name), TESTS);
    assert.ok(summary.tests.every(v => v.required === true && v.invoked === false));
    assert.equal(summary.session.teardownConfirmed, false);
    for (const patch of [{ compiledAppBuildId: '0'.repeat(40) }, { platform: 'linux' },
      { requiredCases: TESTS.slice(0, 5) }, { required: false }, { state: 'passed' }, { publicationEligible: true }])
      assert.throws(() => unmetPrerequisite({ ...prerequisite, ...patch }, f.binding, 81));
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('session descriptor and retained data never replace source identity, exact CoreID, native owner drains or all six cases', async () => {
  await withNativeReceipt('pass', async f => {
    const holder = await fixtureSession(f.selected, f.env, f.directory), p = holder.record;
    assert.equal(sessionPreparation(p, f.binding, 80), p);
    for (const patch of [{ compiledAppBuildId: '0'.repeat(40) }, { platform: 'linux' }, { coreId: '0'.repeat(64) },
      { helperSha256: '0'.repeat(64) }, { coreFiles: [] }, { requiredCases: TESTS.slice(0, 5) }, { caller: 'sS-1-5-18' }])
      assert.throws(() => sessionPreparation({ ...p, owner: { ...p.owner, ...patch } }, f.binding, 80));
    const path = join(f.directory, 'native-session-results.json'), original = readFileSync(path);
    const value = JSON.parse(original);
    assert.equal(value.teardown.directoryState, 'retained'); assert.equal(f.summary().state, 'passed');
    for (const patch of [{ caseDrainCheckpointsMatched: false }, { providerIdle: false },
      { state: 'cleanup-unconfirmed' }, { retainedBytes: p.maxRetainedBytes + 1 }])
      assert.equal(sessionTeardown({ ...value.teardown, ...patch }, p, 80), false);
    for (const patch of [{ state: 'cleanup-unconfirmed' }, { teardownConfirmed: false },
      { failure: 'original owner retained' }, { executor: { ...value.executor, status: 1 } }]) {
      writeFileSync(path, JSON.stringify({ ...value, ...patch })); assert.equal(f.summary().requirementState, 'open');
    }
    writeFileSync(path, original);
    const stdout = join(f.directory, 'native-session.stdout.log'), raw = readFileSync(stdout);
    writeFileSync(stdout, raw.toString().replace('"caseDrainCheckpointsMatched":true', '"caseDrainCheckpointsMatched":false'));
    assert.equal(f.summary().requirementState, 'open'); writeFileSync(stdout, raw);
    unlinkSync(path); assert.equal(f.summary().requirementState, 'open');
    writeFileSync(path, original); assert.equal(f.summary().state, 'passed');
  });
});

test('session finally retains the business failure and a separate cleanup failure', async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'native-teardown-'))), directory = join(root, 'evidence'); mkdirSync(directory);
  try {
    const f = boundNativeFixture(root, directory);
    await assert.rejects(withNativeSession(f.selected, f.env, directory, () => { throw Error('actual case failure'); }, async (...args) => {
      const h = await fixtureSession(...args), finish = h.finish;
      return { ...h, async finish() {
        const end = await finish(); end.stdout = Buffer.from(end.stdout.toString('utf8').replace('"caseDrainCheckpointsMatched":true', '"caseDrainCheckpointsMatched":false'));
        writeFileSync(join(directory, 'native-session.stdout.log'), end.stdout); return end;
      } };
    }), /actual case failure; Session teardown unconfirmed/);
    const r = JSON.parse(readFileSync(join(directory, 'native-session-results.json')));
    assert.match(r.failure, /actual case failure/); assert.match(r.cleanupFailure, /teardown unconfirmed/);
    assert.equal(r.state, 'cleanup-unconfirmed'); assert.equal(f.summary().requirementState, 'open');
  } finally { rmSync(root, { recursive: true, force: true }); }
});


// All process tests below run the actual session supervisor and actual pipe I/O.
// The only seam launches process.execPath with inert source, never a shebang,
// product verifier, Core, provider, native endpoint or substituted finish object.
async function boundedFixtureWait(promise, label) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(Error(`Inert fixture deadline: ${label}`)), 5000);
    })]);
  } finally { clearTimeout(timer); }
}

// This loopback socket controls only two inert Node processes. It is not an
// output substitute: post-exit proof is read from the original child pipes.
async function inheritedOutputControl(controlFault) {
  const token = randomBytes(24).toString('hex'), sockets = new Set(), connections = [], events = [], errors = new Map(), writes = [];
  let holder, inheritor, releasing = false, stopping = false, childExit, exitResolve, probe, sequence = 0, connectionId = 0;
  let serverClosedResolve, errorsTruncated = false, injectedFailure = null, faultResolve;
  const faultObserved = new Promise(resolve => { faultResolve = resolve; });
  const serverClosed = new Promise(resolve => { serverClosedResolve = resolve; });
  const exited = new Promise(resolve => { exitResolve = resolve; });
  const note = (event, fields = {}) => {
    const next = sequence++;
    if (events.length < 48) events.push({ sequence: next, event, ...fields });
  };
  const fail = (error, connection = null, origin = 'server', injected = false) => {
    // Nominate only the actual Error from this invocation's exact injection site.
    // An earlier error, a second Error, or another role/phase is never expected.
    if (injected && injectedFailure === null && errors.size === 0) {
      injectedFailure = error; connection.injectedFailure = error; faultResolve(error);
    }
    if (!errors.has(error) && errors.size < 32) errors.set(error, { id: errors.size + 1,
      connectionId: connection?.id ?? null, role: connection?.role ?? null, phase: connection?.phase ?? 'server', origin });
    if (!errors.has(error)) errorsTruncated = true;
    note('control-error', { errorId: errors.get(error)?.id ?? null, connectionId: connection?.id ?? null,
      role: connection?.role ?? null, phase: connection?.phase ?? 'server', origin,
      code: String(error.code ?? error.name).slice(0, 64) });
    probe?.reject(error); stop();
    // A rejected inheritor auth must not strand an already-authenticated holder
    // waiting for emit. Abort uses the same two-frame ACK/FIN exchange.
    if (holder?.phase === 'ready') { holder.phase = 'abort-sent'; send(holder, 'abort'); }
  };
  const stop = () => {
    if (stopping) return;
    stopping = true;
    server.close(error => { if (error) fail(error, null, 'server-close'); serverClosedResolve(); });
  };
  const send = (connection, action, extra = {}, end = false) => {
    if (!connection || connection.socket.destroyed || connection.socket.writableEnded) {
      fail(Error('Inert control write unavailable'), connection, action); return;
    }
    note('control-send', { connectionId: connection.id, role: connection.role, phase: connection.phase, action });
    writes.push(new Promise(resolve => {
      const done = error => { if (error) fail(error, connection, action); resolve(); };
      try { connection.socket[end ? 'end' : 'write'](JSON.stringify({ action, ...extra }) + '\n', done); }
      catch (error) { done(error); }
    }));
  };
  const releasePeer = () => {
    if (!inheritor || inheritor.phase === 'release-sent' || inheritor.phase === 'fin' || inheritor.closed) return;
    inheritor.phase = 'release-sent'; send(inheritor, 'release');
  };
  const server = createServer({ allowHalfOpen: true }, socket => {
    let closeResolve;
    const connection = { socket, id: ++connectionId, role: null, phase: 'auth', input: '', bytes: 0,
      frames: 0, ended: false, finished: false, closed: false, forced: false,
      closedPromise: new Promise(resolve => { closeResolve = resolve; }) };
    sockets.add(connection);
    if (connection.id <= 2) connections.push(connection);
    if (connection.id > 2) { fail(Error('Inert control connection bound exceeded'), connection, 'accept'); socket.destroy(); }
    socket.on('error', error => fail(error, connection, 'socket-error', error.code === 'ECONNRESET'
      && (controlFault === 'holder-reset' && connection.role === 'holder' && connection.phase === 'emit-sent'
        || controlFault === 'release-reset' && connection.role === 'inheritor' && connection.phase === 'release-sent')));
    socket.once('finish', () => { connection.finished = true;
      note('control-finish', { connectionId: connection.id, role: connection.role, phase: connection.phase }); });
    socket.once('end', () => {
      connection.ended = true;
      note('control-end', { connectionId: connection.id, role: connection.role, phase: connection.phase });
      if (connection.input || connection.phase !== 'fin') {
        fail(Error('Inert control ended without its complete acknowledgement'), connection, 'end'); socket.destroy();
      }
    });
    socket.once('close', hadError => {
      connection.closed = true; sockets.delete(connection);
      note('control-closed', { connectionId: connection.id, role: connection.role, phase: connection.phase, hadError,
        ended: connection.ended, finished: connection.finished,
        causeErrorId: errors.get(connection.injectedFailure)?.id ?? null });
      // This close observes the already-recorded injected reset/frame rejection;
      // do not invent a second Error for the same cause. Every other error event
      // and every uncaused early close still fails independently.
      if (!connection.forced && !(injectedFailure && connection.injectedFailure === injectedFailure)
        && (hadError || connection.phase !== 'fin' || !connection.ended || !connection.finished))
        fail(Error('Inert control closed before acknowledged FIN/end/finish'), connection, 'close');
      closeResolve();
    });
    socket.setEncoding('utf8');
    socket.on('data', chunk => {
      connection.bytes += Buffer.byteLength(chunk); connection.input += chunk;
      let injected = false;
      try {
        assert.ok(connection.bytes <= 512, 'inert control total input bound');
        let end;
        while ((end = connection.input.indexOf('\n')) >= 0) {
          assert.ok(++connection.frames <= 2, 'two closed inert control frames');
          const value = JSON.parse(connection.input.slice(0, end)); connection.input = connection.input.slice(end + 1);
          if (connection.phase === 'auth') {
            injected = controlFault === 'foreign-auth' && value.role === 'inheritor'
              && Object.keys(value).sort().join(',') === 'role,token'
              && value.token === (token[0] === '0' ? '1' : '0') + token.slice(1);
            assert.ok(value.token === token, 'inert control authentication mismatch'); assert.ok(['holder', 'inheritor'].includes(value.role));
            assert.deepEqual(Object.keys(value).sort(), value.role === 'holder'
              ? ['role', 'spawnProcessId', 'token'] : ['role', 'token']);
            connection.role = value.role;
            if (value.role === 'holder') {
              assert.equal(holder, undefined); assert.ok(Number.isSafeInteger(value.spawnProcessId) && value.spawnProcessId > 0);
              holder = connection; connection.phase = 'ready';
              note('inheritor-spawn-event', { processId: value.spawnProcessId });
            } else {
              assert.equal(inheritor, undefined); inheritor = connection; connection.phase = 'held';
              note('inheritor-control-connected');
            }
            if (releasing || stopping) {
              if (connection.role === 'inheritor') releasePeer();
              else { connection.phase = 'abort-sent'; send(connection, 'abort'); }
            } else if (holder && inheritor && holder.phase === 'ready' && !errors.size) {
              holder.phase = 'emit-sent'; send(holder, 'emit');
            }
          } else {
            assert.deepEqual(Object.keys(value), ['action']);
            assert.equal(value.action, connection.role === 'holder' ? 'holder-ack' : 'release-ack');
            injected = controlFault === 'premature-ack' && connection.role === 'inheritor' && connection.phase === 'probe-sent';
            assert.equal(connection.phase, connection.role === 'holder'
              ? connection.phase === 'abort-sent' ? 'abort-sent' : 'emit-sent' : 'release-sent');
            connection.phase = 'fin';
            send(connection, connection.role === 'holder' ? 'holder-acknowledged' : 'release-acknowledged', {}, true);
          }
        }
      } catch (error) { fail(error, connection, 'frame', injected); socket.destroy(); }
    });
  });
  server.maxConnections = 2;
  server.on('error', error => fail(error));
  server.on('drop', () => fail(Error('Inert control connection dropped')));
  try {
    await boundedFixtureWait(new Promise((resolve, reject) => {
      server.once('error', reject); server.listen(0, '127.0.0.1', () => { server.removeListener('error', reject); resolve(); });
    }), 'control listen');
  } catch (error) {
    stop();
    try { await boundedFixtureWait(serverClosed, 'failed control listen close'); }
    catch (cause) { fail(cause, null, 'listen-cleanup'); }
    const causes = [error, ...errors.keys()].filter((value, index, all) => all.indexOf(value) === index);
    throw causes.length === 1 ? error : new AggregateError(causes, 'Inert control listen and cleanup failed');
  }
  return {
    configuration: { token, port: server.address().port },
    bind(child) {
      child.once('spawn', () => note('holder-spawn-event', { processId: child.pid }));
      child.once('error', error => { note('holder-process-error', { code: String(error.code ?? error.name).slice(0, 64) }); exitResolve(null); });
      child.once('exit', (status, signal) => { childExit = { status, signal }; note('holder-exit', childExit); exitResolve(childExit); });
      child.once('close', (status, signal) => note('holder-close', { status, signal }));
      for (const name of ['stdout', 'stderr']) {
        child[name].once('end', () => note('original-output-end', { stream: name }));
        child[name].on('data', bytes => {
          if (!probe) return;
          probe.bytes[name] = Buffer.concat([probe.bytes[name], bytes]);
          if (probe.bytes[name].length > 128 * 1024 + 1) { fail(Error('Inert probe output exceeded bound')); return; }
          if (!probe.seen[name] && probe.bytes[name].includes(probe.markers[name])) {
            probe.seen[name] = true; note('post-exit-original-output', { stream: name });
          }
          if (probe.seen.stdout && probe.seen.stderr) probe.resolve();
        });
      }
    },
    async provePostExitPipes(child) {
      const exit = await boundedFixtureWait(exited, 'actual holder exit');
      assert.deepEqual(exit, { status: 1, signal: null });
      if (errors.size) throw errors.keys().next().value;
      assert.ok(holder && inheritor && !inheritor.closed && !inheritor.socket.destroyed, 'authenticated inheritor remains available');
      await boundedFixtureWait(holder.closedPromise, 'acknowledged holder control close');
      if (errors.size) throw errors.keys().next().value;
      assert.equal(child.stdout.readableEnded, false); assert.equal(child.stderr.readableEnded, false);
      // The challenge exists only after actual direct-child exit. Control ACKs
      // cannot satisfy either original output witness or the 500ms snapshot.
      assert.equal(probe, undefined, 'one original post-exit challenge');
      assert.equal(inheritor.phase, 'held');
      const challenge = randomBytes(24).toString('hex');
      const originalActivity = new Promise((resolve, reject) => {
        probe = { resolve, reject, bytes: { stdout: Buffer.alloc(0), stderr: Buffer.alloc(0) }, seen: {},
          markers: Object.fromEntries(['stdout', 'stderr'].map(name => [name, Buffer.from(`INERT_INHERITED_${name} ${challenge}\n`)])) };
      });
      inheritor.phase = 'probe-sent'; send(inheritor, 'probe', { challenge });
      await boundedFixtureWait(originalActivity, 'post-exit activity on both original output pipes');
      if (errors.size) throw errors.keys().next().value;
      assert.equal(child.stdout.readableEnded, false); assert.equal(child.stderr.readableEnded, false);
    },
    release() {
      if (releasing) return;
      releasing = true; note('explicit-release'); stop(); releasePeer();
    },
    async close() {
      stop();
      try { await boundedFixtureWait(serverClosed, 'acknowledged control server close'); }
      catch (error) {
        fail(error, null, 'cleanup');
        for (const connection of sockets) {
          connection.forced = true; fail(Error('Inert control required forced close'), connection, 'cleanup');
          connection.socket.destroy();
        }
        await boundedFixtureWait(serverClosed, 'forced control server close');
      }
      await boundedFixtureWait(Promise.all(connections.map(connection => connection.closedPromise)), 'actual accepted socket close events');
      await boundedFixtureWait(Promise.all(writes), 'actual control write callbacks');
    },
    async waitForInjectedFailure() { return boundedFixtureWait(faultObserved, 'actual injected control failure'); },
    injectedFailure() { return injectedFailure; },
    failures() { return [...errors.keys()]; },
    diagnostics() { return { events, eventsTruncated: sequence > events.length, errorsTruncated,
      injectedErrorId: errors.get(injectedFailure)?.id ?? null,
      errors: [...errors].map(([error, fields]) => ({ ...fields, name: String(error.name).slice(0, 64),
        code: String(error.code ?? '').slice(0, 64),
        message: String(error.message ?? error).slice(0, 256).replace(/[a-f0-9]{48}/g, '[inert nonce]') })), holderExit: childExit ?? null, inheritorProcessExit: 'UNKNOWN',
      scope: 'Original direct-child pipe custody only; control close is not inheritor exit or native cleanup' }; },
  };
}

function inertReceiptSnapshot(directory) {
  try {
    const bytes = readFileSync(join(directory, 'native-session-results.json'));
    return { bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex'),
      truncated: bytes.length > 256 * 1024, base64: bytes.subarray(0, 256 * 1024).toString('base64') };
  } catch { return null; }
}

function preserveInertSessionFailure(directory, observed, error, cleanupErrors, inheritance, isolated = false, supervision = [], receipt = inertReceiptSnapshot(directory), requireNew = false) {
  // Two exclusive fixed slots, <=768KiB each / 1536KiB total. Keep this
  // inherited-output failure even if an earlier ordinary fixture also failed.
  // Serialize inert original output/receipt/events, never env/argv/source.
  const limit = 768 * 1024;
  const describe = value => ({ name: String(value?.name ?? 'Error').slice(0, 64),
    code: typeof value?.code === 'string' ? value.code.slice(0, 64) : null,
    message: String(value?.message ?? value).slice(0, 1024).replace(/[a-f0-9]{48}/g, '[inert challenge/token]') });
  const value = { schema: 'polaris-inert-session-failure-v1', processId: process.pid,
    testSourceSha256: createHash('sha256').update(readFileSync(new URL(import.meta.url))).digest('hex'),
    node: process.versions.node, uv: process.versions.uv,
    platform: process.platform, failure: error ? describe(error) : null, cleanupFailures: cleanupErrors.map(describe),
    supervision: supervision.map(entry => ({ label: entry.label, settled: entry.settled,
      error: entry.error ? describe(entry.error) : null })),
    inheritedOutput: inheritance?.diagnostics() ?? null, receipt,
    streams: Object.fromEntries(['stdout', 'stderr'].map(name => [name, {
      bytes: observed[name].length, sha256: createHash('sha256').update(observed[name]).digest('hex'),
      base64: observed[name].subarray(0, 128 * 1024 + 1).toString('base64'),
    }])), inheritorProcessExit: 'UNKNOWN', nativeCleanupConfirmed: false };
  const bytes = Buffer.from(JSON.stringify(value, null, 2) + '\n');
  assert.ok(bytes.length <= limit, 'bounded inert failure evidence');
  const target = isolated ? join(directory, 'inert-failure-evidence')
    : new URL('../native-dns-ci-evidence/inert-session-failures/', import.meta.url);
  mkdirSync(target, { recursive: true });
  const slot = inheritance ? 'inherited-output-failure.json' : 'first-other-failure.json';
  const destination = isolated ? join(target, slot) : new URL(slot, target);
  try { writeFileSync(destination, bytes, { mode: 0o600, flag: 'wx' }); }
  catch (cause) { if (cause.code !== 'EEXIST' || requireNew) throw cause; }
  return destination;
}

async function withRealSession(spec, exercise) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'session-pipes-'))), directory = join(root, 'evidence');
  mkdirSync(directory); mkdirSync(join(root, 'src-tauri'));
  const children = [], pending = [], observed = { stdout: Buffer.alloc(0), stderr: Buffer.alloc(0) };
  let inheritance, failure = null, exerciseCompleted = false;
  const track = (operation, label) => {
    assert.ok(['supervisor', 'assertion'].includes(label) && !pending.some(entry => entry.label === label));
    const entry = { label, settled: false, error: null };
    entry.done = operation.then(() => { entry.settled = true; }, error => { entry.settled = true; entry.error = error; });
    pending.push(entry); return operation;
  };
  try {
    assert.ok(spec.controlFault === undefined || spec.inheritedOutput
      && ['holder-reset', 'release-reset', 'foreign-auth', 'premature-ack'].includes(spec.controlFault), 'closed inert transport fault');
    if (spec.inheritedOutput) inheritance = await inheritedOutputControl(spec.controlFault);
    const f = boundNativeFixture(root, directory);
    const prepared = (await fixtureSession(f.selected, f.env, directory)).record;
    const prerequisite = { schema: 'polaris-native-prerequisite-v1', state: 'unmet', stage: 'capability',
      code: spec.code ?? 'admission-unsupported-capability', detail: 'inert refusal 雪', processId: 1,
      compiledAppBuildId: f.env.GITHUB_SHA, platform: f.env.CORE_PLATFORM,
      requiredCases: [...TESTS], required: true, publicationEligible: false };
    const config = { status: 1, kind: 'prerequisite', ...spec, prepared, prerequisite,
      inheritedControl: inheritance?.configuration };
    // This exact inert inheritor retains the two inherited original descriptors.
    // Its independent watchdog bounds even a failed/disconnected test controller.
    config.inheritorSource = spec.inheritedOutput ? String.raw`
import process from 'node:process';
import { connect } from 'node:net';
const c = ${JSON.stringify(inheritance?.configuration)};
const fault = ${JSON.stringify(spec.controlFault ?? null)};
const watchdog = setTimeout(() => process.exit(4), 15000);
const socket = connect({ host: '127.0.0.1', port: c.port, allowHalfOpen: true });
let input = '', received = 0, frames = 0, phase = 'held', failed = false;
let peerEnded = false, finished = false, acknowledged = false, pendingOutput = Promise.resolve();
const fail = () => { failed = true; socket.destroy(); };
socket.once('connect', () => {
  const auth = JSON.stringify({ role: 'inheritor', token: fault === 'foreign-auth' ? (c.token[0] === '0' ? '1' : '0') + c.token.slice(1) : c.token }) + '\n';
  // Actual writes deliberately split the handshake; TCP may coalesce them.
  socket.write(auth.slice(0, 17), error => {
    if (error) fail(); else socket.write(auth.slice(17), error => { if (error) fail(); });
  });
});
socket.on('error', fail);
socket.once('finish', () => { finished = true; });
socket.once('end', () => { peerEnded = true; if (!acknowledged || input) fail(); });
socket.once('close', hadError => {
  pendingOutput.then(() => {
    clearTimeout(watchdog);
    process.exit(!failed && !hadError && peerEnded && finished && acknowledged ? 0 : 3);
  }, () => { clearTimeout(watchdog); process.exit(3); });
});
socket.setEncoding('utf8');
for (const output of [process.stdout, process.stderr]) output.on('error', fail);
socket.on('data', chunk => {
  received += Buffer.byteLength(chunk); input += chunk;
  try {
    if (received > 512) throw Error('inert inheritor input bound');
    let end;
    while ((end = input.indexOf('\n')) >= 0) {
      if (++frames > 3) throw Error('inert inheritor frame bound');
      const value = JSON.parse(input.slice(0, end)); input = input.slice(end + 1);
      if (value.action === 'probe') {
        if (phase !== 'held' || Object.keys(value).sort().join(',') !== 'action,challenge'
          || !/^[a-f0-9]{48}$/.test(value.challenge)) throw Error('inert probe transition');
        phase = 'probed';
        pendingOutput = Promise.all(['stdout', 'stderr'].map(name => new Promise((resolve, reject) => {
          process[name].write('INERT_INHERITED_' + name + ' ' + value.challenge + '\n', error => error ? reject(error) : resolve());
        })));
        pendingOutput.catch(fail);
        if (fault === 'premature-ack') socket.write(JSON.stringify({ action: 'release-ack' }) + '\n', error => { if (error) fail(); });
      } else if (value.action === 'release') {
        if (!['held', 'probed'].includes(phase) || Object.keys(value).join(',') !== 'action') throw Error('inert release transition');
        phase = 'closing';
        if (fault === 'release-reset') { failed = true; socket.resetAndDestroy(); continue; }
        pendingOutput.then(() => {
          if (!failed) socket.end(JSON.stringify({ action: 'release-ack' }) + '\n', error => { if (error) fail(); });
        }, fail);
      } else if (value.action === 'release-acknowledged') {
        if (phase !== 'closing' || acknowledged || Object.keys(value).join(',') !== 'action') throw Error('inert release acknowledgement');
        acknowledged = true;
      } else throw Error('inert inheritor action');
    }
  } catch { fail(); }
});
` : undefined;
    const source = String.raw`
import process from 'node:process';
import { spawn } from 'node:child_process';
import { connect } from 'node:net';
const c = ${JSON.stringify(config)};
let record = c.kind === 'prerequisite' ? c.prerequisite : c.prepared;
record.processId = process.pid;
if (c.kind === 'provider-unmet') { record.state = 'unmet'; record.code = 'provider-unavailable'; }
if (c.wrongBinding) record.compiledAppBuildId = '0'.repeat(40);
const prefix = c.kind === 'prerequisite' ? 'POLARIS_NATIVE_PREREQUISITE ' : 'POLARIS_NATIVE_SESSION ';
let bytes = Buffer.from(prefix + JSON.stringify(record) + (c.partial ? '' : c.crlf ? String.fromCharCode(13, 10) : String.fromCharCode(10)));
const idle = () => setInterval(() => {}, 1000);
const refuse = () => {
  if (c.snoopInput) {
    let received = 0; process.stdin.on('data', bytes => { received += bytes.length; });
    process.stdin.on('end', () => process.stderr.write('observed-input-bytes:' + received + String.fromCharCode(10), () => process.exit(c.status)));
    process.stdin.resume(); return;
  }
  if (c.linger || c.signal) { idle(); return; }
  const end = () => process.stderr.write(c.overflow ? Buffer.alloc(128 * 1024 + 1, 120) : c.invalidUtf8 ? Buffer.from([255]) : 'late inert diagnostic\n', error => process.exit(c.inheritedOutput && error ? 3 : c.status));
  if (c.lateStderr) setTimeout(end, 40); else end();
};
if (c.kind !== 'prerequisite') {
  let input = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', chunk => { input += chunk; });
  process.stdin.on('end', () => {
    if (c.linger) { idle(); return; }
    if (input !== 'teardown\n') { process.stderr.write('wrong closed teardown input\n'); process.exitCode = 3; return; }
    const end = { schema: 'polaris-native-session-teardown-v1', sessionId: record.owner.sessionId,
      processId: process.pid, startedCases: 0, caseDrainCheckpointsMatched: !c.badDrain,
      providerIdle: true, state: 'retained-unopened', directoryState: 'retained', retainedBytes: 0,
      maxRetainedBytes: record.maxRetainedBytes, evidenceKind: 'data-coordination-only',
      registrationChanged: false, providerInstalled: false, publicationEligible: false };
    if (c.wrongSession) end.sessionId = '00000000-0000-4000-8000-000000000002';
    const text = c.missingTeardown ? '' : 'POLARIS_NATIVE_SESSION_TEARDOWN ' + JSON.stringify(end) + '\n';
    process.stdout.write(text, () => process.stderr.write('opened inert diagnostic\n', () => process.exit(c.status)));
  });
}
const emitted = () => {
  if (c.kind === 'prerequisite') refuse();
};
if (c.duplicate) bytes = Buffer.concat([bytes, bytes]);
if (c.split) {
  const at = bytes.indexOf(Buffer.from('雪')) + 1;
  process.stdout.write(bytes.subarray(0, at), () => setTimeout(() => process.stdout.write(bytes.subarray(at), emitted), 20));
} else if (c.inheritedOutput) {
  const watchdog = setTimeout(() => process.exit(3), 15000);
  const heir = spawn(process.execPath, ['--input-type=module', '-e', c.inheritorSource], {
    detached: true, windowsHide: true, stdio: ['ignore', 'inherit', 'inherit'],
  });
  heir.once('error', error => process.stderr.write('inert-inheritor-spawn-error:' + String(error.code ?? error.name).slice(0, 64) + '\n', () => process.exit(3)));
  heir.once('spawn', () => {
    const socket = connect({ host: '127.0.0.1', port: c.inheritedControl.port, allowHalfOpen: true });
    let input = '', received = 0, frames = 0, phase = 'emit', failed = false;
    let peerEnded = false, finished = false, acknowledged = false, aborted = false;
    const fail = () => { failed = true; socket.destroy(); };
    socket.once('connect', () => socket.write(JSON.stringify({ role: 'holder', token: c.inheritedControl.token,
      spawnProcessId: heir.pid }) + '\n', error => { if (error) fail(); }));
    socket.on('error', fail);
    socket.once('finish', () => { finished = true; });
    socket.once('end', () => { peerEnded = true; if (!acknowledged || input) fail(); });
    socket.once('close', hadError => {
      if (failed || hadError || !peerEnded || !finished || !acknowledged) { clearTimeout(watchdog); process.exit(3); }
      if (aborted) { clearTimeout(watchdog); process.exit(3); }
      // Only actual acknowledged TCP closure precedes the original output and
      // intentional holder exit. The inherited descriptors remain with the heir.
      // The watchdog stays armed while either original write is pending.
      for (const output of [process.stdout, process.stderr]) output.on('error', () => process.exit(3));
      process.stdout.write(bytes, error => { if (error) process.exit(3); else emitted(); });
    });
    socket.setEncoding('utf8');
    socket.on('data', chunk => {
      received += Buffer.byteLength(chunk); input += chunk;
      try {
        if (received > 64) throw Error('inert holder input bound');
        let end;
        while ((end = input.indexOf('\n')) >= 0) {
          if (++frames > 2) throw Error('inert holder frame bound');
          const value = JSON.parse(input.slice(0, end)); input = input.slice(end + 1);
          if (Object.keys(value).join(',') !== 'action') throw Error('inert holder action fields');
          if (phase === 'emit' && ['emit', 'abort'].includes(value.action)) {
            aborted = value.action === 'abort'; phase = 'closing';
            if (c.controlFault === 'holder-reset') { failed = true; socket.resetAndDestroy(); continue; }
            socket.end(JSON.stringify({ action: 'holder-ack' }) + '\n', error => { if (error) fail(); });
          } else if (phase === 'closing' && !acknowledged && value.action === 'holder-acknowledged') acknowledged = true;
          else throw Error('inert holder transition');
        }
      } catch { fail(); }
    });
  });
  heir.unref();
} else process.stdout.write(bytes, emitted);
`;
    const spawnChild = (program, argv, options) => {
      assert.equal(program, f.selected.executable); assert.deepEqual(argv, ['--session']);
      assert.deepEqual(options.stdio, ['pipe', 'pipe', 'pipe']); assert.equal(options.windowsHide, true);
      const child = spec.spawnMissing
        ? spawnInertNode(join(root, 'missing-inert-executable'), [], options)
        : spawnInertNode(process.execPath, ['--input-type=module', '-e', source], options);
      const entry = { child, closeObserved: false };
      entry.closed = new Promise(resolve => child.once('close', () => { entry.closeObserved = true; resolve(); }));
      children.push(entry);
      inheritance?.bind(child);
      for (const name of ['stdout', 'stderr']) child[name].on('data', bytes => {
        assert.ok(observed[name].length + bytes.length <= 128 * 1024 + 1, 'inert fixture output bound');
        observed[name] = Buffer.concat([observed[name], bytes]);
      });
      // Fault injection acts on actual child pipes; no fabricated child/finish
      // return values can satisfy the supervisor's close/EOF observations.
      if (spec.pipeFault) child.stdout.once('data', () => {
        const error = Object.assign(Error(`inert ${spec.pipeFault} pipe fault`), { code: 'EINERT', syscall: 'fixture' });
        child[spec.pipeFault].destroy(error);
      });
      if (spec.signal) child.stdout.once('data', () => child.kill('SIGTERM'));
      return child;
    };
    const start = (...args) => startNativeSession(...args, { spawnChild, preparationTimeoutMs: 8000,
      finishTimeoutMs: spec.linger || spec.inheritedOutput ? 500 : 8000 });
    await exercise({ ...f, root, directory, start, track, inheritance, child: () => children[0].child, raw: name => observed[name],
      receipt: () => JSON.parse(readFileSync(join(directory, 'native-session-results.json'))) });
    exerciseCompleted = true;
  } catch (error) { failure = error;
  } finally {
    const primary = failure, cleanupErrors = [];
    let evidenceSaved = null, rootRemoved = false;
    const retain = error => { if (error !== failure && !cleanupErrors.includes(error)) cleanupErrors.push(error); };
    let controllerClosed = !inheritance;
    try { inheritance?.release(); } catch (error) { retain(error); }
    for (const { child } of children) {
      try { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); }
      catch (error) { retain(error); }
    }
    for (const { closed } of children) {
      try { await boundedFixtureWait(closed, 'actual direct-child close after release'); }
      catch (error) { retain(error); }
    }
    try { await inheritance?.close(); controllerClosed = true; } catch (error) { retain(error); }
    // A failed graceful wait may have needed the controller's forced disconnect.
    for (const entry of children) if (!entry.closeObserved) {
      try { await boundedFixtureWait(entry.closed, 'direct-child close after control shutdown'); }
      catch (error) { retain(error); }
    }
    try { await boundedFixtureWait(Promise.all(pending.map(entry => entry.done)), 'original supervisor and assertion settlement'); }
    catch (error) { retain(error); }
    for (const entry of pending) if (entry.label === 'assertion' && entry.error) retain(entry.error);
    for (const error of inheritance?.failures() ?? []) retain(error);
    if (inheritance) for (const { child, closeObserved } of children) if (closeObserved) {
      try {
        assert.equal(child.stdout.readableEnded, true, 'original stdout EOF after release');
        assert.equal(child.stderr.readableEnded, true, 'original stderr EOF after release');
      } catch (error) { retain(error); }
    }
    const settled = controllerClosed && children.every(entry => entry.closeObserved) && pending.every(entry => entry.settled)
      && (!inheritance || children.every(({ child }) => child.stdout.readableEnded && child.stderr.readableEnded));
    if (!settled) retain(Error('Inert original operations unconfirmed; temporary evidence retained'));
    const isolated = spec.controlFault !== undefined, receiptSnapshot = inertReceiptSnapshot(directory);
    const preserve = () => preserveInertSessionFailure(directory, observed, failure, cleanupErrors, inheritance, isolated, pending, receiptSnapshot, isolated);
    // Intentional negatives use only their bounded temporary slot. Unexpected
    // evidence is written once after deletion is attempted, with original receipt
    // bytes captured beforehand, so a removal error is not lost behind wx.
    if (isolated && (failure || cleanupErrors.length)) {
      try {
        const destination = preserve(), bytes = readFileSync(destination);
        assert.ok(bytes.length <= 768 * 1024, 'actual isolated evidence readback bound');
        const value = JSON.parse(bytes);
        assert.equal(value.schema, 'polaris-inert-session-failure-v1');
        assert.equal(value.testSourceSha256, createHash('sha256').update(readFileSync(new URL(import.meta.url))).digest('hex'));
        evidenceSaved = destination;
      } catch (error) { retain(error); }
    }
    // Do not delete a root still used by an unjoined real supervisor callback.
    if (settled) try { rmSync(root, { recursive: true, force: true }); rootRemoved = true; }
    catch (error) { retain(error); }
    if (!isolated && (failure || cleanupErrors.length)) {
      try { preserve(); } catch (error) { retain(error); }
    }
    if (cleanupErrors.length) failure = new AggregateError([failure, ...cleanupErrors].filter(Boolean), 'Inert fixture failure; distinct cleanup/evidence causes retained');
    if (failure) {
      failure.inertControlDiagnostics = inheritance?.diagnostics() ?? null;
      failure.inertFailureEvidenceIsolated = isolated;
      if (isolated) failure.inertControlOutcome = { primary, exerciseCompleted, cleanupErrors,
        injectedFailure: inheritance?.injectedFailure() ?? null, supervision: pending,
        controllerClosed, settled, evidenceSaved, rootRemoved };
    }
  }
  if (failure) throw failure;
}

for (const status of [1, 2]) test(`actual early prerequisite child exit${status} retains exact refusal and complete original observations`, async () => {
  await withRealSession({ status }, async f => {
    let calls = 0;
    await assert.rejects(withNativeSession(f.selected, f.env, f.directory, () => { calls++; }, f.start),
      /^Error: Required native prerequisite unmet: capability\/admission-unsupported-capability$/);
    const r = f.receipt(), end = r.executor;
    assert.equal(f.child().exitCode, status); assert.equal(f.child().signalCode, null);
    assert.equal(calls, 0); assert.equal(end.status, status); assert.equal(end.exitStatus, status);
    assert.equal(end.closeObserved, true); assert.equal(end.exitObserved, true); assert.equal(end.signal, null);
    assert.equal(end.control.action, 'eof'); assert.equal(end.control.payloadBytes, 0);
    assert.equal(end.control.error, null); assert.equal(end.captureError, null);
    assert.equal(r.streams.stdout.eof, true); assert.equal(r.streams.stderr.eof, true);
    assert.equal(f.child().stdout.readableEnded, true); assert.equal(f.child().stderr.readableEnded, true);
    assert.equal(r.unopenedExitConfirmed, true); assert.equal(r.teardownConfirmed, false);
    const summary = f.summary(); assert.equal(summary.state, 'unmet-prerequisite'); assert.equal(summary.requirementState, 'open');
    assert.deepEqual(summary.tests.map(row => row.name), TESTS); assert.ok(summary.tests.every(row => !row.invoked));
    for (const name of ['stdout', 'stderr']) {
      const bytes = readFileSync(join(f.directory, `native-session.${name}.log`));
      assert.deepEqual(bytes, f.raw(name)); assert.equal(r.streams[name].bytes, bytes.length);
      assert.equal(r.streams[name].sha256, createHash('sha256').update(bytes).digest('hex'));
    }
  });
});

test('actual split UTF8 prerequisite and late stderr drain before exit evidence is consumed', async () => {
  await withRealSession({ split: true, crlf: true, lateStderr: true, code: 'admission-unsupported-exact-object' }, async f => {
    await assert.rejects(withNativeSession(f.selected, f.env, f.directory, () => assert.fail('no case'), f.start),
      /^Error: Required native prerequisite unmet: capability\/admission-unsupported-exact-object$/);
    assert.match(f.raw('stdout').toString('utf8'), /inert refusal 雪/);
    assert.equal(f.raw('stderr').toString('utf8'), 'late inert diagnostic\n');
    assert.equal(f.receipt().unopenedExitConfirmed, true);
  });
});

for (const spec of [{ status: 0 }, { signal: true }, { linger: true }]) test(`actual prerequisite abnormal exit remains unconfirmed ${JSON.stringify(spec)}`, async () => {
  await withRealSession(spec, async f => {
    await assert.rejects(withNativeSession(f.selected, f.env, f.directory, () => assert.fail('no case'), f.start),
      /Required native prerequisite unmet: capability\/admission-unsupported-capability; Unopened prerequisite process exit unconfirmed/);
    const r = f.receipt(); assert.equal(r.unopenedExitConfirmed, false); assert.equal(r.teardownConfirmed, false);
    assert.equal(f.summary().requirementState, 'open');
    if (spec.status === 0) assert.equal(r.executor.status, 0);
    if (spec.signal) { assert.equal(r.executor.status, null); assert.equal(r.executor.signal, 'SIGTERM'); }
    if (spec.linger) { assert.equal(r.executor.timedOut, true); assert.equal(r.executor.closeObserved, false); assert.equal(r.streams.stdout.eof, false); }
  });
});

for (const spec of [{ duplicate: true }, { partial: true }, { wrongBinding: true }, { invalidUtf8: true }]) test(`actual prerequisite malformed or foreign stream never gains authority ${JSON.stringify(spec)}`, async () => {
  await withRealSession(spec, async f => {
    await assert.rejects(withNativeSession(f.selected, f.env, f.directory, () => assert.fail('no case'), f.start));
    const r = f.receipt(); assert.equal(r.teardownConfirmed, false); assert.equal(r.unopenedExitConfirmed, false);
    assert.equal(r.executor.closeObserved, true); assert.equal(f.summary().requirementState, 'open');
    if (spec.invalidUtf8) assert.deepEqual(readFileSync(join(f.directory, 'native-session.stderr.log')), Buffer.from([255]));
  });
});

test('actual unopened input error is separate from output capture and never becomes teardown success', async () => {
  await withRealSession({ pipeFault: 'stdin' }, async f => {
    await assert.rejects(withNativeSession(f.selected, f.env, f.directory, () => assert.fail('no case'), f.start), /Required native prerequisite unmet/);
    const r = f.receipt(); assert.equal(r.executor.control.error.code, 'EINERT');
    assert.equal(r.executor.captureError, null); assert.equal(r.executor.processError, null);
    assert.equal(r.streams.stdout.error, null); assert.equal(r.streams.stderr.error, null);
    assert.equal(r.teardownConfirmed, false); assert.equal(r.state, 'unmet');
    assert.equal(r.unopenedExitConfirmed, true); assert.equal(f.summary().state, 'unmet-prerequisite');
  });
});

for (const pipeFault of ['stdout', 'stderr']) test(`actual ${pipeFault} pipe error cannot be washed by a prerequisite`, async () => {
  await withRealSession({ pipeFault, lateStderr: true }, async f => {
    await assert.rejects(withNativeSession(f.selected, f.env, f.directory, () => assert.fail('no case'), f.start));
    const r = f.receipt(); assert.equal(r.streams[pipeFault].error.code, 'EINERT');
    assert.equal(r.streams[pipeFault].eof, false); assert.equal(r.unopenedExitConfirmed, false);
    assert.equal(f.summary().requirementState, 'open');
  });
});

for (const kind of ['ready', 'provider-unmet']) test(`actual prepared ${kind} holder still receives closed teardown and original marker`, async () => {
  await withRealSession({ kind, status: 0 }, async f => {
    let calls = 0;
    const operation = withNativeSession(f.selected, f.env, f.directory, () => { calls++; }, f.start);
    if (kind === 'provider-unmet') await assert.rejects(operation, /Required native provider prerequisite unmet: provider-unavailable/);
    else await operation;
    const r = f.receipt(); assert.equal(calls, kind === 'ready' ? 1 : 0);
    assert.equal(r.executor.control.action, 'teardown'); assert.equal(r.executor.control.payloadBytes, 9);
    assert.equal(r.teardownConfirmed, true); assert.equal(r.unopenedExitConfirmed, false);
    assert.equal(r.teardown.processId, r.processId); assert.equal(r.teardown.sessionId, r.preparation.owner.sessionId);
    assert.equal(f.summary().requirementState, 'open'); // No native case executed by this inert fixture.
  });
});

for (const spec of [{ pipeFault: 'stdin' }, { badDrain: true }, { wrongSession: true }, { missingTeardown: true }, { linger: true }]) test(`actual opened failure retains business and teardown causes ${JSON.stringify(spec)}`, async () => {
  await withRealSession({ kind: 'ready', status: 0, ...spec }, async f => {
    await assert.rejects(withNativeSession(f.selected, f.env, f.directory, () => { throw Error('original business failure'); }, f.start),
      /original business failure;/);
    const r = f.receipt(); assert.match(r.failure, /original business failure/);
    assert.equal(r.teardownConfirmed, false); assert.ok(r.cleanupFailure); assert.equal(f.summary().requirementState, 'open');
  });
});

test('consumer independently rejects altered or missing close EOF process and control observations', async () => {
  await withRealSession({}, async f => {
    await assert.rejects(withNativeSession(f.selected, f.env, f.directory, () => assert.fail('no case'), f.start), /Required native prerequisite unmet/);
    const path = join(f.directory, 'native-session-results.json'), bytes = readFileSync(path), original = JSON.parse(bytes);
    assert.equal(f.summary().state, 'unmet-prerequisite');
    for (const mutate of [
      r => { delete r.executor.closeObserved; }, r => { r.executor.exitObserved = false; },
      r => { r.executor.exitStatus = 0; }, r => { r.executor.signal = 'SIGTERM'; },
      r => { r.executor.processError = { message: 'spawn failed' }; }, r => { r.executor.timedOut = true; },
      r => { r.streams.stdout.eof = false; }, r => { delete r.streams.stderr.error; },
      r => { r.streams.stdout.error = { message: 'read failed' }; }, r => { r.streams.stdout.sha256 = '0'.repeat(64); },
      r => { r.executor.control.action = 'teardown'; r.executor.control.payloadBytes = 9; },
      r => { r.unopenedExitConfirmed = false; },
    ]) {
      const changed = structuredClone(original); mutate(changed); writeFileSync(path, JSON.stringify(changed));
      assert.equal(f.summary().requirementState, 'open'); assert.notEqual(f.summary().state, 'unmet-prerequisite');
    }
    writeFileSync(path, bytes); assert.equal(f.summary().state, 'unmet-prerequisite');
  });
});


test('actual exited child with inherited output handle is unknown until original output EOF and close', async () => {
  await withRealSession({ inheritedOutput: true }, async f => {
    const operation = f.track(withNativeSession(f.selected, f.env, f.directory, () => assert.fail('no case'), f.start), 'supervisor');
    const rejection = f.track(assert.rejects(operation, /Unopened prerequisite process exit unconfirmed/), 'assertion');
    rejection.catch(() => {}); // Retain early assertion rejection while proving the real fixture precondition.
    await f.inheritance.provePostExitPipes(f.child());
    await rejection;
    const r = f.receipt(); assert.equal(r.executor.exitObserved, true); assert.equal(r.executor.exitStatus, 1);
    assert.equal(r.executor.closeObserved, false); assert.equal(r.executor.timedOut, true);
    assert.equal(r.streams.stdout.eof, false); assert.equal(r.streams.stderr.eof, false);
    assert.equal(r.unopenedExitConfirmed, false); assert.equal(f.summary().requirementState, 'open');
    for (const name of ['stdout', 'stderr']) {
      const bytes = readFileSync(join(f.directory, `native-session.${name}.log`));
      // finish saves a bounded snapshot; original data may arrive after that snapshot.
      // The post-exit challenge is separately observed on the two live original pipes.
      assert.deepEqual(bytes, f.raw(name).subarray(0, bytes.length)); assert.equal(r.streams[name].bytes, bytes.length);
      assert.equal(r.streams[name].sha256, createHash('sha256').update(bytes).digest('hex'));
      assert.match(f.raw(name).toString('utf8'), new RegExp(`INERT_INHERITED_${name} [a-f0-9]{48}\\n`));
    }
  });
});

test('actual spawn error preserves typed process observations without invented exit or EOF', async () => {
  await withRealSession({ spawnMissing: true }, async f => {
    await assert.rejects(withNativeSession(f.selected, f.env, f.directory, () => assert.fail('no case'), f.start), /ENOENT/);
    const r = f.receipt(); assert.equal(r.executor.processError.code, 'ENOENT');
    assert.equal(r.executor.exitObserved, false); assert.equal(r.processId, null);
    assert.equal(r.teardownConfirmed, false); assert.equal(r.unopenedExitConfirmed, false);
  });
});

test('actual oversized output retains exactly the bound and cannot become complete capture', async () => {
  await withRealSession({ overflow: true }, async f => {
    await assert.rejects(withNativeSession(f.selected, f.env, f.directory, () => assert.fail('no case'), f.start));
    const r = f.receipt(), raw = readFileSync(join(f.directory, 'native-session.stderr.log'));
    assert.equal(raw.length, 128 * 1024); assert.equal(r.streams.stderr.bytes, raw.length);
    assert.match(r.executor.captureError, /exceeded its bound/);
    assert.equal(r.unopenedExitConfirmed, false); assert.equal(f.summary().requirementState, 'open');
  });
});


test('actual unopened refusal observes EOF with zero teardown bytes on its real stdin', async () => {
  await withRealSession({ snoopInput: true }, async f => {
    await assert.rejects(withNativeSession(f.selected, f.env, f.directory, () => assert.fail('no case'), f.start), /Required native prerequisite unmet/);
    assert.equal(readFileSync(join(f.directory, 'native-session.stderr.log'), 'utf8'), 'observed-input-bytes:0\n');
    assert.equal(f.receipt().unopenedExitConfirmed, true); assert.equal(f.receipt().teardownConfirmed, false);
  });
});

// Intentional transport negatives keep their bounded failure JSON inside the
// actual fixture temporary root, never in the two unexpected-suite slots.
async function expectInertControlFault(controlFault, counterFault = null) {
  assert.ok(['holder-reset', 'release-reset', 'foreign-auth', 'premature-ack'].includes(controlFault));
  assert.ok(counterFault === null || controlFault === 'release-reset'
    && ['premise-assertion', 'evidence-collision'].includes(counterFault));
  let failure, supervisorFailure;
  try {
    await withRealSession({ inheritedOutput: true, controlFault }, async f => {
      const operation = f.track(withNativeSession(f.selected, f.env, f.directory, () => assert.fail('no case'), f.start), 'supervisor');
      const expected = ['holder-reset', 'foreign-auth'].includes(controlFault)
        ? /^Session holder exited without bounded preparation; Prerequisite stream differs$/
        : controlFault === 'release-reset'
          ? /^Required native prerequisite unmet: capability\/admission-unsupported-capability; Unopened prerequisite process exit unconfirmed$/
          : /^Required native prerequisite unmet: capability\/admission-unsupported-capability(?:; Unopened prerequisite process exit unconfirmed)?$/;
      const rejection = f.track(assert.rejects(operation, error => {
        supervisorFailure = error; assert.match(error.message, expected); return true;
      }), 'assertion');
      rejection.catch(() => {}); // Observe early rejection; finally still joins this original promise.
      if (controlFault === 'release-reset') {
        // Same invocation: actual exit1, fresh challenge on BOTH original pipes,
        // and the genuine 500ms incomplete snapshot must all precede release.
        await f.inheritance.provePostExitPipes(f.child());
        await rejection;
        const receipt = f.receipt();
        assert.equal(receipt.executor.exitObserved, true); assert.equal(receipt.executor.exitStatus, 1);
        assert.equal(receipt.executor.timedOut, true); assert.equal(receipt.executor.closeObserved, false);
        assert.equal(receipt.streams.stdout.eof, false); assert.equal(receipt.streams.stderr.eof, false);
        assert.equal(receipt.unopenedExitConfirmed, false); assert.equal(f.summary().requirementState, 'open');
      } else {
        if (controlFault === 'premature-ack') {
          try { await f.inheritance.provePostExitPipes(f.child()); }
          catch (error) { if (error !== f.inheritance.injectedFailure()) throw error; }
        }
        await f.inheritance.waitForInjectedFailure();
        f.inheritance.release();
        await rejection;
        const receipt = f.receipt();
        // Early aborts do not claim the normal A2/500ms premise. They must instead
        // finish their own actual process/pipe join and phase-specific refusal.
        assert.equal(receipt.executor.exitObserved, true); assert.equal(receipt.executor.signal, null);
        assert.equal(receipt.executor.exitStatus, controlFault === 'premature-ack' ? 1 : 3);
        assert.equal(receipt.teardownConfirmed, false);
        if (controlFault !== 'premature-ack') {
          assert.equal(receipt.executor.closeObserved, true); assert.equal(receipt.executor.timedOut, false);
          assert.equal(receipt.streams.stdout.eof, true); assert.equal(receipt.streams.stderr.eof, true);
          assert.equal(receipt.prerequisite, null); assert.equal(receipt.unopenedExitConfirmed, false);
          assert.equal(f.raw('stdout').length, 0); assert.equal(f.raw('stderr').length, 0);
        }
        // A premature ACK can reach the controller before or after the genuine
        // 500ms snapshot. Actual final EOF/close/join is required below either way;
        // its snapshot is not a second claim of the normal A2 holding premise.
      }
      const receipt = f.receipt();
      assert.equal(receipt.executor.processId, f.child().pid);
      assert.equal(receipt.executor.processError, null); assert.equal(receipt.executor.captureError, null);
      assert.equal(receipt.executor.interrupted, false); assert.equal(receipt.executor.control.error, null);
      assert.equal(receipt.executor.control.action, 'eof'); assert.equal(receipt.executor.control.payloadBytes, 0);
      assert.equal(receipt.streams.stdout.error, null); assert.equal(receipt.streams.stderr.error, null);
      if (counterFault === 'premise-assertion')
        assert.equal(receipt.executor.timedOut, false, 'deliberate independent actual snapshot assertion failure');
      if (counterFault === 'evidence-collision')
        writeFileSync(join(f.directory, 'inert-failure-evidence'), 'actual independent file/directory collision', { flag: 'wx' });
      // finally must release even if an independent assertion above fails.
    });
  } catch (error) { failure = error; }
  assert.ok(failure, 'the actual transport injection must reject');
  const outcome = failure.inertControlOutcome, diagnostics = failure.inertControlDiagnostics;
  // Propagate the original full aggregate, including its exact primary/late Error
  // objects. The presence of an expected event cannot accept ANY extra cause.
  if (!outcome || outcome.primary !== null || !outcome.exerciseCompleted
    || !outcome.injectedFailure || outcome.cleanupErrors.length !== 1
    || outcome.cleanupErrors[0] !== outcome.injectedFailure
    || !outcome.controllerClosed || !outcome.settled || !outcome.rootRemoved || !outcome.evidenceSaved
    || outcome.supervision.length !== 2 || outcome.supervision.some(entry => !entry.settled)
    || outcome.supervision.find(entry => entry.label === 'supervisor')?.error !== supervisorFailure
    || !supervisorFailure || outcome.supervision.find(entry => entry.label === 'assertion')?.error !== null
    || !diagnostics || diagnostics.eventsTruncated || diagnostics.errorsTruncated || diagnostics.errors.length !== 1)
    throw failure;
  const [role, phase, origin] = {
    'holder-reset': ['holder', 'emit-sent', 'socket-error'],
    'release-reset': ['inheritor', 'release-sent', 'socket-error'],
    'foreign-auth': [null, 'auth', 'frame'],
    'premature-ack': ['inheritor', 'probe-sent', 'frame'],
  }[controlFault];
  if (!diagnostics.events.some(row => row.event === 'control-error' && row.errorId === diagnostics.injectedErrorId
      && row.role === role && row.phase === phase && row.origin === origin)
    || !diagnostics.events.some(row => row.event === 'explicit-release')
    || diagnostics.inheritorProcessExit !== 'UNKNOWN' || failure.inertFailureEvidenceIsolated !== true)
    throw failure;
}

for (const controlFault of ['holder-reset', 'release-reset', 'foreign-auth', 'premature-ack'])
  test(`inert control rejects actual transport failure ${controlFault} without granting pipe custody`, async () => {
    await expectInertControlFault(controlFault);
  });

for (const counterFault of ['premise-assertion', 'evidence-collision'])
  test(`actual expected release reset cannot accept independent ${counterFault}`, async () => {
    await assert.rejects(expectInertControlFault('release-reset', counterFault), error => {
      const outcome = error.inertControlOutcome, diagnostics = error.inertControlDiagnostics;
      assert.ok(outcome?.injectedFailure && error.errors.includes(outcome.injectedFailure));
      assert.ok(diagnostics.events.some(row => row.event === 'control-error'
        && row.errorId === diagnostics.injectedErrorId && row.role === 'inheritor'
        && row.phase === 'release-sent' && row.origin === 'socket-error'));
      assert.equal(outcome.controllerClosed, true); assert.equal(outcome.settled, true); assert.equal(outcome.rootRemoved, true);
      assert.equal(outcome.supervision.length, 2);
      for (const entry of outcome.supervision) assert.equal(entry.settled, true);
      assert.equal(outcome.supervision.find(entry => entry.label === 'assertion').error, null);
      if (counterFault === 'premise-assertion') {
        assert.equal(outcome.primary.name, 'AssertionError');
        assert.match(outcome.primary.message, /deliberate independent actual snapshot assertion failure/);
        assert.ok(error.errors.includes(outcome.primary), 'original AssertionError object retained');
        assert.equal(outcome.exerciseCompleted, false); assert.ok(outcome.evidenceSaved);
        assert.deepEqual(outcome.cleanupErrors, [outcome.injectedFailure]);
      } else {
        assert.equal(outcome.primary, null); assert.equal(outcome.exerciseCompleted, true); assert.equal(outcome.evidenceSaved, null);
        const filesystemError = outcome.cleanupErrors.find(cause => cause !== outcome.injectedFailure);
        assert.ok(filesystemError && ['EEXIST', 'ENOTDIR'].includes(filesystemError.code));
        assert.equal(outcome.cleanupErrors.length, 2);
        assert.ok(error.errors.includes(filesystemError), 'actual filesystem Error object retained');
      }
      assert.equal(diagnostics.inheritorProcessExit, 'UNKNOWN');
      assert.equal(error.inertFailureEvidenceIsolated, true);
      return true;
    });
  });

test('inert failure evidence uses actual isolated files and preserves the first bounded slot', () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'inert-evidence-')));
  const directory = join(root, 'evidence'); mkdirSync(directory);
  const observed = { stdout: Buffer.from('inert stdout\n'), stderr: Buffer.from('inert stderr\n') };
  try {
    const destination = preserveInertSessionFailure(directory, observed, Error('first inert failure'), [], null, true);
    assert.equal(destination, join(directory, 'inert-failure-evidence', 'first-other-failure.json'));
    const first = readFileSync(destination), value = JSON.parse(first);
    assert.ok(first.length <= 768 * 1024); assert.equal(value.failure.message, 'first inert failure');
    for (const name of ['stdout', 'stderr']) {
      const bytes = Buffer.from(value.streams[name].base64, 'base64');
      assert.deepEqual(bytes, observed[name]); assert.equal(value.streams[name].bytes, bytes.length);
      assert.equal(value.streams[name].sha256, createHash('sha256').update(bytes).digest('hex'));
    }
    assert.equal(value.inheritorProcessExit, 'UNKNOWN'); assert.equal(value.nativeCleanupConfirmed, false);
    preserveInertSessionFailure(directory, observed, Error('second inert failure'), [], null, true);
    assert.deepEqual(readFileSync(destination), first, 'wx must retain the actual first file');
    rmSync(join(directory, 'inert-failure-evidence'), { recursive: true });
    writeFileSync(join(directory, 'inert-failure-evidence'), 'actual file/directory collision');
    assert.throws(() => preserveInertSessionFailure(directory, observed, Error('original cause'), [], null, true));
  } finally { rmSync(root, { recursive: true, force: true }); }
});
