import assert from 'node:assert/strict';
import test, { mock } from 'node:test';
import {
  appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { format } from 'node:util';
import { fileURLToPath } from 'node:url';
import { digest } from './desktop-core/source-graph.mjs';
import { selectedCoreManifest } from './lib/core-identity.mjs';
import { main } from './macos-nested-code.mjs';

// `macos-nested-code.mjs` 只在 macOS 打包腿上真跑（那里才有 codesign）。这里测的是它的**控制流**：
// 只读校验的分流、日志和失败传播。`file` 与 `codesign` 由注入的 `run` 模拟：
//   · `file`：内容以 `MACHO-` 开头 ⇒ 报 Mach-O，否则报文本；
//   · `codesign --verify`：内容含 SIGNED ⇒ 0；含 BROKEN ⇒ 1 并报「签名无效」；否则 1 并报
//     「code object is not signed at all」（真 codesign 对未签名文件的原话）；
//   · `codesign` 把每次调用的全部参数记进日志，一行一次；
//   · 发布核验由注入的结果代替；`real: true` 时用真的发布核验入口，用于核对它拒绝坏清单。
// `release-consumer.test.mjs` 覆盖正式核验入口的签名判据；本文件的模拟不给出真实验签证据。
// 真 codesign 的行为（linker-signed 签名是否过 `--strict`、`--deep` 是否触碰 Resources）不在本文件射程内。
const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), 'macos-nested-code.mjs');

function stubRun(log, missing) {
  return (command, args) => {
    const last = args.at(-1);
    if (command === missing) return { status: null, error: Object.assign(new Error(`spawnSync ${command} ENOENT`), { code: 'ENOENT' }) };
    if (command === 'file') {
      return { status: 0, stdout: readFileSync(last, 'utf8').startsWith('MACHO-') ? 'Mach-O 64-bit executable\n' : 'ASCII text\n', stderr: '' };
    }
    assert.equal(command, 'codesign');
    appendFileSync(log, `${args.join(' ')}\n`);
    if (args[0] !== '--verify') return { status: 64, stdout: '', stderr: '' };
    const body = readFileSync(last, 'utf8');
    if (body.includes('SIGNED')) return { status: 0, stdout: '', stderr: '' };
    if (body.includes('BROKEN')) return { status: 1, stdout: '', stderr: `${last}: invalid signature (code or signature have been modified)\n` };
    return { status: 1, stdout: '', stderr: `${last}: code object is not signed at all\n` };
  };
}

const ARM_SIGNED = 'MACHO-arm64 SIGNED';
const ARM_BARE = 'MACHO-arm64 bare';
const ARM_BROKEN = 'MACHO-arm64 BROKEN';
const X64_SIGNED = 'MACHO-x86_64 SIGNED';
const X64_BARE = 'MACHO-x86_64 bare';
const X64_BROKEN = 'MACHO-x86_64 BROKEN';
const FAT_BARE = 'MACHO-fat bare';

/**
 * 夹具根目录名带空格、单引号、`$` 与非 ASCII：脚本里任何一处漏引号都会在这里断。
 * `ref` 缺省与包内内核同内容（= 打包没有改写内核）。
 */
function fixture(files, { ref } = {}) {
  const root = mkdtempSync(join(tmpdir(), "polaris nested 'code' $x 核-"));
  mkdirSync(join(root, 'src-tauri'));
  writeFileSync(join(root, 'src-tauri/core-manifest.json'), '{}');
  const app = join(root, 'Polaris.app');
  const refBytes = ref ?? files['sing-box'] ?? ARM_SIGNED;
  const key = refBytes.startsWith('MACHO-x86_64') ? 'mac-x64' : 'mac-arm64';
  const core = join(app, 'Contents', 'Resources', '_up_', 'resources', key);
  mkdirSync(core, { recursive: true });
  for (const [name, body] of Object.entries(files)) writeFileSync(join(core, name), body);
  const refPath = join(root, 'resources', key, 'sing-box');
  mkdirSync(dirname(refPath), { recursive: true });
  writeFileSync(refPath, refBytes);
  const helper = files['polaris-helper'] ?? ARM_SIGNED;
  writeFileSync(join(dirname(refPath), 'polaris-helper'), helper);
  const contract = { version: '1.0.0-fixture', platforms: { [key]: {
    providerFingerprint: 'a'.repeat(64), core: { member: 'sing-box', bytes: Buffer.byteLength(refBytes), sha256: digest(refBytes) }, sidecar: null,
  } } };
  const installation = JSON.stringify(selectedCoreManifest(contract, key, '1.0.0-fixture', digest(helper)));
  for (const directory of [dirname(refPath), core]) writeFileSync(join(directory, 'core-manifest.json'), installation);
  const log = join(root, 'codesign.log');
  writeFileSync(log, '');
  return { root, app, core, ref: refPath, log, files };
}

