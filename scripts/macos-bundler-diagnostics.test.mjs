import assert from 'node:assert/strict';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const wrapper = join(root, 'scripts/bundler-diagnostics.sh');
const testShell = (name, fn) => test(name, {
  skip: process.platform === 'win32' ? 'POSIX diagnostic wrapper' : false,
}, fn);
function fixture({ status = 0, teeStatus = null, missingLog = false,
  unusableLog = false, missingTauri = false, bytes = 0, prefixArgs = [],
  target = 'aarch64-apple-darwin' } = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'polaris-dmg-log-test-'));
  const log = unusableLog ? directory : join(directory, 'full output.log');
  const calls = join(directory, 'calls.jsonl');
  const args = ['build', '--target', target, '--config',
    'config with spaces.json', '--', 'literal;$HOME', 'line\nbreak', ''];
  const out = 'original stdout\n' + 'O'.repeat(bytes) + '\nstdout end\n';
  const err = 'original stderr\n' + 'E'.repeat(bytes) + '\nstderr end\n';
  try {
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
    const result = spawnSync('bash', [wrapper, ...prefixArgs, ...args], {
      env, encoding: 'utf8', timeout: 5000, maxBuffer: 4 * 1024 * 1024,
    });
    assert.ifError(result.error);
    return { ...result, args, expectedArgs: [...prefixArgs, ...args], out, err,
      log: !unusableLog && existsSync(log) ? readFileSync(log, 'utf8') : '',
      calls: existsSync(calls) ? readFileSync(calls, 'utf8').trim().split('\n').map(JSON.parse) : [] };
  } finally { rmSync(directory, { recursive: true, force: true }); }
}
function assertOriginalFailure(result, expected = 37) {
  assert.equal(result.status, expected, result.stderr + result.log.slice(-3000));
  assert.deepEqual(result.calls, [result.expectedArgs], 'exactly one Tauri invocation with only admitted diagnostic flags');
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
testShell('Linux explicit CLI Debug preserves full output, every product argument and the original exit', () => {
  for (const status of [0, 1, 37, 127, 143]) {
    const result = fixture({ prefixArgs: ['--verbose'],
      target: 'x86_64-unknown-linux-gnu', status, bytes: status === 37 ? 256 * 1024 : 0 });
    assertOriginalFailure(result, status);
    assert.equal(result.log, result.stdout);
    assert.doesNotMatch(result.log + result.stdout + result.stderr, /must-not-print-/);
  }
});
testShell('Linux evidence failure cannot make a failed bundler green or start it without a destination', () => {
  for (const status of [0, 37]) {
    const result = fixture({ prefixArgs: ['--verbose'], status, teeStatus: 19 });
    assert.equal(result.status, status || 19);
    assert.deepEqual(result.calls, [result.expectedArgs]);
  }
  for (const options of [{ missingLog: true }, { unusableLog: true }]) {
    const result = fixture({ prefixArgs: ['--verbose'], ...options });
    assert.notEqual(result.status, 0);
    assert.deepEqual(result.calls, []);
  }
  const failed = fixture({ prefixArgs: ['--verbose'], missingTauri: true });
  assert.notEqual(failed.status, 0);
  assert(failed.log.length > 0);
});
