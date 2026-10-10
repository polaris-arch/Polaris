import assert from 'node:assert/strict';
import test from 'node:test';
import {
  chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import { delimiter, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonical, digest } from './desktop-core/source-graph.mjs';

// `macos-nested-code.sh` 只在 macOS 打包腿上真跑（那里才有 codesign）。这里测的是它的**控制流**：
// 谁只被校验、谁可以被补签、什么时候必须失败。`file` 与 `codesign` 换成同名桩放在 PATH 最前：
//   · 桩 `file`：内容以 `MACHO-<arch>` 开头 ⇒ 报对应架构的 Mach-O，否则报文本；
//   · 桩 `codesign --verify`：内容含 SIGNED ⇒ 0；含 BROKEN ⇒ 1 并报「签名无效」；否则 1 并报
//     「code object is not signed at all」（真 codesign 对未签名文件的原话）；
//   · 桩 `codesign --sign`：往文件尾追加 SIGNED（`STUB_SIGN_NOOP=1` 时什么都不做）；
//   · 桩 `codesign` 把每次调用的全部参数记进 `$STUB_LOG`，一行一次。
// 真 codesign 的行为（linker-signed 签名是否过 `--strict`、`--deep` 是否触碰 Resources）不在本文件射程内。
const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), 'macos-nested-code.sh');
const scriptText = readFileSync(SCRIPT, 'utf8');

// 行为用例要真起 bash 并靠 `#!/bin/sh` 桩劫持 PATH：Windows 上桩不可执行、路径形态也不同，整组跳过，
// 跳过原因写在用例名旁（TAP 的 `# SKIP`）。Linux / macOS 上 bash 必须在，缺了是红不是跳过（见下一条用例）。
const posix = process.platform !== 'win32';
const hasBash = posix && spawnSync('bash', ['-c', 'exit 0']).status === 0;
const behaviour = hasBash
  ? {}
  : { skip: posix ? '本机没有 bash' : 'Windows：行为用例依赖 POSIX 桩与 bash，脚本本体只在 macOS 打包腿运行' };

test('Linux / macOS 上行为用例不得被跳过（bash 必须可用）', { skip: posix ? false : 'Windows 不跑行为用例' }, () => {
  assert.equal(hasBash, true, '找不到 bash —— 下面的行为用例会整组跳过，不能当作通过');
});

const STUB_FILE = `#!/bin/sh
for last; do :; done
case "$(head -c 12 "$last")" in
  MACHO-arm64*) echo "Mach-O 64-bit executable arm64" ;;
  MACHO-x86_64) echo "Mach-O 64-bit executable x86_64" ;;
  MACHO-fat*) echo "Mach-O universal binary with 2 architectures: [x86_64:Mach-O 64-bit executable x86_64] [arm64]" ;;
  *) echo "ASCII text" ;;
esac
`;
const STUB_CODESIGN = `#!/bin/sh
for last; do :; done
printf '%s\\n' "$*" >> "$STUB_LOG"
case " $* " in
  *" --verify "*)
    if grep -q SIGNED "$last"; then exit 0; fi
    if grep -q BROKEN "$last"; then echo "$last: invalid signature (code or signature have been modified)" >&2; exit 1; fi
    echo "$last: code object is not signed at all" >&2; exit 1 ;;
  *" --display "*) echo "CDHash=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" >&2 ;;
  *" --sign "*) [ "\${STUB_SIGN_NOOP:-0}" = 1 ] || printf SIGNED >> "$last" ;;
  *) exit 64 ;;
esac
`;

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
  const bin = join(root, 'bin');
  mkdirSync(bin);
  for (const [name, body] of [['file', STUB_FILE], ['codesign', STUB_CODESIGN]]) {
    writeFileSync(join(bin, name), body);
    chmodSync(join(bin, name), 0o755);
  }
  const app = join(root, 'Polaris.app');
  const core = join(app, 'Contents', 'Resources', '_up_', 'resources', 'mac-arm64');
  mkdirSync(core, { recursive: true });
  for (const [name, body] of Object.entries(files)) writeFileSync(join(core, name), body);
  const refBytes = ref ?? files['sing-box'] ?? ARM_SIGNED;
  const key = refBytes.startsWith('MACHO-x86_64') ? 'mac-x64' : 'mac-arm64';
  const refPath = join(root, 'resources', key, 'sing-box');
  mkdirSync(dirname(refPath), { recursive: true });
  writeFileSync(refPath, refBytes);
  const metadata = join(root, 'resources/.source-receipts');
  mkdirSync(metadata);
  const facts = { schema: 'polaris-desktop-core-v1', platform: key, binarySha256: digest(refBytes),
    macCodeSignature: refBytes.includes('SIGNED') ? { state: 'signed', cdHash: 'a'.repeat(40) } : { state: 'unsigned', cdHash: null } };
  const receiptPath = join(metadata, `${key}.json`);
  writeFileSync(receiptPath, JSON.stringify({ ...facts, fingerprint: digest(canonical(facts)) }));
  const log = join(root, 'codesign.log');
  writeFileSync(log, '');
  return { root, app, core, bin, ref: refPath, receiptPath, log, files };
}

