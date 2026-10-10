// NSIS 钩子「结束本安装目录内的内核进程」：脚本真值、等价实现与静态判据。
//
// 守的是四件事，任何一件坏掉在构建期与安装器产物里都**看不出来**（NSIS 宏体是纯文本展开，
// 写进去的 PowerShell 更是到用户机器上才第一次被解释）：
//
//   1. 判据是**完整映像路径**，不是进程名 —— 用户可能自己另跑着一份 sing-box；
//   2. 顺序是「主程序先退出 → 再结束内核 → 最后才动文件」—— 主程序还活着时结束内核，
//      崩溃自恢复会把它重新拉起来，等于没结束；
//   3. 安装与卸载两条腿都接了 —— 只接一条时另一条照旧留半截；
//   4. 钩子里实际写出去的 PowerShell 与本文件持有的那一份**逐字相同** —— 样例表断言的是本文件的
//      等价实现，两边不同源的话样例全绿也说明不了钩子。
//
// 同源的做法：PowerShell 脚本的唯一真值是下面的 `NSIS_SWEEP_PS_LINES`；`nsis-hooks.nsh` 里那串
// `FileWrite` 必须与 `renderNsisFileWrites()` 的输出逐行相同。路径判据另有一份 JS 等价实现
// （`sweepRootOf` / `isInstalledCore`），算子与 PowerShell 侧逐一对应（`NSIS_SWEEP_OPERATORS`），
// 由 `nsis-core-sweep.test.mjs` 的样例表驱动；同一张表在 Windows 上还会喂给钩子文本本身真执行。
//
// 由 `verify-packaging.mjs confs` 对真文件调用；`nsis-core-sweep.test.mjs` 拿真文件做绿基线、
// 逐条变异做反向对照。

import { win32 } from 'node:path';

export const NSIS_APP_CHECK =
  '!insertmacro CheckIfAppIsRunning "$INSTDIR\\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"';
export const NSIS_SWEEP_MACRO = 'PolarisStopInstalledCores';
export const NSIS_SWEEP_CALL = `!insertmacro ${NSIS_SWEEP_MACRO}`;

/** 两条必须先结束内核再动文件的钩子。 */
export const NSIS_SWEEP_HOOKS = ['NSIS_HOOK_PREINSTALL', 'NSIS_HOOK_PREUNINSTALL'];

/**
 * 内核在安装目录下只可能运行于这两个相对位置（真值：`runtime/proxy/core_binary.rs` 的候选表在
 * Windows 上落到 `<exe 目录>\_up_\resources\win\` 与 `<exe 目录>\resources\win\`）。
 * 判据是「前缀 = 安装目录」**且**「余下部分恰为其中之一」：只用前缀的话，用户把应用装进一个公共
 * 父目录（如直接装到 `D:\Tools`）时，同目录下别的软件自带的 sing-box 也会落进射程。
 */
export const NSIS_SWEEP_TAILS = ['_up_\\resources\\win\\sing-box.exe', 'resources\\win\\sing-box.exe'];

/**
 * 钩子在目标机器上写出并执行的 PowerShell 脚本，一行一项（真值；不含行尾）。
 *
 * 约束：只能是可打印 ASCII（NSIS 的 `FileWrite` 按 ANSI 代码页落盘，非 ASCII 会随机器代码页变形），
 * 不能含反引号（NSIS 侧用反引号做字符串定界），渲染后单行不超过 NSIS 的 1024 字符串上限。
 * 前三个函数是纯路径判据，不碰进程；测试靠「第一行 `try {` 之前」切出它们单独执行。
 */
