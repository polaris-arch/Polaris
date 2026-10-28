import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const testShell = (name, run) => test(name, { skip: process.platform === 'win32' ? 'POSIX shell fixture; macOS sampler does not run on Windows' : false }, run);
const script = join(dirname(fileURLToPath(import.meta.url)), 'macos-memory-sample.sh');
// Exercise the real shell parser using bounded, deterministic local tool stubs;
// this is not a macOS footprint measurement or evidence of device memory use.
function sample(output, status = 0, web = '-', rss = '42', rssStatus = 0) {
  const scratch = mkdtempSync(join(tmpdir(), 'polaris-memory-sample-test-'));
  const stub = (name, body) => writeFileSync(join(scratch, name), '#!/bin/sh\n' + body + '\n', { mode: 0o700 });
  try {
    stub('uname', 'echo Darwin');
    stub('date', 'case "$*" in *%s*) echo 12345;; *) echo 2026-10-15T00:00:00Z;; esac');
    stub('ps', 'printf "%s\\n" "$SAMPLE_RSS"\nexit "$SAMPLE_RSS_STATUS"');
    stub('footprint', 'echo observed >> "$SAMPLE_CALL_LOG"\nprintf "%s\\n" "$SAMPLE_OUTPUT"\nexit "$SAMPLE_STATUS"');
    const csv = join(scratch, 'sample.csv'), log = join(scratch, 'calls');
    execFileSync('/bin/sh', [script, String(process.pid), web, '0', '1', 'fixture', csv], {
      env: { ...process.env, PATH: `${scratch}:/usr/bin:/bin`, SAMPLE_CALL_LOG: log,
        SAMPLE_OUTPUT: output, SAMPLE_STATUS: String(status), SAMPLE_RSS: rss, SAMPLE_RSS_STATUS: String(rssStatus) }, timeout: 3000, maxBuffer: 16384,
    });
    const [header, row] = readFileSync(csv, 'utf8').trim().split('\n');
    return { values: Object.fromEntries(header.split(',').map((name, index) => [name, row.split(',')[index]])),
      calls: readFileSync(log, 'utf8').trim().split('\n').length };
  } finally { rmSync(scratch, { recursive: true, force: true }); }
}

testShell('one footprint observation yields both fields and absent web stays explicit', () => {
  const { values, calls } = sample('phys_footprint: 123\nphys_footprint_peak: 456');
  assert.equal(calls, 1);
  assert.equal(values.app_phys_bytes, '123');
  assert.equal(values.app_peak_bytes, '456');
  assert.equal(values.web_phys_bytes, '0');
  assert.equal(values.web_alive, '0');
});

testShell('same PID supplied twice shares its single observation', () => {
  const { values, calls } = sample('phys_footprint: 123\nphys_footprint_peak: 456', 0, String(process.pid));
  assert.equal(calls, 1);
  assert.equal(values.web_phys_bytes, values.app_phys_bytes);
  assert.equal(values.web_peak_bytes, values.app_peak_bytes);
});

testShell('distinct live PIDs receive independent single observations', () => {
  const { calls } = sample('phys_footprint: 123\nphys_footprint_peak: 456', 0, String(process.ppid));
  assert.equal(calls, 2);
});

for (const [output, status, expected] of [
  ['phys_footprint: 0\nphys_footprint_peak: 0', 0, ['0', '0']],
  ['phys_footprint: 123', 0, ['123', 'unknown']],
  ['phys_footprint: invalid\nphys_footprint_peak: 456', 0, ['unknown', '456']],
  ['', 0, ['unknown', 'unknown']],
  ['phys_footprint: 123\nphys_footprint_peak: 456', 1, ['unknown', 'unknown']],
]) {
  testShell(`missing/malformed/failed footprint is not measured zero: ${status}:${output}`, () => {
    const { values, calls } = sample(output, status);
    assert.equal(calls, 1);
    assert.deepEqual([values.app_phys_bytes, values.app_peak_bytes], expected);
    assert.equal(values.app_alive, '1');
  });
}


for (const [rss, status, expected] of [['0', 0, '0'], ['42', 0, '43008'], ['', 0, 'unknown'], ['bad', 0, 'unknown'], ['42', 1, 'unknown']]) {
  testShell(`RSS also preserves unknown sampling: ${status}:${rss}`, () => {
    assert.equal(sample('phys_footprint: 1', 0, '-', rss, status).values.app_rss_bytes, expected);
  });
}
