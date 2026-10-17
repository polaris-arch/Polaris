import assert from 'node:assert/strict';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';

const root = fileURLToPath(new URL('../', import.meta.url));
const wrapper = readFileSync(join(root, 'scripts/macos-bundler-diagnostics.sh'), 'utf8');
const workflow = readFileSync(join(root, '.github/workflows/package.yml'), 'utf8');
const testShell = (name, fn) => test(name, {
  skip: process.platform === 'win32' ? 'macOS-only POSIX wrapper; source contract runs on Windows' : false,
}, fn);
function step(source, name) {
  const active = source.split('\n').filter((line) => !/^\s*#/.test(line)).join('\n');
  const marker = '      - name: ' + name + '\n', start = active.indexOf(marker);
  assert(start >= 0, 'missing step: ' + name);
  const end = active.indexOf('\n      - ', start + marker.length);
  return active.slice(start, end < 0 ? active.length : end);
}
function field(source, name) {
  const found = new RegExp('^\\s+' + name + ': (.+)$', 'm').exec(source);
  assert(found, 'missing field: ' + name);
  return found[1];
}
function evaluate(value, os, outcome = 'success') {
  const expression = value.replace(/^\$\{\{ /, '').replace(/ \}\}$/, '');
  return runInNewContext(expression, { runner: { os },
    steps: { installers: { outcome } }, always: () => true }, { timeout: 100 });
}
function assertWorkflow(source) {
  const build = step(source, 'Build installers'), evidence = step(source, 'Preserve macOS bundler evidence');
  assert.equal(field(build, 'id'), 'installers');
  assert.equal(field(build, 'uses'), 'tauri-apps/tauri-action@v1');
  assert.equal(field(build, 'args'), '${{ matrix.tauri_args }}');
  assert.equal(field(build, 'retryAttempts'), '0');
  assert.doesNotMatch(build, /continue-on-error:|TAURI_BUNDLER_DMG_IGNORE_CI:|RUST_LOG:|run:/);
  for (const os of ['macOS', 'Linux', 'Windows']) {
    assert.equal(evaluate(field(build, 'tauriScript'), os),
      os === 'macOS' ? 'bash scripts/macos-bundler-diagnostics.sh' : 'tauri');
    assert.equal(evaluate(field(build, 'TAURI_CLI_VERBOSITY'), os), os === 'macOS' ? '1' : '');
    for (const outcome of ['', 'skipped', 'success', 'failure', 'cancelled']) {
      assert.equal(evaluate(field(evidence, 'if'), os, outcome),
        os === 'macOS' && !['', 'skipped'].includes(outcome));
    }
  }
  assert.match(field(evidence, 'if'), /^always\(\) && /);
  const log = '${{ runner.temp }}/polaris-tauri-${{ matrix.label }}.log';
  assert.equal(field(build, 'POLARIS_TAURI_LOG'), log);
  assert(evidence.includes('\n            ' + log + '\n'), 'upload exact full-output log');
  assert.equal(field(evidence, 'uses'), 'actions/upload-artifact@v7');
  assert.equal(field(evidence, 'if-no-files-found'), 'error');
  assert.equal(field(evidence, 'retention-days'), '7');
  assert.equal(field(evidence, 'name'),
    'macos-bundler-evidence-${{ github.sha }}-${{ github.run_id }}-${{ github.run_attempt }}-${{ matrix.label }}');
  assert(evidence.includes('target/*/release/bundle/dmg/bundle_dmg.sh'));
  assert.doesNotMatch(evidence, /continue-on-error:|run:/);
}
test('macOS-only diagnostics retain the action and arguments and always archive attempted output', () => {
  assertWorkflow(workflow);
});
test('workflow negative controls reject lost logs, retries, skips and widened wrapper scope', () => {
  for (const [before, after] of [
    ["runner.os == 'macOS' && '1' || ''", "runner.os == 'macOS' && '0' || ''"],
    ["runner.os == 'macOS' && 'bash scripts/macos-bundler-diagnostics.sh' || 'tauri'",
      "'bash scripts/macos-bundler-diagnostics.sh'"],
    ['          args: ${{ matrix.tauri_args }}', '          args: --bundles app'],
    ['          retryAttempts: 0', '          retryAttempts: 1'],
    ['        if: always() && runner.os', '        if: success() && runner.os'],
    ["steps.installers.outcome != 'skipped' && steps.installers.outcome != ''",
      "steps.installers.outcome == 'success'"],
    ['            ${{ runner.temp }}/polaris-tauri-${{ matrix.label }}.log\n', ''],
    ['          POLARIS_TAURI_LOG: ${{ runner.temp }}/polaris-tauri-${{ matrix.label }}.log',
      '          POLARIS_TAURI_LOG: target/release/bundle/dmg/diagnostic.log'],
    ['        id: installers', '        id: installers\n        continue-on-error: true'],
    ['          retryAttempts: 0',
      '          retryAttempts: 0\n          TAURI_BUNDLER_DMG_IGNORE_CI: false'],
    ['          if-no-files-found: error\n          retention-days: 7\n\n      # Tauri',
      '          if-no-files-found: warn\n          retention-days: 7\n\n      # Tauri'],
  ]) {
    const changed = workflow.replace(before, after);
    assert.notEqual(changed, workflow, 'negative control did not alter the workflow');
    assert.throws(() => assertWorkflow(changed), 'negative control accepted: ' + before);
  }
});
function fixture({ status = 0, teeStatus = null, source = wrapper, missingLog = false,
  unusableLog = false, missingTauri = false, bytes = 0 } = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'polaris-dmg-log-test-'));
  const log = unusableLog ? directory : join(directory, 'full output.log');
  const calls = join(directory, 'calls.jsonl'), script = join(directory, 'wrapper.sh');
  const args = ['build', '--target', 'aarch64-apple-darwin', '--config',
    'config with spaces.json', '--', 'literal;$HOME', 'line\nbreak', ''];
  const out = 'original stdout\n' + 'O'.repeat(bytes) + '\nstdout end\n';
  const err = 'original stderr\n' + 'E'.repeat(bytes) + '\nstderr end\n';
  try {
    writeFileSync(script, source);
    writeFileSync(join(directory, 'tauri'), '#!/usr/bin/env node\n'
      + 'const fs = require("node:fs");\n'
      + 'fs.appendFileSync(process.env.FIXTURE_CALLS, JSON.stringify(process.argv.slice(2)) + "\\n");\n'
      + 'process.stdout.write(' + JSON.stringify(out) + ', () => {\n'
      + 'process.stderr.write(' + JSON.stringify(err) + ', () => { process.exitCode = ' + status + '; });\n'
      + '});\n', { mode: 0o700 });
    if (teeStatus !== null) writeFileSync(join(directory, 'tee'),
      '#!/bin/sh\ncat\nexit ' + teeStatus + '\n', { mode: 0o700 });
    if (missingTauri) {
      // A nonexistent shebang interpreter deterministically models launch failure,
      // even if the test host has a real Tauri installation elsewhere on PATH.
      writeFileSync(join(directory, 'tauri'), '#!/polaris-nonexistent-interpreter\n', { mode: 0o700 });
    }
    const env = { ...process.env, PATH: directory + ':' + process.env.PATH,
      FIXTURE_CALLS: calls, POLARIS_TAURI_LOG: log,
      GITHUB_TOKEN: 'must-not-print-github-token-fixture',
      GH_TOKEN: 'must-not-print-gh-token-fixture' };
    if (missingLog) delete env.POLARIS_TAURI_LOG;
    const result = spawnSync('bash', [script, ...args], {
      env, encoding: 'utf8', timeout: 5000, maxBuffer: 4 * 1024 * 1024,
    });
    assert.ifError(result.error);
    return { ...result, args, out, err,
      log: !unusableLog && existsSync(log) ? readFileSync(log, 'utf8') : '',
      calls: existsSync(calls) ? readFileSync(calls, 'utf8').trim().split('\n').map(JSON.parse) : [] };
  } finally { rmSync(directory, { recursive: true, force: true }); }
}
function assertOriginalFailure(result, expected = 37) {
  assert.equal(result.status, expected, result.stderr + result.log.slice(-3000));
  assert.deepEqual(result.calls, [result.args], 'exactly one unchanged Tauri invocation');
  assert(result.log.includes(result.out), 'original stdout missing');
  assert(result.log.includes(result.err), 'original stderr missing');
}
testShell('success retains complete stdout, stderr and exact arguments without dumping credentials', () => {
  const result = fixture({ bytes: 256 * 1024 });
  assertOriginalFailure(result, 0);
  assert.equal(result.log, result.stdout, 'job output and artifact contain the same child bytes');
  assert.doesNotMatch(result.log + result.stdout + result.stderr, /must-not-print-/);
});
testShell('every failing status retains complete output and is preserved instead of tee success', () => {
  for (const status of [1, 37, 64, 127, 143]) {
    assertOriginalFailure(fixture({ status, bytes: status === 37 ? 256 * 1024 : 0 }), status);
  }
});
testShell('tee failure is fatal but cannot replace a simultaneous original Tauri failure', () => {
  const success = fixture({ teeStatus: 19 });
  assert.equal(success.status, 19);
  assert.deepEqual(success.calls, [success.args]);
  const failed = fixture({ status: 37, teeStatus: 19 });
  assert.equal(failed.status, 37);
  assert.deepEqual(failed.calls, [failed.args]);
});
testShell('missing or unwritable destination fails before Tauri; spawn failure remains nonzero and logged', () => {
  for (const options of [{ missingLog: true }, { unusableLog: true }]) {
    const result = fixture(options);
    assert.notEqual(result.status, 0);
    assert.deepEqual(result.calls, []);
  }
  const failed = fixture({ missingTauri: true });
  assert.notEqual(failed.status, 0);
  assert.deepEqual(failed.calls, []);
  assert(failed.log.length > 0, 'failed launch stderr is retained');
});
testShell('runtime negative controls reject dropped stderr, swallowed exit status and a second invocation', () => {
  for (const [before, after] of [
    ['tauri "$@" 2>&1 | tee', 'tauri "$@" | tee'],
    ['exit "${statuses[0]}"', 'exit 0'],
    ['tauri "$@" 2>&1 | tee', 'tauri "$@" >/dev/null 2>&1\ntauri "$@" 2>&1 | tee'],
  ]) {
    const source = wrapper.replace(before, after);
    assert.notEqual(source, wrapper);
    assert.throws(() => assertOriginalFailure(fixture({ status: 37, source })));
  }
});

