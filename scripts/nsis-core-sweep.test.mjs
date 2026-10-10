import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  NSIS_APP_CHECK,
  NSIS_SWEEP_CALL,
  NSIS_SWEEP_FORBIDDEN,
  NSIS_SWEEP_HOOKS,
  NSIS_SWEEP_MACRO,
  NSIS_SWEEP_OPERATORS,
  NSIS_SWEEP_ORDER,
  NSIS_SWEEP_PS_LINES,
  NSIS_SWEEP_PS_REQUIRED,
  NSIS_SWEEP_REQUIRED,
  NSIS_SWEEP_TAILS,
  nsisCoreSweepViolations,
  nsisMacroLines,
  renderNsisFileWrites,
  sweepPredicateLines,
  sweepScriptViolations,
  sweepVerdict,
} from './lib/nsis-core-sweep.mjs';

// 绿基线取**真文件**：夹具写得再像，也证明不了仓里那份钩子过得了判据。
// 反向对照全部是对真文件的单点变异 —— 每条变异先断言「确实改到了」，再断言判据转红。
// 行尾归一成 LF：本文件里的变异锚点按 LF 写，检出成 CRLF 时不该让锚点落空。
const REPO = join(dirname(fileURLToPath(import.meta.url)), '..');
const real = readFileSync(join(REPO, 'src-tauri', 'nsis-hooks.nsh'), 'utf8').replace(/\r\n/g, '\n');

/** 单点变异：`from` 必须恰好出现 `times` 次，否则变异没打上，测试自己先红。 */
function mutate(source, from, to, times = 1) {
  const count = source.split(from).length - 1;
  assert.equal(count, times, `变异锚点 ${JSON.stringify(from)} 应出现 ${times} 次，实为 ${count} 次`);
  const mutated = source.split(from).join(to);
  assert.notEqual(mutated, source);
  return mutated;
}

/** 只改某一个宏体内的文本（两条钩子的展开行逐字相同，全局替换会同时打中两条）。 */
function mutateInMacro(source, macro, from, to) {
  const lines = nsisMacroLines(source, macro);
  assert.ok(lines, `宏 ${macro} 不存在`);
  const body = `${lines.join('\n')}\n`;
  assert.equal(source.split(body).length - 1, 1, `宏 ${macro} 的本体在文件里应唯一`);
  // 替换值走函数：字符串形态的替换值会把宏体里的 `$$`、`$'` 当成替换模式解释掉。
  const mutated = mutate(body, from, to);
  return source.replace(body, () => mutated);
}

const block = (psLines) => renderNsisFileWrites(psLines).map((line) => `  ${line}\n`).join('');
const REAL_BLOCK = block(NSIS_SWEEP_PS_LINES);

/** 把真文件里写出脚本的那一段整体换成 `psLines` 渲染的结果（= 钩子与真值「两边一起改」）。 */
const withScript = (psLines) => mutate(real, REAL_BLOCK, block(psLines));

/** 在脚本真值里把含 `needle` 的那一行里的 `needle` 换掉；锚点必须恰好命中一行。 */
function scriptWithout(needle, replacement = 'REMOVED') {
  const hits = NSIS_SWEEP_PS_LINES.filter((line) => line.includes(needle)).length;
  assert.equal(hits, 1, `脚本真值里 ${JSON.stringify(needle)} 应恰好出现在 1 行，实为 ${hits} 行`);
  return NSIS_SWEEP_PS_LINES.map((line) => (line.includes(needle) ? line.replace(needle, replacement) : line));
}

/** 从钩子文本里取出实际写出去的脚本行（`$$` 还原成 `$`，去掉 NSIS 的行尾转义）。 */
function scriptWrittenByHook(source) {
  const macro = nsisMacroLines(source, NSIS_SWEEP_MACRO);
  assert.ok(macro, `宏 ${NSIS_SWEEP_MACRO} 不存在`);
  const PREFIX = 'FileWrite $R6 `';
  const SUFFIX = '$\\r$\\n`';
  return macro
    .map((line) => line.trim())
    .filter((line) => line.startsWith(PREFIX))
    .map((line) => {
      assert.ok(line.endsWith(SUFFIX), `FileWrite 行尾不是 CRLF 转义：${line}`);
      return line.slice(PREFIX.length, -SUFFIX.length).replaceAll('$$', '$');
    });
}

