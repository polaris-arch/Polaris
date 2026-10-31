// Native producer contract for both NSIS PRE consumers. JS is the offline
// path oracle; actual Windows predicates/handles execute in the Rust fixture.
import { win32 } from 'node:path';
export const NSIS_APP_CHECK = '!insertmacro CheckIfAppIsRunning "$INSTDIR\\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"';
export const NSIS_SWEEP_MACRO = 'PolarisStopInstalledCores';
export const NSIS_SWEEP_CALL = `!insertmacro ${NSIS_SWEEP_MACRO}`;
export const NSIS_SWEEP_HOOKS = ['NSIS_HOOK_PREINSTALL', 'NSIS_HOOK_PREUNINSTALL'];
export const NSIS_SWEEP_TAILS = ['_up_\\resources\\win\\sing-box.exe', 'resources\\win\\sing-box.exe'];
export const NSIS_SWEEP_LINES = [
  'InitPluginsDir', 'StrCpy $R6 $OUTDIR', 'SetOutPath "$PLUGINSDIR"',
  'File /oname=polaris-cleaner.exe "${POLARIS_NATIVE_CLEANER_PAYLOAD}"',
  'SetOutPath $R6',
  `nsExec::Exec /TIMEOUT=30000 '"$PLUGINSDIR\\polaris-cleaner.exe" --sweep-installed-cores'`,
];
export const NSIS_SWEEP_OPERATORS = [
  ['strip-extended-prefix', 'path.starts_with(&ext)'],
  ['full-path', 'GetFullPathNameW'],
  ['trim-trailing-separators', 'while full.last() == Some(&92)'],
  ['separator-after-prefix', 'full.push(92)'],
  ['volume-root-skip', 'return Ok(None)'],
  ['ignore-case', 'CompareStringOrdinal'],
  ['tail-allowlist', 'TAILS.iter()'],
].map(([id, rust]) => ({ id, rust }));
export function nsisMacroLines(source,name) {
  const lines=source.replace(/\r\n/g,'\n').split('\n');
  const start=lines.findIndex(line=>new RegExp(`^!macro ${name}(?:\\s|$)`).test(line.trim()));
  if(start<0)return null;
  const end=lines.slice(start+1).findIndex(line=>line.trim()==='!macroend');
  return end<0?null:lines.slice(start+1,start+1+end);
}
export const nsisCodeLines=lines=>lines.map(line=>line.trim()).filter(line=>line&&!line.startsWith(';')&&!line.startsWith('#'));
export function nativeSweepViolations(source) {
  const code=source.replace(/\/\/[^\n]*/g,'').replace(/\/\*[\s\S]*?\*\//g,'');
  const compact=code.replace(/\s+/g,'');
  const required=[...NSIS_SWEEP_OPERATORS.map(row=>row.rust),
    'PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE | PROCESS_TERMINATE',
    'born == observed.birth', 'installed(root, &image)', 'process_image(process.0)',
    'GetProcessTimes', 'TerminateProcess(process.0, 1)', 'WaitForSingleObject(process.0, 5000)',
    'CreateToolhelp32Snapshot', 'sing-box.exe', ...NSIS_SWEEP_TAILS];
  const failures=required.filter(needle=>!compact.includes(needle.replace(/\s+/g,''))).map(needle=>`missing native sweep contract: ${needle}`);
  const kill=compact.indexOf('TerminateProcess(process.0,1)');
  for(const before of ['process_image(process.0)','born==observed.birth','installed(root,&image)'])
    if(compact.indexOf(before)<0||compact.indexOf(before)>kill)failures.push(`identity must precede terminate: ${before}`);
  if((compact.match(/returnOk\(None\)/g)??[]).length!==2)failures.push('both volume-root branches required');
  if((compact.match(/CompareStringOrdinal/g)??[]).length!==2)failures.push('native ordinal import/call required');
  if((compact.match(/process_image\(process\.0\)/g)??[]).length!==2)failures.push('both observed and reopened images required');
  if((compact.match(/TerminateProcess\(/g)??[]).length!==1)failures.push('exactly one handle termination site required');
  if(/taskkill|Stop-Process|powershell|EncodedCommand|ShellExecuteExW|DeleteService|GetProcessById/.test(code))failures.push('forbidden alternate sweep executor');
  return failures;
}
export function nsisCoreSweepViolations(source) {
  const failures=[];const macro=nsisMacroLines(source,NSIS_SWEEP_MACRO);
  if(!macro)return ['missing native sweep macro'];
  // This fixed canonical macro has no block-comment syntax. Reject both raw
  // delimiters, even in comments/literals, before line filtering: stripping or
  // ignoring a prefix could hide an executable opcode on this or a later line.
  // This constrains macro source only; runtime $INSTDIR data is never scanned.
  if (macro.some(line => /\/\*|\*\//.test(line)))
    failures.push('sweep macro source must not contain block-comment delimiters');
  const body=nsisCodeLines(macro);
  const at=body.indexOf(NSIS_SWEEP_LINES[0]);
  if(at<0||NSIS_SWEEP_LINES.some((line,i)=>body[at+i]!==line))failures.push('embedded native sweep block differs');
  for(const line of ['ReadEnvStr $R9 "POLARIS_SWEEP_ROOT"','Push $R9','StrCpy $R9 "$INSTDIR"',
    `System::Call 'kernel32::SetEnvironmentVariable(t "POLARIS_SWEEP_ROOT", t R9)'`,'Pop $R9','${If} $R4 != 0','DetailPrint "$R8 ($R4)"'])
    if(!body.includes(line))failures.push(`missing sweep handoff: ${line}`);
  // The caller's R9 and the previous environment value occupy distinct stack
  // slots. One remaining Push/Pop must not mask deletion of the other.
  const sameLines = (actual, expected) => actual.length === expected.length
    && expected.every((line, index) => actual[index] === line);
  // NSIS command lookup is case-insensitive. Collect every stack instruction,
  // including all Exch forms, before checking the fixed sequence below; no
  // exchange or extra Push/Pop may hide between handoff and restore operations.
  const stack = body.filter(line => /^(Push|Pop|Exch)\b/i.test(line))
    .map(line => line.replace(/\s*;.*$/, ''));
  if (!sameLines(stack, ['Push $R4', 'Push $R6', 'Push $R8', 'Push $R9', 'Push $R9',
    'Pop $R4', 'Pop $R9', 'Pop $R9', 'Pop $R8', 'Pop $R6', 'Pop $R4']))
    failures.push('sweep must have exactly the caller/environment/result stack operations');
  const setter = `System::Call 'kernel32::SetEnvironmentVariable(t "POLARIS_SWEEP_ROOT", t R9)'`;
  const save = ['ReadEnvStr $R9 "POLARIS_SWEEP_ROOT"', 'Push $R9', 'StrCpy $R9 "$INSTDIR"', setter];
  const saved = body.indexOf(save[0]), executed = body.indexOf(NSIS_SWEEP_LINES.at(-1));
  if (!sameLines(body.slice(0, 4), ['Push $R4', 'Push $R6', 'Push $R8', 'Push $R9'])
      || !sameLines(body.slice(-4), ['Pop $R9', 'Pop $R8', 'Pop $R6', 'Pop $R4']))
    failures.push('caller registers must be saved and restored in stack order');
  if (body.filter(line => line === 'Push $R9').length !== 2
      || body.filter(line => line === 'Pop $R9').length !== 2)
    failures.push('caller R9 and previous environment need separate balanced stack slots');
  if (saved < 0 || !sameLines(body.slice(saved, saved + save.length), save)
      || body.filter(line => line === save[0]).length !== 1
      || body.filter(line => line === save[2]).length !== 1
      || saved + save.length > body.indexOf(NSIS_SWEEP_LINES[0]))
    failures.push('previous environment must be saved before literal path handoff and delivery');
  if (body.filter(line => line.includes('SetEnvironmentVariable')).length !== 2
      || body.filter(line => line === setter).length !== 2
      || executed < 0 || !/^Pop \$R4(?:\s*;.*)?$/.test(body[executed + 1] ?? '')
      || !sameLines(body.slice(executed + 2, executed + 4), ['Pop $R9', setter])
      || body[executed + 4] !== '${If} $R4 != 0')
    failures.push('native result and previous environment must be restored before warning/continuation');
  const exec=body.filter(line=>/nsExec::|ExecWait|ExecShell/.test(line));
  if(exec.length!==1||exec[0]!==NSIS_SWEEP_LINES.at(-1))failures.push('sweep must have one exact native executor');
  for(const line of body){
    if(line.includes('$INSTDIR')&&line!=='StrCpy $R9 "$INSTDIR"')failures.push('installation path must be literal environment data');
    if(/powershell|EncodedCommand|ExecutionPolicy|FileWrite|FileOpen|Abort|Quit|MessageBox|--worker|--launch/i.test(line))failures.push('forbidden sweep operation');
  }
  const all=nsisCodeLines(source.split('\n'));
  if(all.some(line=>/taskkill|Stop-Process|nsis_tauri_utils::Kill|TerminateProcess/i.test(line)))failures.push('alternate termination site');
  for(const name of NSIS_SWEEP_HOOKS){const hook=nsisMacroLines(source,name);if(!hook){failures.push(`missing ${name}`);continue;}
    const lines=nsisCodeLines(hook),app=lines.indexOf(NSIS_APP_CHECK),sweep=lines.indexOf(NSIS_SWEEP_CALL);
    if(app<0||sweep<0||app>sweep)failures.push(`${name}: app check must precede native sweep`);
    if(lines.filter(line=>line===NSIS_SWEEP_CALL).length!==1)failures.push(`${name}: sweep must occur once`);
    const file=lines.findIndex(line=>/^(RMDir|Delete|File|Rename|CopyFiles)\b/.test(line));
    if(file>=0&&file<sweep)failures.push(`${name}: files touched before sweep`);
  }
  return failures;
}

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