const snapshot = (dir) => Object.fromEntries(readdirSync(dir).sort().map((name) => [name, readFileSync(join(dir, name), 'utf8')]));
const signCalls = (fx) => readFileSync(fx.log, 'utf8').split('\n').filter((line) => /--sign|--force|--remove-signature/.test(line));

/**
 * 每次运行都附带两条不随用例变化的不变量：
 *   · 包内内核与源内核的字节都没变；
 *   · 没有任何一次签名类调用的参数里出现内核路径。
 */
function run(fx, { state = 'signed', real = false, missing, host = 'darwin' } = {}) {
  const coreBefore = existsSync(join(fx.core, 'sing-box')) ? readFileSync(join(fx.core, 'sing-box'), 'utf8') : null;
  const refBefore = readFileSync(fx.ref, 'utf8');
  const stdout = [], stderr = [];
  const log = mock.method(console, 'log', (...args) => stdout.push(format(...args)));
  const error = mock.method(console, 'error', (...args) => stderr.push(format(...args)));
  let status;
  try {
    status = main([fx.app, fx.ref], stubRun(fx.log, missing),
      real ? undefined : () => ({ macCodeSignature: { state } }), host);
  } finally {
    log.mock.restore();
    error.mock.restore();
  }
  const result = { status, stdout: stdout.join('\n'), stderr: stderr.join('\n') };
  if (coreBefore !== null) {
    assert.equal(readFileSync(join(fx.core, 'sing-box'), 'utf8'), coreBefore, '包内内核字节被改写');
  }
  assert.equal(readFileSync(fx.ref, 'utf8'), refBefore, '源内核字节被改写');
  const touched = signCalls(fx).filter((line) => line.includes('sing-box') || line.includes('source-core'));
  assert.deepEqual(touched, [], '签名命令碰了内核');
  return result;
}

function withFixture(files, options, body) {
  const fx = fixture(files, options);
  try {
    return body(fx);
  } finally {
    rmSync(fx.root, { recursive: true, force: true });
  }
}

test('actual nested-code entry rejects malformed explicit release selection', () => {
  withFixture({ 'sing-box': ARM_SIGNED, 'polaris-helper': ARM_SIGNED }, {}, (fx) => {
    const real = { real: true };
    writeFileSync(join(fx.root, 'src-tauri/core-manifest.json'), JSON.stringify({ desktopRelease: null }));
    const rejected = run(fx, real);
    assert.notEqual(rejected.status, 0); assert.match(rejected.stderr, /[Ee]xplicit|selection|contract/);
    assert.match(rejected.stderr, /包内内核未通过发布核验/);
    assert.deepEqual(signCalls(fx), []);
    rmSync(join(fx.root, 'src-tauri/core-manifest.json'));
    const missing = run(fx, real);
    assert.notEqual(missing.status, 0); assert.match(missing.stderr, /core-manifest\.json/);
    assert.deepEqual(signCalls(fx), []);
  });
});

test('所有架构 helper 未签或损坏均拒绝且不补签', () => {
  for (const helper of [X64_BARE, ARM_BARE, FAT_BARE, X64_BROKEN, ARM_BROKEN]) {
    withFixture({ 'sing-box': ARM_SIGNED, 'polaris-helper': helper }, {}, (fx) => {
      const before = snapshot(fx.core), result = run(fx);
      assert.equal(result.status, 1, result.stdout);
      assert.match(result.stderr, /helper.*签名必须在 App 编译前完成/);
      assert.deepEqual(snapshot(fx.core), before);
      assert.deepEqual(signCalls(fx), []);
    });
  }
});

test('发布核验返回未签名状态时记录 Intel notice 和计数，缺少状态时拒绝', () => {
  withFixture({ 'sing-box': X64_BARE, 'polaris-helper': X64_SIGNED }, {}, (fx) => {
    const result = run(fx, { state: 'unsigned' });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /::notice::.*没有签名的 x86_64 内核，按原字节放行/);
    assert.match(result.stdout, /未签名的 x86_64 1 份/);
    assert.deepEqual(signCalls(fx), []);

    const missing = run(fx, { state: '' });
    assert.equal(missing.status, 1);
    assert.match(missing.stderr, /包内内核缺少已核验的签名状态/);
    assert.doesNotMatch(missing.stdout, /::notice::/);

    // 只有 macOS 上发布核验才观察实际签名；别处给出的签名状态不算数。
    const elsewhere = run(fx, { host: 'linux' });
    assert.equal(elsewhere.status, 1);
    assert.match(elsewhere.stderr, /包内内核未通过发布核验/);
  });
});