export const NSIS_SWEEP_PS_LINES = [
  String.raw`$ErrorActionPreference = 'Stop'`,
  String.raw`$sep = '\'`,
  String.raw`$ext = '\\?\'`,
  `$tails = @(${NSIS_SWEEP_TAILS.map((tail) => `'${tail}'`).join(', ')})`,
  String.raw`function PolarisFullPath([string]$p) {`,
  String.raw`  if ($p.StartsWith($ext + 'UNC' + $sep, [System.StringComparison]::OrdinalIgnoreCase)) { $p = $sep + $sep + $p.Substring(8) } elseif ($p.StartsWith($ext, [System.StringComparison]::Ordinal)) { $p = $p.Substring(4) }`,
  String.raw`  return [System.IO.Path]::GetFullPath($p)`,
  String.raw`}`,
  String.raw`function PolarisSweepRoot([string]$dir) {`,
  String.raw`  $full = (PolarisFullPath $dir).TrimEnd($sep)`,
  String.raw`  if ($full -eq [System.IO.Path]::GetPathRoot($full + $sep).TrimEnd($sep)) { return $null }`,
  String.raw`  return $full + $sep`,
  String.raw`}`,
  String.raw`function PolarisIsInstalledCore([string]$root, [string]$image) {`,
  String.raw`  $full = PolarisFullPath $image`,
  String.raw`  if (-not $full.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) { return $false }`,
  String.raw`  $tail = $full.Substring($root.Length)`,
  String.raw`  foreach ($t in $tails) { if ([string]::Equals($tail, $t, [System.StringComparison]::OrdinalIgnoreCase)) { return $true } }`,
  String.raw`  return $false`,
  String.raw`}`,
  String.raw`try {`,
  String.raw`  $root = PolarisSweepRoot $env:POLARIS_SWEEP_ROOT`,
  String.raw`  if (-not $root) { exit 3 }`,
  String.raw`  $left = 0`,
  String.raw`  foreach ($p in @(Get-CimInstance -ClassName Win32_Process -Filter "Name = 'sing-box.exe'")) {`,
  String.raw`    try {`,
  String.raw`      if (-not $p.ExecutablePath) { continue }`,
  String.raw`      if (-not (PolarisIsInstalledCore $root $p.ExecutablePath)) { continue }`,
  String.raw`    } catch { continue }`,
  String.raw`    try {`,
  String.raw`      $proc = [System.Diagnostics.Process]::GetProcessById([int]$p.ProcessId)`,
  String.raw`      $null = $proc.Handle`,
  String.raw`      if ([Math]::Abs(($proc.StartTime - $p.CreationDate).TotalSeconds) -gt 2) { continue }`,
  String.raw`      $proc.Kill()`,
  String.raw`      if (-not $proc.WaitForExit(5000)) { $left++ }`,
  String.raw`    } catch {`,
  String.raw`      if (Get-Process -Id $p.ProcessId -ErrorAction SilentlyContinue) { $left++ }`,
  String.raw`    }`,
  String.raw`  }`,
  String.raw`  if ($left -gt 0) { exit 2 }`,
  String.raw`  exit 0`,
  String.raw`} catch { exit 1 }`,
];

/** 纯路径判据那一段（三个函数与它们用到的常量）：真值里第一行 `try {` 之前的全部行。 */
export function sweepPredicateLines(psLines = NSIS_SWEEP_PS_LINES) {
  const main = psLines.indexOf('try {');
  return main < 0 ? [] : psLines.slice(0, main);
}

/** 把脚本行渲染成 NSIS 源里的 `FileWrite` 行（`$` 写成 `$$`，行尾 CRLF）。 */
export function renderNsisFileWrites(psLines = NSIS_SWEEP_PS_LINES) {
  return psLines.map((line) => `FileWrite $R6 \`${line.replaceAll('$', '$$$$')}$\\r$\\n\``);
}

/**
 * 路径判据的算子表：每个算子在 PowerShell 真值里必须逐字出现的片段（`ps`），以及 JS 等价实现里
 * 同名的开关（把 `id` 放进 `off` 即关掉该算子）。测试对每个算子断言两件事：片段在钩子文本里；
 * 关掉它之后样例表里至少有一条的结论会变 —— 即 JS 这边确实实现了它，样例也确实盯着它。
 */