const snapshot = (dir) => Object.fromEntries(readdirSync(dir).sort().map((name) => [name, readFileSync(join(dir, name), 'utf8')]));
const signCalls = (fx) => readFileSync(fx.log, 'utf8').split('\n').filter((line) => /--sign|--force|--remove-signature/.test(line));

/**
 * 每次运行都附带两条不随用例变化的不变量：
 *   · 包内内核与源内核的字节都没变；
 *   · 没有任何一次签名类调用的参数里出现内核路径。
 */
function run(fx, mode, env = {}, shell = 'bash') {
  const coreBefore = existsSync(join(fx.core, 'sing-box')) ? readFileSync(join(fx.core, 'sing-box'), 'utf8') : null;
  const refBefore = readFileSync(fx.ref, 'utf8');
  const result = spawnSync(shell, [SCRIPT, mode, fx.app, fx.ref], {
    encoding: 'utf8',
    env: { ...process.env, ...env, STUB_LOG: fx.log, PATH: `${fx.bin}${delimiter}${process.env.PATH}` },
  });
  if (coreBefore !== null) {
    assert.equal(readFileSync(join(fx.core, 'sing-box'), 'utf8'), coreBefore, `${mode}: 包内内核字节被改写`);
  }
  assert.equal(readFileSync(fx.ref, 'utf8'), refBefore, `${mode}: 源内核字节被改写`);
  const touched = signCalls(fx).filter((line) => line.includes('sing-box') || line.includes('source-core'));
  assert.deepEqual(touched, [], `${mode}: 签名命令碰了内核`);
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

test('seal：内核只校验，验不过的 helper 才被补签，签名命令的唯一对象是 helper', behaviour, () => {
  withFixture({ 'sing-box': ARM_SIGNED, 'polaris-helper': ARM_BARE, 'notes.txt': 'plain' }, {}, (fx) => {
    const result = run(fx, 'seal');
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(join(fx.core, 'polaris-helper'), 'utf8'), `${ARM_BARE}SIGNED`);
    assert.equal(readFileSync(join(fx.core, 'notes.txt'), 'utf8'), 'plain');
    const signs = signCalls(fx);
    assert.equal(signs.length, 1, `签名类调用应恰好 1 次：${JSON.stringify(signs)}`);
    assert.ok(signs[0].endsWith('/polaris-helper'), signs[0]);
    assert.match(result.stdout, /::warning::.*polaris-helper 没有有效签名/);
    assert.match(result.stdout, /内核 1 份与源内核逐字节相同.*本次补签 1 份/);
  });
});