test('包内内核与源内核字节不同必须失败（签名有效也不行）', () => {
  withFixture({ 'sing-box': `${ARM_SIGNED} resigned`, 'polaris-helper': ARM_SIGNED }, { ref: ARM_SIGNED }, (fx) => {
    const result = run(fx);
    assert.equal(result.status, 1, result.stdout);
    assert.match(result.stderr, /包内内核与源内核字节不同/);
    assert.deepEqual(signCalls(fx), []);
  });
});

test('全部核验通过时保留原字节，helper 仍在本地逐个验签', () => {
  withFixture({ 'sing-box': ARM_SIGNED, 'polaris-helper': ARM_SIGNED, 'notes.txt': 'plain' }, {}, (fx) => {
    const before = snapshot(fx.core);
    const result = run(fx);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(snapshot(fx.core), before);
    assert.deepEqual(signCalls(fx), []);
    assert.doesNotMatch(result.stdout, /::warning::|::notice::/);
    const calls = readFileSync(fx.log, 'utf8');
    assert.match(calls, /--verify --strict .*polaris-helper/);
    assert.doesNotMatch(calls, /sing-box/);
  });
});

test('未登记的 Mach-O 验不过即失败，不替它补签', () => {
  withFixture({ 'sing-box': ARM_SIGNED, 'polaris-helper': ARM_SIGNED, 'stray-tool': ARM_BARE }, {}, (fx) => {
    const result = run(fx);
    assert.equal(result.status, 1, result.stdout);
    assert.match(result.stderr, /未登记的 Mach-O 没有有效签名：.*stray-tool/);
    assert.equal(readFileSync(join(fx.core, 'stray-tool'), 'utf8'), ARM_BARE);
    assert.deepEqual(signCalls(fx), []);
  });
});

test('两个必需 Mach-O 少一个必须失败', () => {
  for (const [files, need] of [
    [{ 'polaris-helper': ARM_SIGNED }, 'sing-box'],
    [{ 'sing-box': 'SIGNED but plain text', 'polaris-helper': ARM_SIGNED }, 'sing-box'],
    [{ 'sing-box': ARM_SIGNED }, 'polaris-helper'],
  ]) {
    withFixture(files, {}, (fx) => {
      const result = run(fx);
      assert.equal(result.status, 1, `缺 ${need} 应失败`);
      assert.match(result.stderr, new RegExp(`没有扫到 Mach-O「${need}」`));
    });
  }
});

test('符号链接不被跟随：指向包外的 sing-box 链接不算包内内核，包外文件不被触碰', () => {
  withFixture({ 'polaris-helper': ARM_SIGNED }, { ref: ARM_SIGNED }, (fx) => {
    const outside = join(fx.root, 'outside-core');
    writeFileSync(outside, ARM_BARE);
    symlinkSync(outside, join(fx.core, 'sing-box'));
    const result = run(fx);
    assert.equal(result.status, 1, result.stdout);
    assert.match(result.stderr, /没有扫到 Mach-O「sing-box」/);
    assert.equal(readFileSync(outside, 'utf8'), ARM_BARE);
    assert.doesNotMatch(readFileSync(fx.log, 'utf8'), /outside-core/);
  });
});

test('参数不对、缺源内核、缺工具时拒绝执行（rc=2），而不是静默通过', () => {
  withFixture({ 'sing-box': ARM_SIGNED, 'polaris-helper': ARM_SIGNED }, {}, (fx) => {
    const call = (args) => spawnSync(process.execPath, [SCRIPT, ...args], { encoding: 'utf8' });
    assert.equal(call(['check', fx.app, fx.ref]).status, 2);
    assert.equal(call([join(fx.root, 'nope.app'), fx.ref]).status, 2);
    const noRef = call([fx.app]);
    assert.equal(noRef.status, 2);
    assert.match(noRef.stderr, /用法 macos-nested-code.mjs <App.app> <源内核文件>/);
    const missingRef = call([fx.app, join(fx.root, 'missing-core')]);
    assert.equal(missingRef.status, 2);
    assert.match(missingRef.stderr, /第二个参数必须是源内核文件/);
    // 正向对照：同一套参数在工具齐全时是通过的，下面那次 rc=2 才能归因到「缺 codesign」。
    assert.equal(run(fx).status, 0);

    const noTool = run(fx, { missing: 'codesign' });
    assert.equal(noTool.status, 2, noTool.stderr);
    assert.match(noTool.stderr, /找不到命令 codesign/);
  });
});