test('真文件满足判据（绿基线）', () => {
  assert.deepEqual(nsisCoreSweepViolations(real), []);
  assert.deepEqual(sweepScriptViolations(), []);
});

test('同源：钩子实际写出的脚本与脚本真值逐行相同，逐行改动或删除都必须转红', () => {
  assert.deepEqual(scriptWrittenByHook(real), NSIS_SWEEP_PS_LINES);
  assert.equal(real.split(REAL_BLOCK).length - 1, 1, '真文件里写出脚本的那一段应连续且唯一');

  const rendered = renderNsisFileWrites();
  rendered.forEach((_, index) => {
    const edited = rendered.map((line, i) => (i === index ? line.replace('$\\r$\\n`', ' $\\r$\\n`') : line));
    const dropped = rendered.filter((_line, i) => i !== index);
    for (const variant of [edited, dropped]) {
      const source = mutate(real, REAL_BLOCK, variant.map((line) => `  ${line}\n`).join(''));
      assert.ok(
        nsisCoreSweepViolations(source).some((line) => line.includes('与脚本真值不一致')),
        `脚本第 ${index + 1} 行被改动后应报「与脚本真值不一致」`
      );
    }
  });
});

test('钩子与真值两边一起改：路径判据的每个算子、进程侧的每个必需片段被去掉都必须转红', () => {
  const needles = [
    ...NSIS_SWEEP_OPERATORS.flatMap((operator) => operator.ps.map((needle) => [needle, operator.why])),
    ...NSIS_SWEEP_PS_REQUIRED,
  ];
  assert.ok(needles.length >= 18, `必需片段清单缩水了：${needles.length}`);
  for (const [needle, why] of needles) {
    const psLines = scriptWithout(needle);
    const violations = nsisCoreSweepViolations(withScript(psLines), psLines);
    assert.ok(
      violations.some((line) => line.includes(why)),
      `去掉 ${JSON.stringify(needle)} 后应报「${why}」，实为 ${JSON.stringify(violations)}`
    );
    // 两边一致 ⇒ 红的不是「不一致」那条，而是必需片段那条。
    assert.ok(!violations.some((line) => line.includes('与脚本真值不一致')));
  }
});

test('NSIS 侧的交付与执行方式逐条去掉都必须转红', () => {
  for (const [needle, why] of NSIS_SWEEP_REQUIRED) {
    const violations = nsisCoreSweepViolations(mutate(real, needle, 'Nop'));
    assert.ok(
      violations.some((line) => line.includes(why)),
      `去掉 ${JSON.stringify(needle)} 后应报「${why}」，实为 ${JSON.stringify(violations)}`
    );
  }
  // 执行参数逐个去掉。
  for (const flag of [' /TIMEOUT=30000', ' -NoProfile', ' -NonInteractive', ' -ExecutionPolicy Bypass']) {
    const violations = nsisCoreSweepViolations(mutateInMacro(real, NSIS_SWEEP_MACRO, flag, ''));
    assert.ok(violations.some((line) => line.includes('执行方式必须是')), `去掉 ${flag.trim()} 后应转红`);
  }
});

test('安装目录拼进脚本文本或命令行必须转红', () => {
  const inScript = mutateInMacro(real, NSIS_SWEEP_MACRO, '  FileClose $R6\n', '  FileWrite $R6 `# $INSTDIR$\\r$\\n`\n  FileClose $R6\n');
  assert.ok(nsisCoreSweepViolations(inScript).some((line) => line.includes('$INSTDIR 只许经 StrCpy')));
  const inCommand = mutateInMacro(real, NSIS_SWEEP_MACRO, 'polaris-stop-installed-cores.ps1"`', 'polaris-stop-installed-cores.ps1" "$INSTDIR"`');
  assert.ok(nsisCoreSweepViolations(inCommand).some((line) => line.includes('$INSTDIR 只许经 StrCpy')));
});

test('清扫失败打断安装的形态必须转红', () => {
  for (const blocking of ['Abort', 'Quit', 'MessageBox MB_OK "core still running"']) {
    const source = mutateInMacro(real, NSIS_SWEEP_MACRO, '  ${If} $R4 != 0\n', `  \${If} $R4 != 0\n    ${blocking}\n`);
    assert.ok(nsisCoreSweepViolations(source).some((line) => line.includes('不得打断安装/卸载')), blocking);
  }
});

