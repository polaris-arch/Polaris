import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import {
  NSIS_APP_CHECK, NSIS_SWEEP_CALL, NSIS_SWEEP_HOOKS, NSIS_SWEEP_MACRO,
  NSIS_SWEEP_LINES, NSIS_SWEEP_OPERATORS, NSIS_SWEEP_TAILS,
  nsisCoreSweepViolations, nativeSweepViolations, nsisMacroLines, sweepVerdict,
} from './lib/nsis-core-sweep.mjs';
const root = fileURLToPath(new URL('../', import.meta.url));
const source = path => readFileSync(root + path, 'utf8').replace(/\r\n/g, '\n');
const hook = source('src-tauri/nsis-hooks.nsh');
const native = source('crates/windows-cleaner/src/windows/sweep.rs');
const nativeTests = source('crates/windows-cleaner/src/windows/sweep/tests/mod.rs');
const samples = JSON.parse(source('scripts/fixtures/windows-core-sweep-samples.json'));
function replaceOnce(text, from, to) {
  assert.equal(text.split(from).length - 1, 1, `unique mutation anchor: ${from}`);
  return text.replace(from, () => to);
}
function inMacro(text, name, from, to) {
  const lines = nsisMacroLines(text, name); assert.ok(lines);
  const body = lines.join('\n');
  return replaceOnce(text, body, replaceOnce(body, from, to));
}
test('real PRE native producer and Rust source satisfy executable contracts', () => {
  assert.deepEqual(nsisCoreSweepViolations(hook), []);
  assert.deepEqual(nativeSweepViolations(native), []);
  const verifier = source('scripts/verify-packaging.mjs');
  assert.ok(verifier.includes("import { nsisCoreSweepViolations, nativeSweepViolations } from './lib/nsis-core-sweep.mjs';"));
  assert.ok(verifier.includes('for (const violation of nsisCoreSweepViolations(source)) {\n    fail(`nsis-hooks.nsh 内核清扫门：${violation}`);\n  }'));
});
test('embedded delivery/execution block is exact; every missing or changed line turns red', () => {
  for (const line of NSIS_SWEEP_LINES) {
    for (const changed of ['; removed', line + ' EXTRA'])
      assert.ok(nsisCoreSweepViolations(inMacro(hook, NSIS_SWEEP_MACRO, line, changed)).length, line);
  }
  for (const replacement of ['--worker', '--launch', '--sweep-installed-cores --pid=1', '--sweep-installed-cores "$INSTDIR"'])
    assert.ok(nsisCoreSweepViolations(inMacro(hook, NSIS_SWEEP_MACRO, '--sweep-installed-cores', replacement)).length);
});
test('missing native body, comment-only source and every PRE consumer fail closed', () => {
  assert.ok(nsisCoreSweepViolations('').length);
  assert.ok(nsisCoreSweepViolations(hook.replace('!macro PolarisStopInstalledCores', '!macro Missing')).length);
  for (const name of NSIS_SWEEP_HOOKS) {
    for (const [from, to] of [[NSIS_APP_CHECK, '; app check removed'], [NSIS_SWEEP_CALL, '; sweep removed'],
      [`  ${NSIS_APP_CHECK}\n  ${NSIS_SWEEP_CALL}`, `  ${NSIS_SWEEP_CALL}\n  ${NSIS_APP_CHECK}`],
      [NSIS_APP_CHECK, `Delete "$INSTDIR\\resources\\win\\sing-box.exe"\n  ${NSIS_APP_CHECK}`]])
      assert.ok(nsisCoreSweepViolations(inMacro(hook, name, from, to)).length, `${name}: ${from}`);
    assert.ok(nsisCoreSweepViolations(hook.replace(`!macro ${name}`, `!macro Missing_${name}`)).length);
  }
});
test('no alternate name/PID termination, policy override or blocking failure in hooks', () => {
  for (const bad of ['taskkill /F /IM sing-box.exe', 'Stop-Process -Id 1', 'nsis_tauri_utils::KillProcess "sing-box.exe"',
    'TerminateProcess', 'powershell -EncodedCommand AAA', 'Abort', 'Quit', 'MessageBox MB_OK "failed"'])
    assert.ok(nsisCoreSweepViolations(inMacro(hook, NSIS_SWEEP_MACRO, '${If} $R4 != 0', `\${If} $R4 != 0\n${bad}`)).length, bad);
});
test('installation path is literal environment data and previous environment is restored', () => {
  const setter = `System::Call 'kernel32::SetEnvironmentVariable(t "POLARIS_SWEEP_ROOT", t R9)'`;
  const read = 'ReadEnvStr $R9 "POLARIS_SWEEP_ROOT"';
  const literal = 'StrCpy $R9 "$INSTDIR"';
  // Each context is unique even though there are two legitimate Push/Pop R9s.
  for (const [from, to] of [
    [read, '; removed'], [literal, '; removed'],
    ['Push $R8\n  Push $R9', 'Push $R8'],
    [`${read}\n  Push $R9`, read],
    [`Pop $R9\n  ${setter}`, setter],
    ['Pop $R9\n  Pop $R8', 'Pop $R8'],
    [`${literal}\n  ${setter}`, literal],
    [`Pop $R9\n  ${setter}`, 'Pop $R9'],
    [`${read}\n  Push $R9`, `Push $R9\n  ${read}`],
    ['Push $R8\n  Push $R9', 'Push $R9\n  Push $R8'],
    ['Pop $R9\n  Pop $R8', 'Pop $R8\n  Pop $R9'],
    [`${literal}\n  ${setter}`, `${setter}\n  ${literal}`],
    [`Pop $R9\n  ${setter}`, `${setter}\n  Pop $R9`],
    [`${literal}\n  ${setter}`, `${literal}\n  ${setter}\n  ${setter}`],
    [`Pop $R9\n  ${setter}`, `Pop $R9\n  ${setter.replace('t R9', 't R8')}`],
    [`${literal}\n  ${setter}`, `${literal}\n  ${setter.replace('POLARIS_SWEEP_ROOT', 'OTHER_ROOT')}`],
    ['Pop $R9\n  Pop $R8', 'Pop $R7\n  Pop $R8'],
  ]) assert.ok(nsisCoreSweepViolations(inMacro(hook, NSIS_SWEEP_MACRO, from, to)).length, from);
  const restored = `  Pop $R9\n  ${setter}\n`;
  const lateRestore = replaceOnce(hook, restored, '');
  const earlyRestore = replaceOnce(lateRestore, NSIS_SWEEP_LINES.at(-1), restored + NSIS_SWEEP_LINES.at(-1));
  assert.ok(nsisCoreSweepViolations(earlyRestore).length, 'restore cannot move before actual native execution');
  const lines = nsisMacroLines(hook, NSIS_SWEEP_MACRO);
  const original = lines.join('\n');
  for (const [index, line] of lines.entries()) {
    if (!/^\s*(Push|Pop)\b/.test(line)) continue;
    const missing = [...lines]; missing[index] = '  ; removed stack operation';
    assert.ok(nsisCoreSweepViolations(replaceOnce(hook, original, missing.join('\n'))).length,
      `every actual stack slot is required: ${index} ${line}`);
  }
  for (const extra of ['Push $R7', 'Pop $R7', 'Push $R4', 'Pop $R4'])
    assert.ok(nsisCoreSweepViolations(inMacro(hook, NSIS_SWEEP_MACRO,
      'InitPluginsDir', `${extra}\n  InitPluginsDir`)).length, 'unbalanced extra stack operation');
  const body = original;
  assert.equal((body.match(/SetEnvironmentVariable/g) ?? []).length, 2);
  assert.ok(body.indexOf('ReadEnvStr') < body.indexOf('nsExec::'));
  assert.ok(body.lastIndexOf('SetEnvironmentVariable') > body.indexOf('nsExec::'));
});
test('every case-insensitive stack instruction is collected at handoff and caller restore', () => {
  assert.deepEqual(nsisCoreSweepViolations(hook), [], 'unchanged real producer is the positive control');
  const extras = ['Push $R7', 'push $R7', 'PuSh $R4', 'PUSH $0',
    'Pop $R7', 'pop $R8', 'pOp $R4', 'POP $0'];
  for (const command of ['Exch', 'exch', 'eXcH', 'EXCH'])
    for (const argument of ['', ' 0', ' 1', ' 2', ' $R9', ' $R7', ' $0']) extras.push(command + argument);
  for (const extra of extras) {
    // Both gaps preserve all original save/result/setter blocks and the final
    // four restores; only the shared fixed stack sequence can admit/reject it.
    for (const [from, to] of [
      ['InitPluginsDir', `${extra}\n  InitPluginsDir`],
      ['Pop $R9\n  Pop $R8', `${extra}\n  Pop $R9\n  Pop $R8`],
    ]) {
      const changed = inMacro(hook, NSIS_SWEEP_MACRO, from, to);
      assert.notEqual(changed, hook, 'negative must really insert a stack instruction');
      assert.ok(nsisCoreSweepViolations(changed).includes(
        'sweep must have exactly the caller/environment/result stack operations'), `${extra} before ${from}`);
    }
  }
});
test('fixed sweep macro rejects every block-comment prefix before line filtering', () => {
  const diagnostic = 'sweep macro source must not contain block-comment delimiters';
  assert.deepEqual(nsisCoreSweepViolations(hook), [], 'unchanged real producer is the positive control');
  for (const extra of ['Push $R7', 'push $R7', 'PuSh $R4', 'PUSH $0',
    'Pop $R7', 'pop $R8', 'pOp $R4', 'POP $0',
    'Exch $R9', 'exch', 'eXcH 1', 'EXCH $0']) {
    for (const prefixed of [
      `/*c*/ ${extra}`, `/*c*/ /*d*/ ${extra}`, `\t/*c*/\t${extra}`,
      `/*c\n  */ ${extra}`, `/*c\n  comment body\n  */\t${extra}`,
      `/*c\n  ; apparent line comment\n  # apparent line comment\n  */ ${extra}`,
    ]) {
      for (const [from, to] of [
        ['InitPluginsDir', `${prefixed}\n  InitPluginsDir`],
        ['Pop $R9\n  Pop $R8', `${prefixed}\n  Pop $R9\n  Pop $R8`],
      ]) {
        const changed = inMacro(hook, NSIS_SWEEP_MACRO, from, to);
        assert.notEqual(changed, hook, 'negative must really insert a prefixed stack instruction');
        assert.ok(nsisCoreSweepViolations(changed).includes(diagnostic), `${prefixed} before ${from}`);
      }
    }
  }
  // The source-format restriction is deliberately raw and local, including
  // isolated delimiters and markers hidden by line-comment/literal filtering.
  for (const line of ['/*', '*/', '; /*', '# */', 'DetailPrint "/* literal */"']) {
    const changed = inMacro(hook, NSIS_SWEEP_MACRO, 'InitPluginsDir', `${line}\n  InitPluginsDir`);
    assert.notEqual(changed, hook);
    assert.ok(nsisCoreSweepViolations(changed).includes(diagnostic), line);
  }
  assert.deepEqual(nsisCoreSweepViolations(`/* outside fixed macro */\n${hook}`), [],
    'the canonical-format constraint does not scan other source or runtime paths');
});
test('all 55 samples and every seven-operator inverse case remain covered', () => {
  assert.equal(samples.length, 55); assert.equal(NSIS_SWEEP_OPERATORS.length, 7);
  for (const [category, instdir, image, expected] of samples)
    assert.equal(sweepVerdict(instdir, image), expected, `${category}: ${instdir} <- ${image}`);
  for (const { id, rust } of NSIS_SWEEP_OPERATORS) {
    assert.ok(samples.some(([, dir, image, expected]) => sweepVerdict(dir, image, new Set([id])) !== expected), id);
    assert.ok(nativeSweepViolations(native.replace(rust, 'REMOVED')).length, rust);
  }
  const categories = new Set(samples.map(row => row[0]));
  for (const name of ['baseline','space','neighbour','single-quote','dollar','backtick','semicolon','percent','non-ascii',
    'trailing-separator','volume-root','unc','case','mixed-slashes','dot-segments','extended-prefix','short-name','link','other-location','other-name','unrelated'])
    assert.ok(categories.has(name), name);
});
test('same shared samples execute production Rust predicate, held image and exact birth before terminate', () => {
  for (const fragment of ['windows-core-sweep-samples.json', 'sweep_root', 'installed(&root', 'sweep(&root',
    'foreign-image', 'wrong-birth', 'child.0.wait()', 'child.0.try_wait()', 'self.0.kill()', 'self.0.wait()'])
    assert.ok(nativeTests.includes(fragment), fragment);
  for (const fragment of ['GetProcessTimes', 'born == observed.birth', 'process_image(process.0)',
    'TerminateProcess(process.0, 1)', 'WaitForSingleObject(process.0, 5000)'])
    assert.ok(nativeSweepViolations(native.replace(fragment, 'REMOVED')).length, fragment);
  assert.ok(!native.includes('TotalSeconds'));
  assert.deepEqual(NSIS_SWEEP_TAILS, ['_up_\\resources\\win\\sing-box.exe', 'resources\\win\\sing-box.exe']);
});
test('Windows consumer requires fresh actual native fixture output; missing producer is failure', { skip: process.platform !== 'win32' }, () => {
  assert.ok(process.env.POLARIS_C05_NATIVE_LOG, 'Windows native producer receipt is required, never skip on missing binary');
  const log = readFileSync(process.env.POLARIS_C05_NATIVE_LOG, 'utf8');
  for (const marker of ['Passed 55 native sweep predicate cases.', 'Passed 3 native owned-handle sweep cases.',
    'Passed 8 native registration fixture cases.', 'Passed 17 native inert SCM cases.', 'Passed 20 native rollback state/transaction cases.'])
    assert.ok(log.includes(marker), marker);
});