export const NSIS_SWEEP_OPERATORS = [
  {
    id: 'strip-extended-prefix',
    ps: [String.raw`{ $p = $sep + $sep + $p.Substring(8) } elseif ($p.StartsWith($ext, [System.StringComparison]::Ordinal)) { $p = $p.Substring(4) }`],
    why: String.raw`规范化前先去掉 \\?\ 与 \\?\UNC\ 前缀`,
  },
  {
    id: 'full-path',
    ps: ['return [System.IO.Path]::GetFullPath($p)'],
    why: '路径必须规范化（斜杠方向、重复分隔符、. 与 .. 段）',
  },
  {
    id: 'trim-trailing-separators',
    ps: ['$full = (PolarisFullPath $dir).TrimEnd($sep)'],
    why: '安装目录尾部的分隔符必须先去掉',
  },
  {
    id: 'separator-after-prefix',
    ps: ['return $full + $sep'],
    why: '前缀后必须紧跟一个分隔符（否则 `…\\Polaris` 会匹配到 `…\\Polaris2\\`）',
  },
  {
    id: 'volume-root-skip',
    ps: ['if ($full -eq [System.IO.Path]::GetPathRoot($full + $sep).TrimEnd($sep)) { return $null }', 'if (-not $root) { exit 3 }'],
    why: '安装目录是卷根（盘符根或 UNC 共享根）时整条跳过',
  },
  {
    id: 'ignore-case',
    ps: [
      'if (-not $full.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) { return $false }',
      '[string]::Equals($tail, $t, [System.StringComparison]::OrdinalIgnoreCase)',
    ],
    why: '前缀与余下部分的比较都不分大小写',
  },
  {
    id: 'tail-allowlist',
    ps: ['$tail = $full.Substring($root.Length)', 'foreach ($t in $tails) { if ([string]::Equals($tail, $t,'],
    why: '前缀之后余下的部分必须恰为内核的已知相对位置之一',
  },
];

/** 脚本真值里除路径判据外还必须逐字出现的片段（进程侧）。 */
export const NSIS_SWEEP_PS_REQUIRED = [
  ['$root = PolarisSweepRoot $env:POLARIS_SWEEP_ROOT', '清扫根必须取自环境变量并经判据函数规范化'],
  ['Get-CimInstance -ClassName Win32_Process', '映像路径必须经 WMI 取（32 位 PowerShell 读不了 64 位进程的模块表）'],
  [`-Filter "Name = 'sing-box.exe'"`, '枚举面必须限定为映像名 sing-box.exe'],
  ['if (-not $p.ExecutablePath) { continue }', '取不到映像路径的进程必须跳过'],
  ['if (-not (PolarisIsInstalledCore $root $p.ExecutablePath)) { continue }', '只结束通过路径判据的进程'],
  ['$null = $proc.Handle', '结束前必须先持有进程句柄（此后 PID 不会被复用）'],
  [
    'if ([Math]::Abs(($proc.StartTime - $p.CreationDate).TotalSeconds) -gt 2) { continue }',
    '结束前必须核对启动时间（枚举到取得句柄之间 PID 可能已被复用）',
  ],
  ['$proc.Kill()', '缺结束动作'],
  ['$proc.WaitForExit(5000)', '结束后必须等进程真的退出（映像占用在退出后才释放）'],
  ['} catch { exit 1 }', '任何未预料的错误都必须以非零退出码收场，而不是抛给安装器'],
];

/** 这些片段在脚本真值里的先后顺序不可换（前者必须先于后者）。 */
const PS_ORDER = [
  ['PolarisIsInstalledCore $root $p.ExecutablePath', '$proc.Kill()', '路径判据排在结束动作之后'],
  ['$null = $proc.Handle', '$proc.StartTime - $p.CreationDate', '启动时间在取得句柄之前核对（核对与结束之间又留出了 PID 复用窗口）'],
  ['$proc.StartTime - $p.CreationDate', '$proc.Kill()', '启动时间核对排在结束动作之后'],
];