test('按进程名结束的形态必须转红', () => {
  const byName = mutate(real, '$$proc.Kill()', '& taskkill /F /IM sing-box.exe');
  assert.ok(nsisCoreSweepViolations(byName).some((line) => line.includes('taskkill')));

  const stopProcess = mutate(real, '$$proc.Kill()', 'Stop-Process -Name sing-box -Force');
  assert.ok(nsisCoreSweepViolations(stopProcess).some((line) => line.includes('Stop-Process')));

  const pluginKill = mutate(
    real,
    NSIS_SWEEP_CALL,
    `${NSIS_SWEEP_CALL}\n  nsis_tauri_utils::KillProcess "sing-box.exe"`,
    2
  );
  assert.ok(nsisCoreSweepViolations(pluginKill).some((line) => line.includes('nsis_tauri_utils')));

  // Win32 的 TerminateProcess 与小写的 WMI `terminate`：旧判据 `\bTerminate\b` 两样都匹不到。
  for (const form of ['[Native]::TerminateProcess($$proc.Handle, 1)', 'wmic process where processid=1 call terminate']) {
    const violations = nsisCoreSweepViolations(mutate(real, '$$proc.Kill()', form));
    assert.ok(violations.some((line) => line.includes('Terminate / TerminateProcess')), `${form}: ${JSON.stringify(violations)}`);
  }
});

