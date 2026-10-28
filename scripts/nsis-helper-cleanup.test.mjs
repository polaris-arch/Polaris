import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { nsisHelperCleanupViolations, HELPER_CLEANUP_LINES } from './lib/nsis-helper-cleanup.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const hook = readFileSync(`${root}src-tauri/nsis-hooks.nsh`, 'utf8');
const worker = readFileSync(`${root}crates/windows-cleaner/src/windows.rs`, 'utf8');
const compact = worker.replace(/\s+/g, '');
const policy = readFileSync(`${root}crates/windows-cleaner/src/lib.rs`, 'utf8');
const failures = (s) => nsisHelperCleanupViolations(s);
const mutateHook = (from, to) => {
  const at = hook.indexOf('InitPluginsDir', hook.indexOf('!macro NSIS_HOOK_POSTUNINSTALL'));
  return hook.slice(0, at) + hook.slice(at).replace(from, to);
};
test('real embedded-native NSIS cleanup has no contract violations', () => assert.deepEqual(failures(hook), []));
for (const line of HELPER_CLEANUP_LINES) {
  test(`native handoff rejects missing instruction: ${line}`, () => {
    assert.ok(hook.includes(line));
    assert.ok(failures(mutateHook(line, '; removed')).length);
  });
}
for (const needle of ['${If} $UpdateMode <> 1', '${If} $R7 != 0', '${If} $R7 == 20', '${If} $R7 == 2', '${If} $R7 == 3', 'DetailPrint "$R8 ($R7)"']) {
  test(`native cleanup outcome/guard cannot disappear: ${needle}`, () => {
    const at = hook.indexOf('!macro NSIS_HOOK_POSTUNINSTALL');
    const before = hook.slice(0, at), body = hook.slice(at);
    assert.ok(body.includes(needle));
    assert.ok(failures(before + body.replace(needle, '; removed')).length);
  });
}
test('missing hook fails rather than vacuously passing', () => assert.ok(failures(hook.replace('!macro NSIS_HOOK_POSTUNINSTALL', '!macro OTHER')).length));
test('embedded executable stays independent of the removed app directory', () => {
  const body = hook.slice(hook.indexOf('!macro NSIS_HOOK_POSTUNINSTALL'));
  assert.ok(body.includes('File /oname=polaris-cleaner.exe'));
  assert.ok(!body.includes('$INSTDIR'));
});
test('payload filename/role substitution is rejected', () => {
  for (const [from,to] of [['--launch','--worker'],['/oname=polaris-cleaner.exe','/oname=other.exe']]) {
    assert.ok(failures(hook.replace(from,to)).length);
  }
});
test('old privileged script/recursive deletion regressions are rejected', () => {
  for (const operation of ['powershell -EncodedCommand ABC', 'RMDir /r "$APPDATA"', 'ExecShell runas "$INSTDIR\\script.ps1"', 'Abort']) {
    assert.ok(failures(hook.replace('!macro NSIS_HOOK_POSTUNINSTALL', `!macro NSIS_HOOK_POSTUNINSTALL\n${operation}`)).length);
  }
});
test('comment-only handoff is not executable evidence', () => {
  for (const line of HELPER_CLEANUP_LINES) assert.ok(failures(mutateHook(line, `; ${line}`)).length);
});
test('native target matches the installed fixed root despite redirected ProgramData', () => {
  assert.ok(policy.includes('SUPPORT_PARENT: &str = r"C:\\ProgramData"'));
  const proto = readFileSync(`${root}crates/helper-proto/src/lib.rs`, 'utf8');
  assert.ok(proto.includes('DEFAULT_SUPPORT_DIR: &str = r"C:\\ProgramData\\Polaris"'));
  assert.ok(compact.includes('PathBuf::from(crate::SUPPORT_PARENT)'));
  assert.ok(!worker.includes('SHGetKnownFolderPath'));
  assert.ok(!worker.includes('std::env::var'));
  assert.ok(policy.includes('SERVICE: &str = "PolarisHelper"'));
  assert.ok(policy.includes('SUPPORT_LEAF: &str = "Polaris"'));
});
test('native cleanup uses relative opens, retained tree handles and same-handle deletion', () => {
  for (const needle of ['RootDirectory: handle(parent)', 'FILE_OPEN_REPARSE_POINT', 'GetFileInformationByHandleEx', 'GetSecurityInfo(handle(file)', 'NtSetInformationFile(handle(file)', 'identity(&object.file)? != object.identity']) assert.ok(compact.includes(needle.replace(/\s+/g,'')), needle);
  assert.ok(!/remove_dir_all|Remove-Item|DeleteFileW|RemoveDirectoryW|cmd\.exe|sc\.exe/.test(worker));
});
test('service deletion waits for stop and actual SCM absence', () => {
  for (const needle of ['QueryServiceStatusEx', 'SERVICE_STOPPED', 'DeleteService', 'ERROR_SERVICE_DOES_NOT_EXIST', 'service delete still pending']) assert.ok(compact.includes(needle.replace(/\s+/g,'')), needle);
});
test('image custody encloses native elevation and waits for worker completion', () => {
  for (const needle of ['K32GetMappedFileNameW', 'VOLUME_NAME_NT', 'SEE_MASK_NOCLOSEPROCESS', 'ShellExecuteExW', 'WaitForSingleObject', 'GetExitCodeProcess', 'drop(process); drop(image)']) assert.ok(compact.includes(needle.replace(/\s+/g,'')), needle);
});
test('SYSTEM helper explicitly requires bundled UAC and never creates a private worker', () => {
  const proc = readFileSync(`${root}crates/helper/src/platform/windows/winproc/win.rs`,'utf8');
  const helper = readFileSync(`${root}crates/helper/src/platform/windows/helper.rs`,'utf8');
  assert.ok(proc.includes('native-cleaner-uac-required'));
  assert.ok(helper.includes('native-cleaner-uac-required'));
  assert.ok(!worker.includes('launch_system_worker'));
  assert.ok(!worker.includes('PolarisCleaner-'));
  assert.ok(!worker.includes('CreateProcessW'));
  assert.ok(!helper.includes('self.proc.spawn_self_uninstall()'));
  assert.ok(!proc.includes('self_uninstall_cmd_line'));
});
test('manager fallback elevates only bundled cleaner and has no Windows uninstall script', () => {
  const manager = readFileSync(`${root}crates/helper-client/src/manager.rs`,'utf8');
  assert.ok(manager.includes('polaris_windows_cleaner::launch_bundle()'));
  assert.ok(!manager.includes('fn build_win_uninstall_script'));
  assert.ok(!manager.includes('polaris-helper-uninstall.ps1'));
});
test('package builds/stages native payload before constructing NSIS', () => {
  const packageWorkflow = readFileSync(`${root}.github/workflows/package.yml`,'utf8');
  assert.ok(packageWorkflow.includes('cargo build --release -p polaris-windows-cleaner --bin polaris-cleaner'));
  assert.ok(packageWorkflow.includes('resources/win/polaris-cleaner.exe'));
});