test('seal：helper 补签的点名级别 —— 纯 x86_64 没有签名是每次打包的预期形态（notice），其余是异常（warning）', behaviour, () => {
  for (const [helper, core, level] of [
    [X64_BARE, X64_SIGNED, 'notice'],
    [ARM_BARE, ARM_SIGNED, 'warning'],
    [FAT_BARE, ARM_SIGNED, 'warning'],
    [X64_BROKEN, X64_SIGNED, 'warning'],
    [ARM_BROKEN, ARM_SIGNED, 'warning'],
  ]) {
    withFixture({ 'sing-box': core, 'polaris-helper': helper }, {}, (fx) => {
      const result = run(fx, 'seal');
      assert.equal(result.status, 0, `${helper}: ${result.stderr}`);
      const named = result.stdout.split('\n').filter((line) => /polaris-helper 没有有效签名/.test(line));
      assert.equal(named.length, 1, `${helper}: 补签必须点名恰好一次：${result.stdout}`);
      assert.ok(named[0].startsWith(`::${level}::macos-nested-code: `), `${helper}: 应为 ${level}，实为 ${named[0]}`);
      // 级别只影响提示，不影响动作：照样补签、照样复验、照样计数。
      assert.equal(signCalls(fx).length, 1);
      assert.equal(readFileSync(join(fx.core, 'polaris-helper'), 'utf8'), `${helper}SIGNED`);
      assert.match(result.stdout, /本次补签 1 份/);
    });
  }
  // verify 模式不因级别放宽而放行：没有签名的 x86_64 helper 在最终产物里仍是失败。
  withFixture({ 'sing-box': X64_SIGNED, 'polaris-helper': X64_BARE }, {}, (fx) => {
    const result = run(fx, 'verify');
    assert.equal(result.status, 1, result.stdout);
    assert.match(result.stderr, /最终产物里的 polaris-helper 没有有效签名/);
    assert.deepEqual(signCalls(fx), []);
  });
});

test('seal：arm64 内核没有签名或签名无效必须失败，且不补签（一次签名命令都不发）', behaviour, () => {
  for (const [core, why] of [[ARM_BARE, /内核没有签名/], [ARM_BROKEN, /内核带有无效签名/], [FAT_BARE, /内核没有签名/]]) {
    withFixture({ 'sing-box': core, 'polaris-helper': ARM_SIGNED }, {}, (fx) => {
      const result = run(fx, 'seal');
      assert.equal(result.status, 1, `${core}: ${result.stdout}`);
      assert.match(result.stderr, why);
      assert.match(result.stderr, /应在内核构建产物那一步签名.*scripts\/core-patches\/README\.md/);
      assert.deepEqual(signCalls(fx), []);
      assert.equal(readFileSync(join(fx.core, 'sing-box'), 'utf8'), core);
    });
  }
});

test('纯 x86_64 内核没有签名时放行并留一条 notice；签名无效仍失败', behaviour, () => {
  for (const mode of ['seal', 'verify']) {
    withFixture({ 'sing-box': X64_BARE, 'polaris-helper': X64_SIGNED }, {}, (fx) => {
      const result = run(fx, mode);
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /::notice::.*没有签名的 x86_64 内核，按原字节放行/);
      assert.match(result.stdout, /未签名的 x86_64 1 份/);
      assert.deepEqual(signCalls(fx), []);
    });
    withFixture({ 'sing-box': X64_BROKEN, 'polaris-helper': X64_SIGNED }, {}, (fx) => {
      const result = run(fx, mode);
      assert.equal(result.status, 1);
      assert.match(result.stderr, /内核带有无效签名/);
      assert.deepEqual(signCalls(fx), []);
    });
  }
});

test('包内内核与源内核字节不同必须失败（签名有效也不行）', behaviour, () => {
  for (const mode of ['seal', 'verify']) {
    withFixture({ 'sing-box': `${ARM_SIGNED} resigned`, 'polaris-helper': ARM_SIGNED }, { ref: ARM_SIGNED }, (fx) => {
      const result = run(fx, mode);
      assert.equal(result.status, 1, result.stdout);
      assert.match(result.stderr, /包内内核与源内核字节不同/);
      assert.deepEqual(signCalls(fx), []);
    });
  }
});

test('verify：全部有效时通过，不改任何字节，不发任何签名命令', behaviour, () => {
  withFixture({ 'sing-box': ARM_SIGNED, 'polaris-helper': ARM_SIGNED, 'notes.txt': 'plain' }, {}, (fx) => {
    const before = snapshot(fx.core);
    const result = run(fx, 'verify');
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(snapshot(fx.core), before);
    assert.deepEqual(signCalls(fx), []);
    assert.doesNotMatch(result.stdout, /::warning::|::notice::/);
    // 反向对照：桩确实被调到了（否则「没有签名命令」只是因为桩根本没在 PATH 上）。
    assert.match(readFileSync(fx.log, 'utf8'), /--verify --strict .*sing-box/);
  });
});