/** 宏体里除 `FileWrite` 外必须逐字出现的 NSIS 行（整行相等）。 */
export const NSIS_SWEEP_REQUIRED = [
  ['StrCpy $R9 "$INSTDIR"', '清扫根必须取自 $INSTDIR'],
  [
    `System::Call 'kernel32::SetEnvironmentVariable(t "POLARIS_SWEEP_ROOT", t R9)'`,
    '清扫根必须经环境变量交给脚本（不进脚本文本与命令行）',
  ],
  ['FileOpen $R6 "$PLUGINSDIR\\polaris-stop-installed-cores.ps1" w', '脚本必须写进安装器的私有临时目录'],
  [
    'nsExec::Exec /TIMEOUT=30000 `"$SYSDIR\\WindowsPowerShell\\v1.0\\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\\polaris-stop-installed-cores.ps1"`',
    '执行方式必须是：System32 绝对路径的 PowerShell、无窗口（nsExec）、30 秒上界、不读 profile、不等输入、带 Bypass',
  ],
];

/** 按进程名结束、或绕开路径判据的形态。出现在代码面任何位置都算越界。 */
export const NSIS_SWEEP_FORBIDDEN = [
  [/taskkill/i, 'taskkill（按映像名或 PID 树结束，无路径判据）'],
  [/Stop-Process/i, 'Stop-Process（脱离上面那条路径判据的第二个结束口）'],
  [/nsis_tauri_utils::Kill/i, 'nsis_tauri_utils::KillProcess*（按进程名结束）'],
  // 不加词边界、不分大小写：`\bTerminate\b` 匹不到 `TerminateProcess` / `NtTerminateProcess`，
  // 也匹不到 `wmic process … call terminate`。
  [/terminate/i, 'Terminate / TerminateProcess（第二个结束口）'],
];

/**
 * 宏体内这些指令的先后顺序不可换（前者的最后一次出现必须先于后者的第一次出现），且两端都必须存在。
 * 顺序错了脚本文本一个字都不差、同源判据照样绿：`FileOpen` 排在 `InitPluginsDir` 之前时
 * `$PLUGINSDIR` 还是空串，脚本落到别处；`nsExec` 排在 `FileClose` 之前时执行的是没写完、仍被
 * 安装器占着的文件。
 */
export const NSIS_SWEEP_ORDER = [
  [/^InitPluginsDir$/, /^FileOpen\b/, 'InitPluginsDir 必须先于 FileOpen（否则 $PLUGINSDIR 还没有建立）'],
  [/^FileOpen\b/, /^FileWrite\b/, 'FileOpen 必须先于全部 FileWrite'],
  [/^FileWrite\b/, /^FileClose\b/, 'FileClose 必须排在全部 FileWrite 之后'],
  [/^FileClose\b/, /^nsExec::/, 'FileClose 必须先于 nsExec（否则执行的是没写完、仍被占用的脚本）'],
];

/** 宏体内出现即意味着「清扫失败会打断安装/卸载」的指令。 */
const BLOCKING = /^(Abort|Quit|MessageBox)\b/;

// ───────────────────────── 路径判据的 JS 等价实现 ─────────────────────────

const SEP = '\\';
const NONE = new Set();

/** 逐 UTF-16 单元、按单字符大写映射比较（.NET 的 OrdinalIgnoreCase 口径）。 */
function sameUnit(a, b, ignoreCase) {
  if (a === b) return true;
  if (!ignoreCase) return false;
  const ua = a.toUpperCase();
  const ub = b.toUpperCase();
  return ua.length === 1 && ub.length === 1 && ua === ub;
}
function equalsOrdinal(a, b, ignoreCase) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i += 1) if (!sameUnit(a[i], b[i], ignoreCase)) return false;
  return true;
}
const startsWithOrdinal = (text, prefix, ignoreCase) =>
  text.length >= prefix.length && equalsOrdinal(text.slice(0, prefix.length), prefix, ignoreCase);
const trimEndSep = (text) => text.replace(/\\+$/, '');

