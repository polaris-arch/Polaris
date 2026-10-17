import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, mkdirSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { boundedDownload, DOWNLOAD_TIMEOUT_MS } from './lib/bounded-download.mjs';
import { verifyDashboardArchive, dashboardInventory, dashboardFresh } from './fetch-dashboard.mjs';
import { seedMatches } from './assert-r8-evidence.mjs';

test('D01 authentic bytes required; login HTML and same-size mutation rejected', () => {
  const bytes = Buffer.from('pinned bytes');
  const pin = {bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex')};
  verifyDashboardArchive(bytes, pin);
  assert.throws(() => verifyDashboardArchive(Buffer.from('<html>login</html>'), pin));
  const wrong = Buffer.from(bytes); wrong[0] ^= 1;
  assert.throws(() => verifyDashboardArchive(wrong, pin));
});
test('D01 cache checks nested bytes, additions, removals and links, not index existence', () => {
  const root = mkdtempSync(join(tmpdir(), 'dashboard-regression-'));
  try {
    writeFileSync(join(root, 'index.html'), 'html'); mkdirSync(join(root, 'assets'));
    writeFileSync(join(root, 'assets/app.js'), 'original');
    const before = dashboardInventory(root); assert.equal(dashboardFresh(root), false);
    writeFileSync(join(root, 'assets/app.js'), 'changed'); assert.notEqual(dashboardInventory(root), before);
    writeFileSync(join(root, 'assets/app.js'), 'original'); assert.equal(dashboardInventory(root), before);
    writeFileSync(join(root, 'extra'), 'x'); assert.notEqual(dashboardInventory(root), before);
    rmSync(join(root, 'extra')); rmSync(join(root, 'assets/app.js')); assert.notEqual(dashboardInventory(root), before);
    if (process.platform !== 'win32') {
      symlinkSync('missing', join(root, 'assets/app.js')); assert.throws(() => dashboardInventory(root));
    }
  } finally { rmSync(root, {recursive:true, force:true}); }
});
test('D02 bounded attempts, retry window and child; caller cannot expand hard timeout', () => {
  boundedDownload('https://example.invalid', {timeout:999999}, (cmd, args, options) => {
    assert.equal(cmd,'curl'); assert.equal(args[0],'--disable');
    for (const flag of ['--connect-timeout','--max-time','--speed-limit','--speed-time','--retry-max-time']) assert.ok(Number(args[args.indexOf(flag)+1])>0);
    assert.equal(options.timeout, DOWNLOAD_TIMEOUT_MS); assert.equal(options.killSignal,'SIGKILL');
  });
  assert.throws(() => boundedDownload('http://example.invalid'));
  assert.throws(() => boundedDownload('https://example.invalid',{},()=>{throw Error('timeout');}), /timeout/);
});
test('NA04 exact declaring class and full JNI overload; layout only normalized', () => {
  const owner='com.polaris2.app.RustWebView', expected=[owner,'java.lang.String getCookies(java.lang.String)'];
  assert.equal(seedMatches(`${owner}: java.lang.String getCookies( java.lang.String );`,expected),true);
  for (const row of [`${owner}$Nested: java.lang.String getCookies(java.lang.String)`, `prefix.${owner}: java.lang.String getCookies(java.lang.String)`, `${owner}: java.lang.String getCookies()`, `${owner}: void getCookies(java.lang.String)`]) assert.equal(seedMatches(row,expected),false);
});