test('verify：内核或 helper 验不过都必须失败，且只读（不补签、不改字节）', behaviour, () => {
  for (const [files, why] of [
    [{ 'sing-box': ARM_BARE, 'polaris-helper': ARM_SIGNED }, /内核没有签名/],
    [{ 'sing-box': ARM_SIGNED, 'polaris-helper': ARM_BARE }, /最终产物里的 polaris-helper 没有有效签名/],
    [{ 'sing-box': ARM_SIGNED, 'polaris-helper': ARM_BROKEN }, /最终产物里的 polaris-helper 没有有效签名/],
  ]) {
    withFixture(files, {}, (fx) => {
      const before = snapshot(fx.core);
      const result = run(fx, 'verify');
      assert.equal(result.status, 1, result.stdout);
      assert.match(result.stderr, why);
      assert.deepEqual(snapshot(fx.core), before);
      assert.deepEqual(signCalls(fx), []);
    });
  }
});

test('seal：helper 的签名命令 rc=0 但没签上，必须失败（不把「跑过签名」当成「签上了」）', behaviour, () => {
  withFixture({ 'sing-box': ARM_SIGNED, 'polaris-helper': ARM_BARE }, {}, (fx) => {
    const result = run(fx, 'seal', { STUB_SIGN_NOOP: '1' });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /补签后仍验不过/);
  });
});

test('未登记的 Mach-O 验不过即失败，不替它补签', behaviour, () => {
  for (const mode of ['seal', 'verify']) {
    withFixture({ 'sing-box': ARM_SIGNED, 'polaris-helper': ARM_SIGNED, 'stray-tool': ARM_BARE }, {}, (fx) => {
      const result = run(fx, mode);
      assert.equal(result.status, 1, result.stdout);
      assert.match(result.stderr, /未登记的 Mach-O 没有有效签名：.*stray-tool/);
      assert.equal(readFileSync(join(fx.core, 'stray-tool'), 'utf8'), ARM_BARE);
      assert.deepEqual(signCalls(fx), []);
    });
  }
});

test('两种模式：两个必需 Mach-O 少一个必须失败（一个都没验 ≠ 全部通过）', behaviour, () => {
  for (const mode of ['seal', 'verify']) {
    for (const [files, need] of [
      [{ 'polaris-helper': ARM_SIGNED }, 'sing-box'],
      [{ 'sing-box': 'SIGNED but plain text', 'polaris-helper': ARM_SIGNED }, 'sing-box'],
      [{ 'sing-box': ARM_SIGNED }, 'polaris-helper'],
    ]) {
      withFixture(files, {}, (fx) => {
        const result = run(fx, mode);
        assert.equal(result.status, 1, `${mode}: 缺 ${need} 应失败`);
        assert.match(result.stderr, new RegExp(`没有扫到 Mach-O「${need}」`));
      });
    }
  }
});

test('符号链接不被跟随：指向包外的 sing-box 链接不算包内内核，包外文件不被触碰', behaviour, () => {
  withFixture({ 'polaris-helper': ARM_SIGNED }, { ref: ARM_SIGNED }, (fx) => {
    const outside = join(fx.root, 'outside-core');
    writeFileSync(outside, ARM_BARE);
    symlinkSync(outside, join(fx.core, 'sing-box'));
    const result = run(fx, 'seal');
    assert.equal(result.status, 1, result.stdout);
    assert.match(result.stderr, /没有扫到 Mach-O「sing-box」/);
    assert.equal(readFileSync(outside, 'utf8'), ARM_BARE);
    assert.doesNotMatch(readFileSync(fx.log, 'utf8'), /outside-core/);
  });
});

