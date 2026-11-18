import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, mkdirSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { boundedDownload, DOWNLOAD_TIMEOUT_MS } from './lib/bounded-download.mjs';
import { verifyDashboardArchive, dashboardInventory, dashboardFresh } from './fetch-dashboard.mjs';
import { inspectAppleArchitecture } from './mobile-core/apple-admission.mjs';

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

test('Apple architecture admission rechecks the explicit extension and retains sanitized observations', () => {
  const app = 'polaris_iOS';
  const extension = 'polaris_PacketTunnel';
  const minimum = '17.0';
  for (const [slot, sdk] of [['device-arm64', 'iphoneos'], ['sim-arm64', 'iphonesimulator']]) {
    const row = (target, settings = {}) => ({ target, buildSettings: {
      ARCHS: 'arm64', VALID_ARCHS: 'arm64', EXCLUDED_ARCHS: target === app && sdk === 'iphoneos' ? 'x86_64' : '',
      PLATFORM_NAME: sdk, SDKROOT: sdk, CONFIGURATION: 'release', IPHONEOS_DEPLOYMENT_TARGET: minimum, ...settings,
    } });
    const inspect = (rows, query, scheme) =>
      inspectAppleArchitecture(Buffer.from(JSON.stringify(rows)), { slot, query, minimum, scheme });
    const appRow = row(app);
    const extensionRow = row(extension);
    delete extensionRow.buildSettings.EXCLUDED_ARCHS;
    const scheme = inspect([appRow], 'scheme');
    const result = inspect([extensionRow], 'extension', scheme.admission);
    assert.deepEqual(Object.keys(result.admission), [app, extension]);
    assert.deepEqual(result.admission[extension].effectiveArchitectures, ['arm64']);
    assert.deepEqual(result.observation.rows[0].architectureSettings.EXCLUDED_ARCHS,
      { present: false, type: 'NoneType' });
    const combined = inspect([extensionRow, appRow], 'scheme').admission;
    assert.deepEqual(inspect([extensionRow], 'extension', combined).admission, result.admission);
    assert.equal(inspect([row(extension, { EXCLUDED_ARCHS: 'arm64e' })], 'extension', combined).error,
      'scheme and explicit extension settings differ');
    const raw = Buffer.from(JSON.stringify([row(app, { ARCHS: '/withheld', SECRET: 'redacted' })]));
    const rejected = inspectAppleArchitecture(raw, { slot, query: 'scheme', minimum });
    assert.deepEqual(rejected.observation.normalizedResponseText,
      { bytes: raw.length, sha256: createHash('sha256').update(raw).digest('hex') });
    assert.equal(rejected.observation.rows[0].architectureSettings.ARCHS.valueWithheld, true);
    assert.doesNotMatch(JSON.stringify(rejected), /\/withheld|redacted/);
    assert.match(inspect([row(extension, { ARCHS: 'arm64 arm64' })], 'extension', rejected.admission).error,
      /duplicate-token/);
    assert.match(inspect([extensionRow], 'extension', rejected.admission).error, /unknown-token/);
  }
});