/**
 * `PolarisFullPath` 的等价实现。规范化交给 `node:path` 的 win32 实现（任何宿主上都是 Windows 语义），
 * 再补上 `GetFullPath` 保留尾部分隔符这一点。只接受绝对路径：相对路径在 .NET 里按进程当前目录
 * 解析，这里没有可对拍的「当前目录」。
 * 不模拟的 Windows 行为：8.3 短名展开、段尾空格与句点的裁剪 —— 这两样只有真 Windows 上才有。
 */
function fullPath(input, off) {
  let p = input;
  if (!off.has('strip-extended-prefix')) {
    if (startsWithOrdinal(p, '\\\\?\\UNC\\', true)) p = SEP + SEP + p.slice(8);
    else if (p.startsWith('\\\\?\\')) p = p.slice(4);
  }
  if (off.has('full-path')) return p;
  if (!win32.isAbsolute(p)) throw new Error(`不是绝对路径：${input}`);
  const resolved = win32.resolve(p);
  return /[\\/]$/.test(p) && !resolved.endsWith(SEP) ? resolved + SEP : resolved;
}

/**
 * `PolarisSweepRoot` 的等价实现。
 * @returns {string | null} 以单个分隔符结尾的清扫根；安装目录是卷根时为 null（= 钩子里的 exit 3）
 */
export function sweepRootOf(instdir, off = NONE) {
  let full = fullPath(instdir, off);
  if (!off.has('trim-trailing-separators')) full = trimEndSep(full);
  if (!off.has('volume-root-skip')) {
    const volume = trimEndSep(win32.parse(full + SEP).root);
    if (equalsOrdinal(trimEndSep(full), volume, true)) return null;
  }
  return off.has('separator-after-prefix') ? full : full + SEP;
}

/** `PolarisIsInstalledCore` 的等价实现。 */
export function isInstalledCore(root, image, off = NONE) {
  const ignoreCase = !off.has('ignore-case');
  const full = fullPath(image, off);
  if (!startsWithOrdinal(full, root, ignoreCase)) return false;
  if (off.has('tail-allowlist')) return true;
  const tail = full.slice(root.length);
  return NSIS_SWEEP_TAILS.some((known) => equalsOrdinal(tail, known, ignoreCase));
}

/**
 * 一对（安装目录，进程映像路径）的结论，三值与钩子的行为一一对应：
 *   'no-sweep' 整条跳过（exit 3）｜'kill' 结束该进程｜'keep' 不碰它。
 * 路径无法规范化时按钩子的处理归入 'keep'（单个进程路径出错只跳过它自己）。
 */
export function sweepVerdict(instdir, image, off = NONE) {
  const root = sweepRootOf(instdir, off);
  if (root === null) return 'no-sweep';
  try {
    return isInstalledCore(root, image, off) ? 'kill' : 'keep';
  } catch {
    return 'keep';
  }
}

// ───────────────────────── 静态判据 ─────────────────────────