test('参数不对、缺源内核、缺工具时拒绝执行（rc=2），而不是静默通过', behaviour, () => {
  withFixture({ 'sing-box': ARM_SIGNED, 'polaris-helper': ARM_SIGNED }, {}, (fx) => {
    const call = (args, env = {}) => spawnSync('bash', [SCRIPT, ...args], {
      encoding: 'utf8',
      env: { ...process.env, STUB_LOG: fx.log, PATH: `${fx.bin}${delimiter}${process.env.PATH}`, ...env },
    });
    assert.equal(call(['check', fx.app, fx.ref]).status, 2);
    assert.equal(call(['verify', join(fx.root, 'nope.app'), fx.ref]).status, 2);
    const noRef = call(['verify', fx.app]);
    assert.equal(noRef.status, 2);
    assert.match(noRef.stderr, /第三个参数必须是源内核文件/);
    assert.equal(call(['verify', fx.app, join(fx.root, 'missing-core')]).status, 2);
    // 正向对照：同一套参数在工具齐全时是通过的，下面那次 rc=2 才能归因到「缺 codesign」。
    assert.equal(call(['verify', fx.app, fx.ref]).status, 0);

    // PATH 里只留脚本需要的其它工具，唯独没有 codesign。
    const lean = join(fx.root, 'lean-bin');
    mkdirSync(lean);
    for (const tool of ['bash', 'find', 'cmp', 'mktemp', 'rm', 'head']) {
      const found = spawnSync('bash', ['-c', `command -v ${tool}`], { encoding: 'utf8' }).stdout.trim();
      assert.ok(found.startsWith('/'), `本机找不到 ${tool}`);
      symlinkSync(found, join(lean, tool));
    }
    writeFileSync(join(lean, 'file'), STUB_FILE);
    chmodSync(join(lean, 'file'), 0o755);
    const noTool = call(['verify', fx.app, fx.ref], { PATH: lean });
    assert.equal(noTool.status, 2, noTool.stderr);
    assert.match(noTool.stderr, /找不到命令 codesign/);
  });
});

// ── 以下两条不起进程，三平台都跑 ──

