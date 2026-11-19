import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { bindCommand, canonical, declaredPlan, gitEnvironment, load, sha, toolEnvironment } from './fork-input.mjs';
import { boundedModuleDownload, compilerFact, equivalentBinding, PRODUCER_FILES } from './producer.mjs';
import { install, recover } from './transaction.mjs';

function temporary(t) {
  const root = mkdtempSync(join(tmpdir(), 'polaris-producer-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

test('formal binding plan retains configured targets, BuildID and clean explicit tool environment', () => {
  const source = load();
  const android = declaredPlan('android', source);
  const apple = declaredPlan('apple', source);
  const fingerprint = sha(canonical(android.facts));
  const argv = bindCommand('android', source, '/tool/gomobile', '/work/stage.aar', fingerprint, '/sdk/android.jar');
  assert.ok(argv.includes('android/arm64,android/arm,android/386,android/amd64'));
  assert.ok(argv.some(arg => arg.endsWith('-buildid=polaris-mobile-' + fingerprint)));
  assert.deepEqual(argv.slice(argv.indexOf('-androidapi'), argv.indexOf('-androidapi') + 4), [
    '-androidapi', '24', '-bootclasspath', '/sdk/android.jar',
  ]);
  assert.ok(apple.argv.includes('ios/arm64,iossimulator/arm64'));
  assert.throws(() => bindCommand('android', source, '/tool/gomobile', '/bad', fingerprint), /bootclasspath required/);
  assert.throws(() => bindCommand('apple', source, '/tool/gomobile', '/bad', 'unknown'), /fingerprint required/);
  assert.equal(gitEnvironment({ GIT_DIR: 'foreign', PATH: 'explicit' }).GIT_DIR, undefined);
  const env = toolEnvironment('/go/go', '/tools/gomobile', '/tools/gobind', '/cache', '/scratch',
    { sdk: '/sdk', ndk: '/ndk', jdk: '/jdk' });
  assert.equal(env.GOTOOLCHAIN, 'local');
  assert.equal(env.ANDROID_NDK_HOME, '/ndk');
  assert.equal(env.GOMODCACHE, '/cache/go-mod');
  assert.equal(env.DEVELOPER_DIR, undefined);
});

test('component rollback restores the entire original pair and retains the rejected bytes', async t => {
  const root = temporary(t);
  const pairs = ['component', 'receipt', 'bind.log'].map(name => {
    const stage = join(root, 'stage-' + name), destination = join(root, name);
    writeFileSync(stage, 'new-' + name);
    writeFileSync(destination, 'old-' + name);
    return [stage, destination];
  });
  await assert.rejects(install(pairs, root, async () => {
    assert.equal(readFileSync(pairs[1][1], 'utf8'), 'new-receipt');
    throw new Error('late actual content rejection');
  }), /late actual content rejection/);
  assert.deepEqual(pairs.map(([, dest]) => readFileSync(dest, 'utf8')), ['old-component', 'old-receipt', 'old-bind.log']);
  assert.equal(readFileSync(join(root, 'rejected-pair/component'), 'utf8'), 'new-component');
  assert.ok(existsSync(join(root, 'recovered-transaction.json')));
});

test('interrupted recovery leaves equal unmoved bytes and rejects a foreign backup name', t => {
  const root = temporary(t), stage = join(root, 'stage'), dest = join(root, 'component');
  writeFileSync(stage, 'same');
  writeFileSync(dest, 'same');
  const pin = { bytes: 4, sha256: sha('same') };
  const journal = join(root, 'component-transaction.json');
  const row = { stage, destination: dest, name: '../component', stagePin: pin, originalPin: pin };
  const record = { schema: 'polaris-mobile-pair-transaction-v1', entries: [row] };
  writeFileSync(journal, canonical(record));
  assert.throws(() => recover([[stage, dest]], root), /fixed slots/);
  row.name = 'component';
  writeFileSync(journal, canonical(record));
  recover([[stage, dest]], root);
  assert.equal(readFileSync(dest, 'utf8'), 'same');
  assert.ok(!existsSync(join(root, 'rejected-pair')));
});

test('an occupied original recovery slot prevents replacement before touching stage or source', async t => {
  const root = temporary(t), stage = join(root, 'stage');
  mkdirSync(join(root, 'previous-pair'));
  writeFileSync(join(root, 'previous-pair/unique'), 'original');
  writeFileSync(stage, 'new');
  await assert.rejects(install([[stage, join(root, 'component')]], root, () => {}), /pair retained/);
  assert.equal(readFileSync(stage, 'utf8'), 'new');
  assert.equal(readFileSync(join(root, 'previous-pair/unique'), 'utf8'), 'original');
});

test('Android cache comparison ignores only reviewed Apple producer changes and no compiler change', () => {
  const pin = { bytes: 4, sha256: '1'.repeat(64) };
  const files = Object.fromEntries(PRODUCER_FILES.map(name => [name, pin]));
  const original = { platform: 'android', producer: { files, candidate: 'previous' } };
  const current = structuredClone(original);
  current.producer.candidate = 'next';
  for (const name of ['apple-admission.mjs', 'ci-apple.mjs', 'apple-evidence.mjs']) {
    current.producer.files['scripts/mobile-core/' + name] = { ...pin, sha256: '2'.repeat(64) };
  }
  assert.ok(equivalentBinding(original, current));
  current.producer.files['scripts/mobile-core/source-admission.mjs'] = { ...pin, sha256: '3'.repeat(64) };
  assert.ok(!equivalentBinding(original, current));
  assert.equal(original.producer.files['scripts/mobile-core/source-admission.mjs'].sha256, pin.sha256);
});

test('compiler fact runs the bound symlink driver then refuses a changed physical file', async t => {
  const root = temporary(t), tool = join(root, 'clang'), driver = join(root, 'clang++');
  writeFileSync(tool, 'old');
  symlinkSync('clang', driver);
  let invoked;
  const good = await compilerFact(driver, {}, async argv => { invoked = argv; return 'clang version'; });
  assert.equal(good.path, tool);
  assert.deepEqual(invoked, [driver, '--version']);
  await assert.rejects(compilerFact(driver, {}, async () => {
    writeFileSync(tool, 'new');
    return 'clang version';
  }), /physical bytes changed/);
});

test('module collection admits exact split JSON and records curated bounded failure without raw data', async t => {
  const root = temporary(t);
  function runner(raw) {
    const bytes = Buffer.from(raw);
    return async (_argv, options) => {
      let reason, stdoutBytesObserved = 0;
      for (let offset = 0; offset < bytes.length && !reason; offset += 7) {
        const chunk = bytes.subarray(offset, offset + 7);
        stdoutBytesObserved += chunk.length;
        reason = options.onStdout(chunk);
      }
      return { complete: !reason, reason, cleanupComplete: true, stdoutBytesObserved, stderrBytesObserved: 0 };
    };
  }
  const valid = '{"Path":"module","Version":"v1.2.3","Sum":"h1:value","Dir":"/cache/path"}';
  const observation = join(root, 'module.json');
  const rows = await boundedModuleDownload(runner('\n' + valid + '\n'), [], { expectedCount: 1, observation });
  assert.equal(rows[0].Dir, '/cache/path');
  const refused = join(root, 'refused.json');
  await assert.rejects(boundedModuleDownload(runner(valid + valid), [], { expectedCount: 1, observation: refused }),
    /structure-or-item-budget/);
  const raw = readFileSync(refused, 'utf8');
  const report = JSON.parse(raw);
  assert.equal(report.complete, false);
  assert.equal(report.cleanupComplete, true);
  assert.equal(report.rawOutputRetained, false);
  assert.ok(!raw.includes('/cache/path'));
});
