import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';
import { mkdtempSync, mkdirSync, readdirSync, rmSync, symlinkSync, writeFileSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { readFileSync } from 'node:fs';
import { TESTS, selectExecutable, runtimeEnvironment, exactPass, runTests, confirmedCleanup, preserveProductionGraph } from './run-native-dns-controlled-tests.mjs';

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
    const row = { reason: 'compiler-artifact', package_id: 'current-polaris-id', profile: { test: true }, target, executable: exe };
    assert.equal(selectExecutable([row], metadata, root).executable, exe);
    for (const patch of [{ package_id: 'other' }, { profile: { test: false } }, { executable: null },
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

test('exact one-test verdict rejects zero, ignored, failed, duplicated and interrupted results', () => {
  const passed = { status: 0, output: 'test result: ok. 1 passed; 0 failed; 0 ignored; 10 filtered out;\n' };
  assert.equal(exactPass(passed), true);
  for (const output of ['', passed.output.replace('1 passed', '0 passed'), passed.output.replace('0 ignored', '1 ignored'), passed.output.repeat(2)])
    assert.equal(exactPass({ ...passed, output }), false);
  for (const patch of [{ status: 1 }, { error: 'failed' }, { timedOut: true }, { cleanupUncertain: true }, { interrupted: true }])
    assert.equal(exactPass({ ...passed, ...patch }), false);
  const workflow = readFileSync(new URL('../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const names = [...workflow.matchAll(/^            (runtime::[^\s]+)$/gm)].map((m) => m[1]);
  assert.deepEqual(names, TESTS);
  const runner = readFileSync(new URL('./run-native-dns-controlled-tests.mjs', import.meta.url), 'utf8');
  assert.equal((runner.match(/'--no-run'/g) ?? []).length, 1);
  assert.match(runner, /root: dirname\(selected.manifest\)/);
  assert.equal((runner.match(/hash\(selected.executable\) === digest/g) ?? []).length, 2);
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
  output: `POLARIS_CONTROLLED_CLEANUP ${JSON.stringify(syntheticCleanup(name))}\n`
    + (name === TESTS[2] ? `POLARIS_DEFAULT_GRAPH ${JSON.stringify(syntheticGraph())}\n` : '')
    + `test result: ${failed ? 'FAILED. 0 passed; 1 failed' : 'ok. 1 passed; 0 failed'}; 0 ignored;\n`,
});

test('six exact isolated cases aggregate ordinary failures only after positive owned-child cleanup', async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'controlled-processes-')));
  try {
    const executable = join(root, 'fixture'), manifest = join(root, 'Cargo.toml');
    writeFileSync(executable, 'original');
    const selected = { executable, manifest };
    for (const mode of ['pass', 'ordinary-failure', 'zero', 'unknown-cleanup', 'cleanup-failure', 'wrong-pid', 'wrong-test', 'drift']) {
      const directory = join(root, mode); mkdirSync(directory);
      const calls = [];
      writeFileSync(executable, 'original');
      const run = runTests(selected, { FROZEN: 'yes' }, directory, async (command, options) => {
        calls.push(command);
        assert.equal(options.root, root); assert.equal(options.env.FROZEN, 'yes');
        const name = command.args[0], result = syntheticResult(name, mode === 'ordinary-failure');
        if (mode === 'zero') result.output = result.output.replace('1 passed', '0 passed');
        if (mode === 'unknown-cleanup') { result.status = 1; result.output = 'test result: FAILED. 0 passed; 1 failed; 0 ignored;\n'; }
        if (mode === 'cleanup-failure') result.cleanupUncertain = true;
        if (mode === 'wrong-pid') result.processId = 71;
        if (mode === 'wrong-test') result.output = result.output.replace(name, TESTS[1]);
        if (mode === 'drift') writeFileSync(executable, 'different bytes');
        return result;
      });
      if (mode === 'pass') await run; else await assert.rejects(run);
      assert.equal(calls.length, ['pass', 'ordinary-failure'].includes(mode) ? 6 : 1);
      assert.ok(calls.every((c) => c.program === executable && c.args.slice(1).join(' ') === '--exact --ignored --nocapture --test-threads=1'));
      if (calls.length === 6) assert.deepEqual(calls.map((c) => c.args[0]), TESTS);
      if (mode === 'ordinary-failure') assert.equal((readFileSync(join(directory, 'runtime-status.txt'), 'utf8').match(/cleanup confirmed/g) ?? []).length, 6);
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('cleanup requires one affirmative current-process/current-test original-custody record', () => {
  const name = TESTS[0], valid = syntheticResult(name, true);
  assert.equal(confirmedCleanup(valid, name), true);
  for (const patch of [{ processId: null }, { timedOut: true }, { interrupted: true }, { error: 'spawn' },
    { status: null }, { output: valid.output + valid.output }, { output: '' }, { cleanupUncertain: true }]) {
    assert.equal(confirmedCleanup({ ...valid, ...patch }, name), false);
  }
  for (const patch of [{ shutdownFenced: false }, { generation: -1 }, { publicationEligible: true },
    { testName: TESTS[1] }, { processId: 71 }, { custody: { state: 'unknown' } }]) {
    const output = `POLARIS_CONTROLLED_CLEANUP ${JSON.stringify({ ...syntheticCleanup(name), ...patch })}\n`;
    assert.equal(confirmedCleanup({ ...valid, output }, name), false);
  }
  for (const state of ['retained-child-exit-observed', 'emergency-owned-child-reaped']) {
    const record = syntheticCleanup(name);
    record.custody = { state, runRef: 'direct-00000000-0000-0000-0000-000000000001', corePid: 81 };
    const result = { ...valid, output: `POLARIS_CONTROLLED_CLEANUP ${JSON.stringify(record)}\n` };
    assert.equal(confirmedCleanup(result, name), true);
    record.custody.runRef = 'rediscovered-by-name';
    assert.equal(confirmedCleanup({ ...valid, output: `POLARIS_CONTROLLED_CLEANUP ${JSON.stringify(record)}\n` }, name), false);
  }
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
    for (const patch of [{ processId: 71 }, { bytes: 1 }, { sha256: '0'.repeat(64) },
      { dashboardEnabled: false }, { source: 'reconstructed' }, { publicationEligible: true },
      { configJson: '{}' }, { dashboardPath: '/other' }]) {
      const output = `POLARIS_DEFAULT_GRAPH ${JSON.stringify({ ...syntheticGraph(), ...patch })}\n`;
      rejectedBeforeWrite({ ...result, output });
    }
    rejectedBeforeWrite({ ...result, output: result.output + result.output });
    rejectedBeforeWrite(result, TESTS[0]);
    for (const mutate of [(g) => { g.services = []; }, (g) => { g.inbounds[0].listen = '0.0.0.0'; },
      (g) => { g.inbounds[0].type = 'tun'; }]) {
      const graph = JSON.parse(syntheticGraph().configJson); mutate(graph);
      const configJson = JSON.stringify(graph), bytes = Buffer.from(configJson);
      const row = { ...syntheticGraph(), configJson, bytes: bytes.length,
        sha256: createHash('sha256').update(bytes).digest('hex') };
      rejectedBeforeWrite({ ...result, output: `POLARIS_DEFAULT_GRAPH ${JSON.stringify(row)}\n` });
    }
    assert.equal(preserveProductionGraph({ ...result, output: '' }, TESTS[2], root, 'a'.repeat(64)), false);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
