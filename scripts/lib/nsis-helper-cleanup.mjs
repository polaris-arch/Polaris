// NSIS hands its embedded native payload to the fixed cleaner role. Target
// selection, image custody, UAC, SCM and handle deletion live in the Rust crate.
import { nsisMacroLines } from './nsis-core-sweep.mjs';

export const HELPER_CLEANUP_HOOK = 'NSIS_HOOK_POSTUNINSTALL';
export const NATIVE_PAYLOAD = 'polaris-cleaner.exe';
export const HELPER_CLEANUP_LINES = [
  'InitPluginsDir',
  'StrCpy $R6 $OUTDIR',
  'SetOutPath "$PLUGINSDIR"',
  'File /oname=polaris-cleaner.exe "${POLARIS_NATIVE_CLEANER_PAYLOAD}"',
  'SetOutPath $R6',
  'nsExec::ExecToStack \'"$PLUGINSDIR\\polaris-cleaner.exe" --launch\'',
  'Pop $R7',
  'Pop $R5',
];

export function nsisHelperCleanupViolations(source) {
  const macro = nsisMacroLines(source, HELPER_CLEANUP_HOOK);
  if (!macro) return [`missing ${HELPER_CLEANUP_HOOK}`];
  const code = macro.map((line) => line.trim()).filter((line) => line && !line.startsWith(';'));
  const failures = [];
  if (!source.startsWith('!define POLARIS_NATIVE_CLEANER_PAYLOAD "${__FILEDIR__}\\..\\resources\\win\\polaris-cleaner.exe"\n')) failures.push("payload must resolve at hook include time");
  const start = code.indexOf(HELPER_CLEANUP_LINES[0]);
  if (start < 0 || HELPER_CLEANUP_LINES.some((line, i) => code[start+i] !== line)) {
    failures.push('embedded native payload/execution block differs');
  }
  for (const needle of [
    '${If} $UpdateMode <> 1',
    '${If} $R7 != 0', '${If} $R7 == 20', '${If} $R7 == 2', '${If} $R7 == 3',
    'DetailPrint "$R8 ($R7)"',
  ]) {
    if (!code.includes(needle)) failures.push(`missing native cleanup contract: ${needle}`);
  }
  const guards = [];
  for (const line of code.slice(0, start)) {
    if (line.startsWith('${If}')) guards.push(line);
    else if (line === '${EndIf}') guards.pop();
    else if (line.startsWith('${Else')) failures.push('unexpected pre-launch branch');
  }
  if (guards.length !== 1 || guards[0] !== '${If} $UpdateMode <> 1') {
    failures.push('native payload must be reached on every non-update uninstall');
  }
  const dangerous = /powershell|EncodedCommand|ExecutionPolicy|POLARIS_CLEANUP_|Remove-Item|rmdir|RMDir|ExecShell|Abort|Quit|UninstallString|\$LOCALAPPDATA/i;
  for (const line of code) if (dangerous.test(line)) failures.push(`forbidden cleanup operation: ${line}`);
  const launches = code.filter((line) => /nsExec::Exec|ExecWait|ExecShell/.test(line));
  if (launches.length !== 1 || launches[0] !== HELPER_CLEANUP_LINES[5]) {
    failures.push('cleanup may only launch the fixed native role');
  }
  if (code.some((line) => /FileExists|sc\.exe|SetShellVarContext/.test(line))) {
    failures.push('unprivileged absence probes cannot gate native cleanup');
  }
  return failures;
}
