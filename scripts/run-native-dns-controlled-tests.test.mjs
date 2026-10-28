import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { readFileSync } from 'node:fs';
import { TESTS, selectExecutable, runtimeEnvironment, exactPass, runTests } from './run-native-dns-controlled-tests.mjs';

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

test('six exact processes retain package cwd, fail immediately and reject executable drift', async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'controlled-processes-')));
  try {
    const executable = join(root, 'fixture'), manifest = join(root, 'Cargo.toml');
    writeFileSync(executable, 'original');
    const selected = { executable, manifest };
    let calls = [];
    const execute = async (command, options) => {
      calls.push(command);
      assert.equal(options.root, root); assert.equal(options.env.FROZEN, 'yes');
      return { status: 0, output: 'test result: ok. 1 passed; 0 failed; 0 ignored;\n' };
    };
    await runTests(selected, { FROZEN: 'yes' }, root, execute);
    assert.deepEqual(calls.map((c) => c.args[0]), TESTS);
    assert.ok(calls.every((c) => c.program === executable && c.args.slice(1).join(' ') === '--exact --ignored --nocapture --test-threads=1'));
    for (const mode of ['zero', 'failure', 'drift']) {
      calls = [];
      writeFileSync(executable, 'original');
      await assert.rejects(runTests(selected, { FROZEN: 'yes' }, root, async (...args) => {
        const result = await execute(...args);
        if (mode === 'zero') result.output = 'test result: ok. 0 passed; 0 failed; 0 ignored;';
        if (mode === 'failure') result.status = 1;
        if (mode === 'drift') writeFileSync(executable, 'different bytes');
        return result;
      }));
      assert.equal(calls.length, 1, 'failed test or byte drift must stop later invocations');
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});
