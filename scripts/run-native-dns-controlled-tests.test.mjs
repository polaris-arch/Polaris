import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';
import { mkdtempSync, mkdirSync, readdirSync, rmSync, symlinkSync, unlinkSync, writeFileSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { readFileSync } from 'node:fs';
import { TESTS, SOURCE_TESTS, BOOTSTRAP_TESTS, contractGroups, runContractTests, selectExecutable, runtimeEnvironment, exactPass, runTests, confirmedCleanup, preserveProductionGraph, selectNativeExecutable, nativeCaseRecord, exactNativePass, bindNativeRuntime, nativeRuntimeSummary } from './run-native-dns-controlled-tests.mjs';

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
  assert.match(native, /^  await runTests\(selected, env, directory\);$/m);
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
  bindNativeRuntime(selected, root, env, directory, git);
  return { selected, env, git, summary: (patch = {}) => nativeRuntimeSummary(directory, { ...env, ...patch }, root, git) };
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
    assert.equal(f.summary().requirementState, 'open'); assert.equal(f.summary().evidenceState, 'missing');
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