const isComment = (line) => /^\s*[;#]/.test(line);

/** 取一个 NSIS 宏的本体行（保留注释行）；缺失或未闭合返回 null。 */
export function nsisMacroLines(source, name) {
  const lines = source.split('\n');
  const start = lines.findIndex((line) => new RegExp(`^!macro ${name}(?:\\s|$)`).test(line.trim()));
  if (start < 0) return null;
  const endOffset = lines.slice(start + 1).findIndex((line) => line.trim() === '!macroend');
  return endOffset < 0 ? null : lines.slice(start + 1, start + 1 + endOffset);
}

/** 代码面：去掉整行注释与空行，逐行去首尾空白。 */
export const nsisCodeLines = (lines) => lines.filter((line) => !isComment(line)).map((line) => line.trim()).filter(Boolean);
const codeOf = nsisCodeLines;

/** 钩子里「动安装目录文件」的指令：出现在清扫之前即顺序错误。 */
const TOUCHES_FILES = /^(RMDir|Delete|File|Rename|CopyFiles)\b/;

/** 脚本真值自身的约束（与钩子文件无关）。 */
export function sweepScriptViolations(psLines = NSIS_SWEEP_PS_LINES) {
  const out = [];
  const text = psLines.join('\n');
  psLines.forEach((line, index) => {
    if (!/^[\x20-\x7e]*$/.test(line)) out.push(`脚本第 ${index + 1} 行含非 ASCII 或控制字符（FileWrite 按 ANSI 代码页落盘）`);
    if (line.includes('`')) out.push(`脚本第 ${index + 1} 行含反引号（NSIS 侧用它做字符串定界）`);
    if (/\$[{(]/.test(line)) out.push(`脚本第 ${index + 1} 行含 \${ 或 \$(（渲染进 NSIS 源后会被当成编译期常量或语言串引用）`);
    if (line.replaceAll('$', '$$$$').length > 900) out.push(`脚本第 ${index + 1} 行渲染后过长（NSIS 字符串上限 1024）`);
  });
  for (const operator of NSIS_SWEEP_OPERATORS) {
    for (const needle of operator.ps) {
      if (!text.includes(needle)) out.push(`脚本真值：${operator.why} —— 缺 ${JSON.stringify(needle)}`);
    }
  }
  for (const [needle, why] of NSIS_SWEEP_PS_REQUIRED) {
    if (!text.includes(needle)) out.push(`脚本真值：${why} —— 缺 ${JSON.stringify(needle)}`);
  }
  for (const [first, second, why] of PS_ORDER) {
    const a = psLines.findIndex((line) => line.includes(first));
    const b = psLines.findIndex((line) => line.includes(second));
    if (a >= 0 && b >= 0 && a >= b) out.push(`脚本真值：${why}`);
  }
  const kills = psLines.filter((line) => /\.Kill\(/.test(line)).length;
  if (kills !== 1) out.push(`脚本真值：结束动作应恰好 1 处，实为 ${kills} 处`);
  if (sweepPredicateLines(psLines).some((line) => /Kill|Get-CimInstance|Get-Process|GetProcessById/.test(line))) {
    out.push('脚本真值：路径判据那一段（第一行 `try {` 之前）不得碰进程 —— 测试会单独执行这一段');
  }
  return out;
}

/**
 * @param {string} rawSource `nsis-hooks.nsh` 全文
 * @param {string[]} psLines 脚本真值（缺省即本文件导出的那一份；测试用它做「两边一起改」的变异）
 * @returns {string[]} 违规清单；空 = 判据成立
 */
export function nsisCoreSweepViolations(rawSource, psLines = NSIS_SWEEP_PS_LINES) {
  const source = rawSource.replace(/\r\n/g, '\n');
  const out = [];
  const allCode = codeOf(source.split('\n'));

  // ── 取材面自检：剥注释不能把代码剥没。
  if (!allCode.some((line) => line.startsWith('!macro '))) {
    return ['取材面为空：代码面里一个 `!macro` 都没有（文件不对，或注释剥离把代码也剥掉了）'];
  }

  out.push(...sweepScriptViolations(psLines));

  // ── 宏体：写出去的脚本与真值逐行相同，外加 NSIS 侧的交付与执行方式。
  const macro = nsisMacroLines(source, NSIS_SWEEP_MACRO);
  if (macro === null) {
    out.push(`缺宏 ${NSIS_SWEEP_MACRO}（或未闭合）`);
  } else {
    const body = codeOf(macro);
    const written = body.filter((line) => line.startsWith('FileWrite '));
    const expected = renderNsisFileWrites(psLines);
    if (written.length !== expected.length) {
      out.push(`${NSIS_SWEEP_MACRO}：写出的脚本应为 ${expected.length} 行，实为 ${written.length} 行 —— 与脚本真值不一致`);
    }
    for (let i = 0; i < Math.min(written.length, expected.length); i += 1) {
      if (written[i] !== expected[i]) {
        out.push(
          `${NSIS_SWEEP_MACRO}：写出的脚本第 ${i + 1} 行与脚本真值不一致 —— 应为 ${JSON.stringify(expected[i])}，实为 ${JSON.stringify(written[i])}`
        );
        break;
      }
    }
    for (const [needle, why] of NSIS_SWEEP_REQUIRED) {
      if (!body.includes(needle)) out.push(`${NSIS_SWEEP_MACRO}：${why} —— 缺 ${JSON.stringify(needle)}`);
    }
    // 安装目录只许以「拷进寄存器」这一种形态出现：拼进脚本文本或命令行就把路径里的引号、`$`、
    // 分号交给了 PowerShell 或命令行解析。
    const instdirUses = body.filter((line) => line.includes('$INSTDIR'));
    if (instdirUses.some((line) => line !== 'StrCpy $R9 "$INSTDIR"')) {
      out.push(`${NSIS_SWEEP_MACRO}：$INSTDIR 只许经 StrCpy 进寄存器再走环境变量，实为 ${JSON.stringify(instdirUses)}`);
    }
    const blocking = body.find((line) => BLOCKING.test(line));
    if (blocking) out.push(`${NSIS_SWEEP_MACRO}：清扫失败不得打断安装/卸载 —— 出现 ${JSON.stringify(blocking)}`);
    for (const [first, second, why] of NSIS_SWEEP_ORDER) {
      const lastFirst = body.findLastIndex((line) => first.test(line));
      const firstSecond = body.findIndex((line) => second.test(line));
      if (lastFirst < 0 || firstSecond < 0 || lastFirst > firstSecond) out.push(`${NSIS_SWEEP_MACRO}：${why}`);
    }
    for (const [pattern, what] of [[/^FileOpen\b/, 'FileOpen'], [/^FileClose\b/, 'FileClose'], [/^nsExec::/, 'nsExec']]) {
      const count = body.filter((line) => pattern.test(line)).length;
      if (count !== 1) out.push(`${NSIS_SWEEP_MACRO}：${what} 应恰好 1 处，实为 ${count} 处`);
    }
  }

  // ── 全文件代码面：不得有第二个结束口，也不得有按名字结束的形态。
  const killsEverywhere = allCode.filter((line) => /\.Kill\(/.test(line)).length;
  if (killsEverywhere > 1) out.push(`代码面里 .Kill( 共 ${killsEverywhere} 处，只允许 ${NSIS_SWEEP_MACRO} 里那 1 处`);
  for (const [pattern, what] of NSIS_SWEEP_FORBIDDEN) {
    const hit = allCode.find((line) => pattern.test(line));
    if (hit) out.push(`代码面出现 ${what}：${hit}`);
  }

  // ── 两条钩子：接线与顺序。
  for (const hook of NSIS_SWEEP_HOOKS) {
    const lines = nsisMacroLines(source, hook);
    if (lines === null) {
      out.push(`缺 ${hook}（该路径上仍在运行的内核会占住安装目录里的文件）`);
      continue;
    }
    const body = codeOf(lines);
    const app = body.indexOf(NSIS_APP_CHECK);
    const sweep = body.indexOf(NSIS_SWEEP_CALL);
    if (sweep < 0) {
      out.push(`${hook}：未展开 ${NSIS_SWEEP_MACRO}`);
      continue;
    }
    if (app < 0) {
      out.push(`${hook}：清扫前必须先逐字展开模板的主程序检查 ${JSON.stringify(NSIS_APP_CHECK)}`);
    } else if (app > sweep) {
      out.push(`${hook}：主程序检查排在清扫之后（主程序还活着时结束内核，崩溃自恢复会把它重新拉起来）`);
    }
    const firstFileOp = body.findIndex((line) => TOUCHES_FILES.test(line));
    if (firstFileOp >= 0 && firstFileOp < sweep) {
      out.push(`${hook}：${JSON.stringify(body[firstFileOp])} 排在清扫之前（动文件时内核还占着它）`);
    }
  }
  return out;
}