test('Terminate 判据收紧前后的输入对差：新判据命中旧判据命中的每一条，并补上旧判据漏掉的形态', () => {
  const OLD = /\bTerminate\b/;
  const entry = NSIS_SWEEP_FORBIDDEN.filter(([, what]) => what.includes('Terminate'));
  assert.equal(entry.length, 1, 'NSIS_SWEEP_FORBIDDEN 里 Terminate 那一条应恰好 1 条');
  const NEW = entry[0][0];
  // [输入, 旧判据, 新判据]
  const TABLE = [
    ['Invoke-CimMethod -InputObject $p -MethodName Terminate', true, true],
    ['$p.Terminate()', true, true],
    ['(Get-WmiObject Win32_Process -Filter "Name = \'sing-box.exe\'").Terminate()', true, true],
    ['$null = $p | Invoke-CimMethod -Name Terminate -Arguments @{}', true, true],
    ['[Native]::TerminateProcess($h, 1)', false, true],
    ['[Native]::NtTerminateProcess($h, 1)', false, true],
    ['[Native]::TerminateJobObject($job, 1)', false, true],
    ['wmic process where name="sing-box.exe" call terminate', false, true],
    ['$p.terminate()', false, true],
    ['$p.PSBase.InvokeMethod("TERMINATE", $null)', false, true],
    ['$proc.Kill()', false, false],
    ['if (-not $proc.WaitForExit(5000)) { $left++ }', false, false],
    ['exit 0', false, false],
  ];
  for (const [input, before, after] of TABLE) {
    assert.equal(OLD.test(input), before, `旧判据对 ${input}`);
    assert.equal(NEW.test(input), after, `新判据对 ${input}`);
    // 不比旧的弱：旧判据命中的，新判据必命中。
    if (before) assert.ok(after, input);
  }
  assert.ok(TABLE.filter(([, before, after]) => !before && after).length >= 6, '对差表里没有体现收紧');
  assert.ok(TABLE.some(([, before]) => before), '对差表里没有旧判据命中的样例，「不比旧的弱」没有被检验');
  // 真文件的代码面（含写出去的脚本）对新判据零命中；取材面自检：代码面里确实有那条结束动作。
  const code = real.split('\n').filter((line) => !/^\s*[;#]/.test(line));
  assert.ok(code.some((line) => line.includes('$$proc.Kill()')));
  assert.deepEqual(code.filter((line) => NEW.test(line)), []);
});

test('宏体内的指令顺序：InitPluginsDir → FileOpen → FileWrite… → FileClose → nsExec，反了、缺了、重了都必须转红', () => {
  assert.equal(NSIS_SWEEP_ORDER.length, 4);
  const OPEN = '  FileOpen $R6 "$PLUGINSDIR\\polaris-stop-installed-cores.ps1" w\n';
  const CLOSE = '  FileClose $R6\n';
  const INIT = '  InitPluginsDir\n';
  const lines = nsisMacroLines(real, NSIS_SWEEP_MACRO);
  const exec = `${lines.find((line) => line.trim().startsWith('nsExec::Exec '))}\n`;
  assert.ok(exec.length > 40, '取不到执行行');
  const firstWrite = `${lines.find((line) => line.trim().startsWith('FileWrite '))}\n`;
  const inMacro = (from, to) => mutateInMacro(real, NSIS_SWEEP_MACRO, from, to);

  for (const [source, why] of [
    // FileClose 挪到 nsExec 之后：执行的是仍被安装器占着的文件。
    [mutateInMacro(inMacro(CLOSE, ''), NSIS_SWEEP_MACRO, exec, `${exec}${CLOSE}`), 'FileClose 必须先于 nsExec'],
    // FileClose 整个没了。
    [inMacro(CLOSE, ''), 'FileClose 必须先于 nsExec'],
    // InitPluginsDir 挪到 FileOpen 之后。
    [mutateInMacro(inMacro(INIT, ''), NSIS_SWEEP_MACRO, OPEN, `${OPEN}${INIT}`), 'InitPluginsDir 必须先于 FileOpen'],
    // InitPluginsDir 整个没了。
    [inMacro(INIT, ''), 'InitPluginsDir 必须先于 FileOpen'],
    // FileOpen 挪到第一条 FileWrite 之后。
    [mutateInMacro(inMacro(OPEN, ''), NSIS_SWEEP_MACRO, firstWrite, `${firstWrite}${OPEN}`), 'FileOpen 必须先于全部 FileWrite'],
    // FileClose 挪到第一条 FileWrite 之后（其余 FileWrite 写进已关闭的句柄）。
    [mutateInMacro(inMacro(CLOSE, ''), NSIS_SWEEP_MACRO, firstWrite, `${firstWrite}${CLOSE}`), 'FileClose 必须排在全部 FileWrite 之后'],
    // 第二次执行、第二次打开。
    [inMacro(exec, `${exec}${exec}`), 'nsExec 应恰好 1 处'],
    [inMacro(OPEN, `${OPEN}${OPEN}`), 'FileOpen 应恰好 1 处'],
  ]) {
    const violations = nsisCoreSweepViolations(source);
    assert.ok(violations.some((line) => line.includes(why)), `应报「${why}」，实为 ${JSON.stringify(violations)}`);
  }
});

test('注释里的被禁形态不算数（判据落在代码面上）', () => {
  const commented = mutate(
    real,
    '!macro PolarisStopInstalledCores',
    '; 不要写成 taskkill /F /IM sing-box.exe 或 Stop-Process -Name sing-box\n!macro PolarisStopInstalledCores'
  );
  assert.deepEqual(nsisCoreSweepViolations(commented), []);
});

test('脚本真值里的顺序：先筛后杀、先持句柄再核启动时间再结束，反了必须转红', () => {
  const swap = (first, second) => {
    const a = NSIS_SWEEP_PS_LINES.findIndex((line) => line.includes(first));
    const b = NSIS_SWEEP_PS_LINES.findIndex((line) => line.includes(second));
    assert.ok(a >= 0 && b > a, `锚点顺序不对：${first} / ${second}`);
    const lines = [...NSIS_SWEEP_PS_LINES];
    [lines[a], lines[b]] = [lines[b], lines[a]];
    return lines;
  };
  for (const [first, second, why] of [
    ['PolarisIsInstalledCore $root $p.ExecutablePath', '$proc.Kill()', '路径判据排在结束动作之后'],
    ['$proc.StartTime - $p.CreationDate', '$proc.Kill()', '启动时间核对排在结束动作之后'],
    ['$null = $proc.Handle', '$proc.StartTime - $p.CreationDate', '启动时间在取得句柄之前核对'],
  ]) {
    const psLines = swap(first, second);
    assert.ok(nsisCoreSweepViolations(withScript(psLines), psLines).some((line) => line.includes(why)), why);
  }
});

test('脚本真值自身的约束：非 ASCII、反引号、NSIS 会展开的形态、第二个结束口、判据段碰进程', () => {
  const add = (line, at = 1) => NSIS_SWEEP_PS_LINES.toSpliced(at, 0, line);
  assert.ok(sweepScriptViolations(add('# 中文注释')).some((line) => line.includes('非 ASCII')));
  assert.ok(sweepScriptViolations(add('$x = "a`n"')).some((line) => line.includes('反引号')));
  assert.ok(sweepScriptViolations(add('$x = "${env:TEMP}"')).some((line) => line.includes('编译期常量')));
  assert.ok(sweepScriptViolations(add('$y = $(1)')).some((line) => line.includes('编译期常量')));
  const main = NSIS_SWEEP_PS_LINES.indexOf('try {');
  assert.ok(main > 0);
  assert.ok(sweepScriptViolations(add('      $proc.Kill()', main + 12)).some((line) => line.includes('结束动作应恰好 1 处')));
  assert.ok(sweepScriptViolations(add('Get-Process | Out-Null', 2)).some((line) => line.includes('不得碰进程')));
  assert.ok(sweepPredicateLines().length > 10, '判据段取空了');
});

test('枚举面放宽到全部进程必须转红', () => {
  const psLines = scriptWithout(` -Filter "Name = 'sing-box.exe'"`, '');
  assert.ok(nsisCoreSweepViolations(withScript(psLines), psLines).some((line) => line.includes('枚举面必须限定')));
});

test('两条钩子各自：没接、顺序反、先动文件，都必须转红', () => {
  for (const hook of NSIS_SWEEP_HOOKS) {
    const unwired = mutateInMacro(real, hook, `  ${NSIS_SWEEP_CALL}\n`, '');
    assert.ok(
      nsisCoreSweepViolations(unwired).some((line) => line.startsWith(`${hook}：未展开`)),
      `${hook} 去掉清扫后应转红`
    );

    const noAppCheck = mutateInMacro(real, hook, `  ${NSIS_APP_CHECK}\n`, '');
    assert.ok(
      nsisCoreSweepViolations(noAppCheck).some((line) => line.startsWith(`${hook}：清扫前必须先`)),
      `${hook} 去掉主程序检查后应转红`
    );

    const swapped = mutateInMacro(
      real,
      hook,
      `  ${NSIS_APP_CHECK}\n  ${NSIS_SWEEP_CALL}\n`,
      `  ${NSIS_SWEEP_CALL}\n  ${NSIS_APP_CHECK}\n`
    );
    assert.ok(
      nsisCoreSweepViolations(swapped).some((line) => line.startsWith(`${hook}：主程序检查排在清扫之后`)),
      `${hook} 顺序反了应转红`
    );

    const fileFirst = mutateInMacro(
      real,
      hook,
      `  ${NSIS_SWEEP_CALL}\n`,
      `  Delete "$INSTDIR\\stale.bin"\n  ${NSIS_SWEEP_CALL}\n`
    );
    assert.ok(
      nsisCoreSweepViolations(fileFirst).some((line) => line.includes('排在清扫之前')),
      `${hook} 清扫前先动文件应转红`
    );
  }
});

test('整条钩子被删或改名必须转红', () => {
  const renamed = mutate(real, '!macro NSIS_HOOK_PREUNINSTALL', '!macro NSIS_HOOK_PREUNINSTAL');
  assert.ok(nsisCoreSweepViolations(renamed).some((line) => line.startsWith('缺 NSIS_HOOK_PREUNINSTALL')));
});

test('取材面塌陷时红在取材面上，而不是恒绿', () => {
  assert.equal(nsisCoreSweepViolations('').length, 1);
  assert.match(nsisCoreSweepViolations('; only a comment\n')[0], /取材面为空/);
});

// ───────────────────────── 路径判据：样例表 ─────────────────────────

const CORE = NSIS_SWEEP_TAILS[0];
const LEGACY = NSIS_SWEEP_TAILS[1];
const HOME = String.raw`C:\Users\me\AppData\Local\Polaris`;
const APP = String.raw`C:\Apps\Polaris`;
const under = (dir, tail = CORE) => `${dir}\\${tail}`;

/**
 * [类别, 安装目录, 进程映像路径, 期望]。期望三值：'kill' 结束｜'keep' 不碰｜'no-sweep' 整条跳过。
 * 样例里的路径都不需要真实存在；带 `~` 的短名样例特意取不存在的盘符与目录 —— 真 Windows 上
 * `GetFullPath` 只展开真实存在的短名，取存在的目录会让同一条样例在不同机器上结论不同。
 */
const SAMPLES = [
  ['baseline', HOME, under(HOME), 'kill'],
  ['baseline', HOME, under(HOME, LEGACY), 'kill'],
  ['space', String.raw`C:\Program Files\Polaris`, under(String.raw`C:\Program Files\Polaris`), 'kill'],
  ['neighbour', String.raw`C:\Program Files\Polaris`, under(String.raw`C:\Program Files\Polaris Beta`), 'keep'],
  ['neighbour', String.raw`C:\Program Files\Polaris`, under(String.raw`C:\Program Files\Polaris2`), 'keep'],
  ['neighbour', APP, String.raw`C:\Apps\Polaris_up_\resources\win\sing-box.exe`, 'keep'],
  ['neighbour', String.raw`C:\Apps\Polaris Beta`, under(APP), 'keep'],
  ['single-quote', String.raw`C:\Users\O'Brien\AppData\Local\Polaris`, under(String.raw`C:\Users\O'Brien\AppData\Local\Polaris`), 'kill'],
  ['single-quote', String.raw`C:\Users\O'Brien\AppData\Local\Polaris`, under(String.raw`C:\Users\OBrien\AppData\Local\Polaris`), 'keep'],
  ['dollar', String.raw`C:\Apps\$HOME\$(calc)\Polaris`, under(String.raw`C:\Apps\$HOME\$(calc)\Polaris`), 'kill'],
  ['dollar', String.raw`C:\Apps\$HOME\Polaris`, under(APP), 'keep'],
  ['backtick', 'C:\\Apps\\Po`laris', under('C:\\Apps\\Po`laris'), 'kill'],
  ['backtick', 'C:\\Apps\\Po`laris', under(String.raw`C:\Apps\Polaris`), 'keep'],
  ['semicolon', String.raw`C:\Apps\Polaris;calc.exe`, under(String.raw`C:\Apps\Polaris;calc.exe`), 'kill'],
  ['semicolon', String.raw`C:\Apps\Polaris;calc.exe`, under(APP), 'keep'],
  ['percent', String.raw`C:\Apps\%TEMP%\Polaris`, under(String.raw`C:\Apps\%TEMP%\Polaris`), 'kill'],
  ['percent', String.raw`C:\Apps\%TEMP%\Polaris`, under(String.raw`C:\Users\me\AppData\Local\Temp\Polaris`), 'keep'],
  ['non-ascii', String.raw`C:\Users\用户\AppData\Local\Polaris`, under(String.raw`C:\Users\用户\AppData\Local\Polaris`), 'kill'],
  ['non-ascii', String.raw`C:\Users\用户\AppData\Local\Polaris`, under(String.raw`C:\Users\用戶\AppData\Local\Polaris`), 'keep'],
  ['non-ascii', String.raw`C:\ÄPPS\Polaris`, under(String.raw`c:\äpps\polaris`), 'kill'],
  ['trailing-separator', `${APP}\\`, under(APP), 'kill'],
  ['trailing-separator', `${APP}\\\\`, under(APP), 'kill'],
  ['trailing-separator', `${APP}/`, under(APP), 'kill'],
  ['volume-root', 'C:\\', String.raw`C:\_up_\resources\win\sing-box.exe`, 'no-sweep'],
  ['volume-root', 'c:/', String.raw`C:\resources\win\sing-box.exe`, 'no-sweep'],
  ['volume-root', String.raw`\\nas\apps`, String.raw`\\nas\apps\_up_\resources\win\sing-box.exe`, 'no-sweep'],
  ['volume-root', String.raw`\\nas\apps` + '\\', String.raw`\\nas\apps\_up_\resources\win\sing-box.exe`, 'no-sweep'],
  ['unc', String.raw`\\nas\apps\Polaris`, under(String.raw`\\nas\apps\Polaris`), 'kill'],
  ['unc', String.raw`\\nas\apps\Polaris`, under(String.raw`\\NAS\Apps\polaris`), 'kill'],
  ['unc', String.raw`\\nas\apps\Polaris`, under(String.raw`\\nas\apps2\Polaris`), 'keep'],
  ['unc', String.raw`\\nas\apps\Polaris`, under(String.raw`Z:\Polaris`), 'keep'],
  ['case', APP, String.raw`c:\APPS\polaris\_UP_\Resources\WIN\Sing-Box.EXE`, 'kill'],
  ['case', String.raw`c:\apps\POLARIS`, under(APP, LEGACY), 'kill'],
  ['mixed-slashes', 'C:/Apps\\Polaris/', 'C:\\Apps/Polaris/_up_/resources\\win/sing-box.exe', 'kill'],
  ['mixed-slashes', 'C:/Apps/Polaris', under(APP), 'kill'],
  ['mixed-slashes', APP, String.raw`C:\Apps\\Polaris\_up_\\resources\win\sing-box.exe`, 'kill'],
  ['dot-segments', String.raw`C:\Apps\Other\..\Polaris\.`, under(APP), 'kill'],
  ['dot-segments', APP, String.raw`C:\Apps\Polaris\..\Evil\_up_\resources\win\sing-box.exe`, 'keep'],
  ['dot-segments', APP, String.raw`C:\Apps\Polaris\_up_\resources\win\..\..\..\..\Evil\sing-box.exe`, 'keep'],
  ['extended-prefix', String.raw`\\?\C:\Apps\Polaris`, under(APP), 'kill'],
  ['extended-prefix', APP, under(String.raw`\\?\C:\Apps\Polaris`), 'kill'],
  ['extended-prefix', String.raw`\\?\UNC\nas\apps\Polaris`, under(String.raw`\\nas\apps\Polaris`), 'kill'],
  ['extended-prefix', String.raw`\\?\C:` + '\\', String.raw`C:\_up_\resources\win\sing-box.exe`, 'no-sweep'],
  // 已知限制：短名不展开（样例目录不存在）、联接点两侧的字面路径对不上 —— 结论都是「不碰」。
  ['short-name', String.raw`Q:\NOSUCH~1\Polaris`, under(String.raw`Q:\NoSuchDirectory\Polaris`), 'keep'],
  ['link', String.raw`C:\Link\Polaris`, under(String.raw`D:\Real\Polaris`), 'keep'],
  ['other-location', APP, String.raw`C:\Apps\Polaris\tools\sing-box.exe`, 'keep'],
  ['other-location', APP, String.raw`C:\Apps\Polaris\sing-box.exe`, 'keep'],
  ['other-location', APP, String.raw`C:\Apps\Polaris\_up_\resources\win\extra\sing-box.exe`, 'keep'],
  ['other-location', String.raw`D:\Tools`, String.raw`D:\Tools\OtherVPN\sing-box.exe`, 'keep'],
  ['other-location', String.raw`D:\Tools`, String.raw`D:\Tools\OtherVPN\resources\win\sing-box.exe`, 'keep'],
  ['other-name', APP, String.raw`C:\Apps\Polaris\_up_\resources\win\polaris-helper.exe`, 'keep'],
  ['other-name', APP, String.raw`C:\Apps\Polaris\_up_\resources\win\sing-box.exe.bak`, 'keep'],
  ['unrelated', APP, String.raw`C:\Users\me\Downloads\sing-box.exe`, 'keep'],
  ['unrelated', APP, String.raw`C:\ProgramData\Polaris\core\sing-box.exe`, 'keep'],
  ['unrelated', APP, under(String.raw`D:\Apps\Polaris`), 'keep'],
];

const REQUIRED_CATEGORIES = [
  'baseline', 'space', 'neighbour', 'single-quote', 'dollar', 'backtick', 'semicolon', 'percent', 'non-ascii',
  'trailing-separator', 'volume-root', 'unc', 'case', 'mixed-slashes', 'dot-segments', 'extended-prefix',
  'short-name', 'link', 'other-location', 'other-name', 'unrelated',
];

test('样例表覆盖全部输入类别，且每类特殊字符都同时有「结束」与「不碰」两种结论', () => {
  const seen = new Set(SAMPLES.map(([category]) => category));
  assert.deepEqual(REQUIRED_CATEGORIES.filter((category) => !seen.has(category)), []);
  for (const category of ['single-quote', 'dollar', 'backtick', 'semicolon', 'percent', 'non-ascii', 'unc']) {
    const verdicts = new Set(SAMPLES.filter(([c]) => c === category).map((sample) => sample[3]));
    assert.ok(verdicts.has('kill') && verdicts.has('keep'), `${category} 只有单向样例`);
  }
  assert.ok(SAMPLES.some((sample) => sample[3] === 'no-sweep'));
});

test('JS 等价实现对样例表逐条给出期望结论', () => {
  for (const [category, instdir, image, expected] of SAMPLES) {
    assert.equal(sweepVerdict(instdir, image), expected, `[${category}] ${instdir} ← ${image}`);
  }
});

test('算子逐一对应：每个算子的片段都在钩子文本里，且关掉它之后样例表至少有一条结论会变', () => {
  const written = scriptWrittenByHook(real).join('\n');
  assert.equal(NSIS_SWEEP_OPERATORS.length, 7);
  for (const operator of NSIS_SWEEP_OPERATORS) {
    for (const needle of operator.ps) {
      assert.ok(written.includes(needle), `钩子文本里没有算子「${operator.id}」的片段 ${JSON.stringify(needle)}`);
    }
    const off = new Set([operator.id]);
    const flipped = SAMPLES.filter(([, instdir, image, expected]) => sweepVerdict(instdir, image, off) !== expected);
    assert.ok(flipped.length > 0, `关掉算子「${operator.id}」后样例表全无变化 —— JS 侧没实现它，或样例没盯着它`);
  }
});

test('余下部分的允许表与安装包里内核的实际位置一致', () => {
  const conf = JSON.parse(readFileSync(join(REPO, 'src-tauri', 'tauri.windows.conf.json'), 'utf8'));
  // Tauri 把资源条目里的 `../` 段落成安装目录下的 `_up_`。
  assert.ok(conf.bundle.resources.includes('../resources/win/'), 'Windows 包的内核资源条目变了，允许表要跟着改');
  assert.deepEqual(NSIS_SWEEP_TAILS, ['_up_\\resources\\win\\sing-box.exe', 'resources\\win\\sing-box.exe']);
  // 第二项是旧版裸布局：安装钩子清的就是这棵目录，活着的旧内核会占住它。
  assert.ok(nsisMacroLines(real, 'NSIS_HOOK_PREINSTALL').some((line) => line.trim() === 'RMDir /r "$INSTDIR\\resources"'));
});

// 钩子文本里的判据函数在真 PowerShell 上对同一张样例表逐条执行。只在 Windows 上有意义：
// `GetFullPath` 的 Windows 路径语义（反斜杠是分隔符、盘符、UNC）在别的系统的 .NET 上不存在。
// Windows 上用的是钩子实际会拉起的 Windows PowerShell（`powershell.exe`），找不到它是红，不是跳过。
const onWindows = process.platform === 'win32';
test(
  '真执行：钩子文本里的路径判据函数在 Windows PowerShell 上对样例表逐条给出期望结论',
  { skip: onWindows ? false : `非 Windows（${process.platform}）：钩子文本的真执行只在有 Windows PowerShell 的 runner 上发生` },
  () => {
    const predicate = scriptWrittenByHook(real).slice(0, sweepPredicateLines().length);
    assert.deepEqual(predicate, sweepPredicateLines());
    const driver = [
      '$samples = Get-Content -LiteralPath $env:POLARIS_SWEEP_SAMPLES -Raw -Encoding UTF8 | ConvertFrom-Json',
      'foreach ($s in $samples) {',
      "  try { $r = PolarisSweepRoot $s.instdir } catch { Write-Output 'error'; continue }",
      "  if (-not $r) { Write-Output 'no-sweep'; continue }",
      "  try { if (PolarisIsInstalledCore $r $s.image) { Write-Output 'kill' } else { Write-Output 'keep' } } catch { Write-Output 'keep' }",
      '}',
    ];
    const dir = mkdtempSync(join(tmpdir(), 'polaris-nsis-sweep-'));
    try {
      const script = join(dir, 'predicate.ps1');
      const samples = join(dir, 'samples.json');
      writeFileSync(script, `${[...predicate, ...driver].join('\r\n')}\r\n`, 'ascii');
      writeFileSync(samples, JSON.stringify(SAMPLES.map(([, instdir, image]) => ({ instdir, image }))), 'utf8');
      const result = spawnSync(
        'powershell.exe',
        ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', script],
        { encoding: 'utf8', env: { ...process.env, POLARIS_SWEEP_SAMPLES: samples } }
      );
      assert.equal(result.error, undefined, `拉不起 powershell.exe：${result.error}`);
      assert.equal(result.status, 0, result.stderr);
      const verdicts = result.stdout.split(/\r?\n/).filter(Boolean);
      assert.equal(verdicts.length, SAMPLES.length, result.stdout);
      SAMPLES.forEach(([category, instdir, image, expected], index) => {
        assert.equal(verdicts[index], expected, `[${category}] ${instdir} ← ${image}`);
      });
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }
);