test('service absent plus an inaccessible protected tree still reaches native cleanup', () => {
  // The real hook has no ordinary-token SCM/FindFirstFile absence gate.
  // A mutant reinstating either probe must be rejected, regardless of its result.
  const body = hook.slice(hook.indexOf('!macro NSIS_HOOK_POSTUNINSTALL'));
  assert.ok(!body.includes('FileExists'));
  assert.ok(!body.includes('sc.exe'));
  for (const guard of ['${If} $R4 == 0', '${If} ${FileExists} "$R9\\*.*"']) {
    const mutant = hook.replace('InitPluginsDir', `${guard}\nInitPluginsDir`)
      .replace('SetOutPath $R6', 'SetOutPath $R6\n${EndIf}');
    assert.ok(failures(mutant).length, guard);
  }
  assert.deepEqual(failures(hook), []);
});

test('actual configuration verifier accepts the native hook without an unprivileged absence gate', () => {
  const result = spawnSync(process.execPath, [`${root}scripts/verify-packaging.mjs`, 'confs'], { cwd: root, encoding: 'utf8' });
  assert.ifError(result.error);
  assert.ok(result.status === 0 || result.status === 1, result.stderr);
  // Missing resource fixtures/mac_arch_tag remain real separate failures.
  // No source-only hook matcher can substitute for running this CLI.
  const output = result.stdout + result.stderr;
  assert.ok(!/✗[^\n]*nsis-hooks\.nsh/.test(output), output);
  assert.ok(!/✗[^\n]*十处|✗[^\n]*SetShellVarContext all/.test(output), output);
});