/** 取顶层函数 `name() { … }` 的本体行（到行首的 `}` 为止）。 */
function functionBody(name) {
  const lines = scriptText.split(/\r?\n/);
  const start = lines.indexOf(`${name}() {`);
  assert.ok(start >= 0, `脚本里没有函数 ${name}`);
  const end = lines.indexOf('}', start);
  assert.ok(end > start, `函数 ${name} 未闭合`);
  return lines.slice(start + 1, end);
}
const codeLines = (lines) => lines.filter((line) => !/^\s*#/.test(line));

test('源码面：会改写文件的命令只有 helper 那一处，内核路径上没有任何签名或改写命令', () => {
  const MUTATING = /--sign\b|--force\b|--remove-signature|\bstrip\b|\blipo\b|install_name_tool|\bxattr\b|\bchmod\b|\bcp\b|\bmv\b|\btouch\b/;
  const all = codeLines(scriptText.split(/\r?\n/));
  // 取材面自检：剥注释不能把代码剥没。
  assert.ok(all.some((line) => line.includes('codesign --verify --strict')));

  const mutating = all.filter((line) => MUTATING.test(line)).map((line) => line.trim());
  assert.deepEqual(mutating, ['codesign --force --sign - --timestamp=none "$f"']);
  assert.ok(codeLines(functionBody('check_helper')).some((line) => line.trim() === mutating[0]), '补签不在 check_helper 里');

  for (const name of ['check_core', 'sig_state']) {
    const body = codeLines(functionBody(name));
    assert.ok(body.length > 2, `${name} 的本体取空了`);
    assert.deepEqual(body.filter((line) => MUTATING.test(line)), [], `${name} 里出现改写命令`);
  }
  assert.ok(codeLines(functionBody('check_core')).some((line) => line.includes('cmp -s "$f" "$core_ref"')));
  // 内核只经 check_core：分流点里 `sing-box)` 分支的第一条命令就是它。
  const branch = all.findIndex((line) => line.trim() === 'sing-box)');
  assert.ok(branch >= 0, '找不到内核分流分支');
  assert.equal(all[branch + 1].trim(), 'check_core "$f" "$kind" "$rel"');
});

// ── bash 3.2 ──
// 脚本在 macOS 打包腿上由系统自带的 bash 3.2 执行。能在本机回答「3.2 跑不跑得了」的只有真的 3.2 二进制：
// macOS 上 `/bin/bash` 就是；别处可经 `POLARIS_BASH32` 指一个过来。两者都没有时（Linux 开发机与 CI 的
// Linux 腿通常如此）下面那条真跑用例显式跳过，本机能做的只剩三样，各自的效力写在各自的用例里：
//   · `bash -n`：语法过得了本机的 bash（不是 3.2 的语法分析器）；
//   · 静态面：一张「bash 4 及以后才有」的写法表，对脚本代码面零命中；
//   · `BASH_COMPAT=3.2`：只把 bash 文档列出的那几项行为切回 3.2，不会让 4+ 的语法变成错误。
// 真判断在 macOS 打包腿：那里脚本被 3.2 实际执行，跑不了即打包失败。
const versionOf = (shell) => {
  const probe = spawnSync(shell, ['-c', 'echo "${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"'], { encoding: 'utf8' });
  return probe.status === 0 ? probe.stdout.trim() : null;
};
const localBash = hasBash ? versionOf('bash') : null;
const bash32 = posix ? ['/bin/bash', process.env.POLARIS_BASH32].find((shell) => shell && versionOf(shell) === '3.2') ?? null : null;

test(
  'bash 3.2 真跑：seal 与 verify 在 3.2 二进制上给出与默认 bash 相同的结论',
  {
    skip: bash32
      ? false
      : `本机没有 bash 3.2（bash 是 ${localBash ?? '缺失'}；/bin/bash 与 POLARIS_BASH32 都不是 3.2）—— 「3.2 可跑」在这里没有被执行过，真判断在 macOS 打包腿`,
  },
  () => {
    withFixture({ 'sing-box': ARM_SIGNED, 'polaris-helper': ARM_BARE, 'notes.txt': 'plain' }, {}, (fx) => {
      const sealed = run(fx, 'seal', {}, bash32);
      assert.equal(sealed.status, 0, sealed.stderr);
      assert.match(sealed.stdout, /内核 1 份与源内核逐字节相同.*本次补签 1 份/);
      const verified = run(fx, 'verify', {}, bash32);
      assert.equal(verified.status, 0, verified.stderr);
    });
    withFixture({ 'sing-box': ARM_BARE, 'polaris-helper': ARM_SIGNED }, {}, (fx) => {
      const result = run(fx, 'seal', {}, bash32);
      assert.equal(result.status, 1, result.stdout);
      assert.match(result.stderr, /内核没有签名/);
    });
  }
);

/**
 * bash 4 及以后才有的写法：[判据, 说明, 一条必须命中的样例]。样例是这张表的反向对照 ——
 * 判据写错成什么都匹不到时，红在样例上，而不是让「零命中」恒真。
 */
const BASH4_ONLY = [
  [/\b(declare|typeset|local)\s+-[A-Za-z]*[AngluIc]/, 'declare / local 的 -A -n -g -l -u -c -I', 'declare -A seen'],
  [/\b(mapfile|readarray)\b/, 'mapfile / readarray', 'mapfile -t lines <"$list"'],
  [/\bcoproc\b/, 'coproc', 'coproc sig { codesign --verify "$f"; }'],
  [/\$\{[^}]*(\^|,)[^}]*\}/, '大小写展开 ${var^^} ${var,,} ${var^} ${var,}', 'kind="${kind,,}"'],
  [/\$\{[A-Za-z_][A-Za-z0-9_]*(\[[@*]\])?@[A-Za-z]\}/, '参数变换 ${var@Q}', 'echo "${rel@Q}"'],
  [/&>>|\|&/, '&>> 与 |&', 'codesign --verify "$f" |& tee -a "$log"'],
  [/;;&|;&/, 'case 的 ;& 与 ;;&', '  signed) count=$((count + 1)) ;&'],
  [/\bshopt\s+-s\s+(globstar|lastpipe|autocd|checkjobs|dirspell|inherit_errexit|localvar_\w+|compat\d+)/, 'bash 4+ 的 shopt 选项', 'shopt -s globstar'],
  [/\bread\b[^#]*\s-[A-Za-z]*[iN]\b/, 'read -i / -N', 'read -r -N 4 magic <"$f"'],
  [/\bwait\s+-[A-Za-z]*[nfp]\b/, 'wait -n / -f / -p', 'wait -n'],
  [/\$\{?(EPOCHSECONDS|EPOCHREALTIME|BASHPID|SRANDOM|BASH_ARGV0|BASH_XTRACEFD)\b/, 'bash 4+ 的特殊变量', 'tmp="/tmp/x.$BASHPID"'],
  [/\$\{[A-Za-z_][A-Za-z0-9_]*\[-\d+\]\}/, '负数下标 ${arr[-1]}', 'last="${parts[-1]}"'],
  [/\$\{[A-Za-z_][A-Za-z0-9_]*:[^}:]*:\s*-\d/, '负数长度的子串 ${var:0:-1}', 'stem="${base:0:-4}"'],
  [/\[\[?\s[^\]]*-v\s+[A-Za-z_]/, '[[ -v var ]]', 'if [[ -v core_ref ]]; then'],
  [/printf\s[^#]*%\([^)]*\)T/, "printf '%(fmt)T'", "printf '%(%F)T\\n' -1"],
  [/\{\d+\.\.\d+\.\.\d+\}|\{0\d+\.\.\d+\}/, '带步长或补零的花括号序列', 'for i in {01..10}; do'],
  [/\$'[^']*\\[uU][0-9A-Fa-f]/, "$'\\u…' 转义", "sep=$'\\u2028'"],
  [/"\$\{[A-Za-z_][A-Za-z0-9_]*\[@\]\}"/, '"${arr[@]}"（set -u 下空数组在 bash 4.4 之前按未绑定变量报错）', 'codesign "${args[@]}" "$f"'],
];

test('bash 3.2 兼容的静态面：bash -n 通过，bash 4+ 才有的写法对脚本代码面零命中（不等于在 3.2 上跑过）', behaviour, () => {
  const all = codeLines(scriptText.split(/\r?\n/));
  // 取材面自检：代码面取到了，且脚本确实开着 set -u（上表最后一条的前提）。
  assert.ok(all.length > 80, `代码面只有 ${all.length} 行`);
  assert.ok(all.includes('set -euo pipefail'));

  assert.ok(BASH4_ONLY.length >= 18, `写法表缩水了：${BASH4_ONLY.length}`);
  for (const [pattern, what, sample] of BASH4_ONLY) {
    assert.ok(pattern.test(sample), `判据「${what}」匹不到它自己的样例 ${sample}`);
    assert.deepEqual(all.filter((line) => pattern.test(line)), [], `用了 bash 4+ 才有的 ${what}`);
  }
  // 脚本里实际用到的 3.2 写法不该被上表误伤（表写得过宽时红在这里）。
  for (const used of ['while IFS= read -r -d \'\' f; do', '  rel="${f:$((app_len + 1))}"', '  base="${f##*/}"', '  local f="$1" kind="$2" rel="$3"', '  exit "${2:-1}"']) {
    assert.ok(scriptText.includes(used), `脚本里找不到 ${used}`);
    assert.deepEqual(BASH4_ONLY.filter(([pattern]) => pattern.test(used)).map(([, what]) => what), [], used);
  }

  // `bash -n`：本机 bash 的语法分析器。反向对照：同一条命令对坏脚本确实非零。
  const parsed = spawnSync('bash', ['-n', SCRIPT], { encoding: 'utf8' });
  assert.equal(parsed.status, 0, parsed.stderr);
  const tmp = mkdtempSync(join(tmpdir(), 'polaris-bash-n-'));
  try {
    const broken = join(tmp, 'broken.sh');
    writeFileSync(broken, 'case "$1" in\n  a) echo a\n');
    assert.notEqual(spawnSync('bash', ['-n', broken]).status, 0, 'bash -n 没有对坏脚本报错');
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});

// `BASH_COMPAT=3.2` 把 bash 文档列出的那几项行为（`[[` 里的字符串比较与引号、`<` `>` 的排序口径等）切回 3.2。
// 它不关掉任何 4+ 的语法，所以这条说明的是「脚本的结论不依赖那几项行为」，不是「脚本在 3.2 上能跑」。
const compat32 = hasBash && spawnSync('bash', ['-c', 'shopt -q compat32'], { env: { ...process.env, BASH_COMPAT: '3.2' } }).status === 0;
test(
  'BASH_COMPAT=3.2 下 seal / verify 的结论与默认一致（只切换 bash 文档列出的兼容行为）',
  { skip: compat32 ? false : hasBash ? `本机 bash ${localBash} 不接受 BASH_COMPAT=3.2` : behaviour.skip },
  () => {
    // 反向对照：不设该变量时 compat32 是关的，上面的探测不是恒真。
    assert.notEqual(spawnSync('bash', ['-c', 'shopt -q compat32'], { env: { ...process.env, BASH_COMPAT: '' } }).status, 0);
    for (const env of [{}, { BASH_COMPAT: '3.2' }]) {
      withFixture({ 'sing-box': X64_BARE, 'polaris-helper': X64_BARE, 'notes.txt': 'plain' }, {}, (fx) => {
        const sealed = run(fx, 'seal', env);
        assert.equal(sealed.status, 0, sealed.stderr);
        assert.match(sealed.stdout, /未签名的 x86_64 1 份.*本次补签 1 份/);
        assert.equal(run(fx, 'verify', env).status, 0);
      });
      withFixture({ 'sing-box': ARM_BROKEN, 'polaris-helper': ARM_SIGNED }, {}, (fx) => {
        const result = run(fx, 'verify', env);
        assert.equal(result.status, 1, result.stdout);
        assert.match(result.stderr, /内核带有无效签名/);
      });
    }
  }
);

test('源码面：find 不跟随链接、清单按 NUL 读、路径变量逐条带引号', () => {
  const all = codeLines(scriptText.split(/\r?\n/));
  assert.ok(all.some((line) => line.includes('find "$res" -type f -print0')), 'find 必须 -type f（不跟随链接）且 -print0');
  assert.ok(all.some((line) => line.includes("read -r -d ''")), '清单必须按 NUL 读');
  // 文件路径变量出现在命令参数里时必须带双引号。
  const unquoted = all.filter((line) => /(^|[\s=(])\$(f|app|res|core_ref|list)\b/.test(line));
  assert.deepEqual(unquoted, [], '路径变量未加引号');
  // 反向对照：判据对一条漏引号的写法确实命中。
  assert.ok(/(^|[\s=(])\$(f|app|res|core_ref|list)\b/.test('  codesign --verify --strict $f'));
});

test('包内实际state/CDHash必须与源receipt相符；缺receipt及签后改字节拒绝且不补签', behaviour, () => {
  for (const mutation of ['state', 'cdHash', 'fingerprint', 'hash', 'missing', 'platform']) {
    withFixture({ 'sing-box': X64_SIGNED, 'polaris-helper': X64_SIGNED }, {}, (fx) => {
      const receipt = JSON.parse(readFileSync(fx.receiptPath));
      if (mutation === 'missing') rmSync(fx.receiptPath);
      else {
        if (mutation === 'state') receipt.macCodeSignature = { state: 'unsigned', cdHash: null };
        if (mutation === 'cdHash') receipt.macCodeSignature.cdHash = 'b'.repeat(40);
        if (mutation === 'hash') receipt.binarySha256 = digest('changed signed bytes');
        if (mutation === 'platform') receipt.platform = 'mac-arm64';
        if (mutation !== 'fingerprint') {
          const { fingerprint: _old, ...facts } = receipt;
          receipt.fingerprint = digest(canonical(facts));
        } else receipt.macCodeSignature.cdHash = 'b'.repeat(40);
        writeFileSync(fx.receiptPath, JSON.stringify(receipt));
      }
      const result = run(fx, 'verify');
      assert.notEqual(result.status, 0, mutation);
      assert.deepEqual(signCalls(fx), []);
    });
  }
});
