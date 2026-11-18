import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { fixtureTar, fixtureZip, writeFormalAssets, writeLinuxFixture } from './lib/formal-package-fixtures.mjs';
import { LOCAL_MARKER, rejectLocalDistribution, prepareLocalInstallation } from './desktop-core/local-development.mjs';
import { selectedCoreManifest, writeSelectedCoreManifest } from './lib/core-identity.mjs';
import { createHash } from 'node:crypto';
const hash = (value) => createHash('sha256').update(value).digest('hex');
function fixture(action) {
  const root = mkdtempSync(join(tmpdir(), 'polaris-local-selection-'));
  try { action(root); } finally { rmSync(root, { recursive: true, force: true }); }
}
test('Formal package and publisher reject local input slots, including a receipt without binaries', () => fixture((root) => {
  rejectLocalDistribution(root);
  mkdirSync(join(root, 'resources/.local-development/linux'), { recursive: true });
  writeFileSync(join(root, 'resources/.local-development/linux/selection.json'), '{}');
  assert.throws(() => rejectLocalDistribution(root), /Formal distribution rejects/);
}));
test('removing local receipts cannot turn a compiled local Helper into a Formal distribution', () => fixture((root) => {
  const stage = join(root, 'stage'); mkdirSync(stage);
  writeFileSync(join(stage, 'polaris-helper'), Buffer.concat([Buffer.from('owned-negative-fixture\0'), LOCAL_MARKER]));
  assert.throws(() => rejectLocalDistribution(root, stage), /LocalDevelopmentSelection executable/);
}));
test('an alternate directory cannot borrow local installation semantics', () => fixture((root) => {
  assert.throws(() => prepareLocalInstallation(root, join(root, 'resources/linux'), 'prepare'), /Fixed local development/);
}));
test('the existing final installation collector freezes actual Helper bytes without a CoreId self-reference', () => fixture((root) => {
  const core = Buffer.from('owned core fixture'), sidecar = Buffer.from('owned sidecar fixture');
  const contract = { version: '1.15.0-polaris-dev.control', platforms: { linux: { providerFingerprint: 'a'.repeat(64),
    core: { member: 'sing-box', bytes: core.length, sha256: hash(core) }, sidecar: { member: 'libcronet.so', bytes: sidecar.length, sha256: hash(sidecar) } } } };
  writeFileSync(join(root, 'sing-box'), core); writeFileSync(join(root, 'libcronet.so'), sidecar);
  const pure = selectedCoreManifest(contract, 'linux', '2.12.3');
  writeFileSync(join(root, 'polaris-helper'), 'first actual owned Helper');
  const first = writeSelectedCoreManifest(root, contract, 'linux', '2.12.3');
  writeFileSync(join(root, 'polaris-helper'), 'second actual owned Helper');
  const second = writeSelectedCoreManifest(root, contract, 'linux', '2.12.3');
  assert.equal(first.id, pure.id); assert.equal(second.id, pure.id);
  assert.notEqual(first.core.helperSha256, second.core.helperSha256);
  assert.equal(second.core.helperSha256, hash(Buffer.from('second actual owned Helper')));
}));

const receipt = fileURLToPath(new URL('./final-package-receipt.py', import.meta.url));
function inspect(asset, expectedError, env = process.env) {
  const { status, stdout, stderr } = spawnSync('python3', ['-B', receipt, '--reject-local-selection', asset],
    { encoding: 'utf8', timeout: 30_000, env });
  if (expectedError) {
    assert.equal(status, 1, stderr);
    assert.match(stderr, expectedError);
    return;
  }
  assert.equal(status, 0, stderr);
  return JSON.parse(stdout);
}

test('Linux formal packages keep case-distinct paths and bind the exact executable members', { skip: process.platform !== 'linux' }, () => fixture(root => {
  const app = 'inert namespace App', helper = 'inert namespace Helper';
  const upper = 'usr/bin/Polaris', lower = 'usr/bin/polaris';
  const helperPath = 'usr/lib/Polaris/_up_/resources/linux/polaris-helper';
  const stage = join(root, 'stage');
  for (const [name, data] of [[upper, app], [helperPath, helper], ['Polaris.png', 'icon'],
    ['icons/polaris.png', 'icon'], ['Share', 'case-distinct file'], ['share/config', 'case-distinct ancestor']]) {
    mkdirSync(join(stage, dirname(name)), { recursive: true });
    writeFileSync(join(stage, name), data);
  }
  symlinkSync('polaris', join(stage, 'usr/bin/alias'));
  symlinkSync('Polaris.png', join(stage, '.DirIcon'));
  symlinkSync('icons/polaris.png', join(stage, 'polaris.png'));
  for (const [target, error, marked] of [['Polaris'], ['POLARIS', /link target absent/],
    ['../../../outside', /link escapes archive/], ['alias', /link cycle/], ['Polaris', /LocalDevelopmentSelection executable/, true]]) {
    const link = join(stage, lower);
    rmSync(link, { force: true });
    symlinkSync(target, link);
    if (marked) {
      writeFileSync(join(stage, upper), Buffer.concat([Buffer.alloc(65530), LOCAL_MARKER]));
    }
    for (const ext of ['AppImage', 'deb']) {
      const asset = join(root, 'fixture.' + ext);
      writeLinuxFixture(stage, asset);
      const result = inspect(asset, error);
      if (error) continue;
      assert.equal(result.artifactSha256, hash(readFileSync(asset)));
      assert.deepEqual(Object.fromEntries(result.checkedExecutables.map(row => [row.member, row.sha256])),
        { [upper]: hash(app), [lower]: hash(app), [helperPath]: hash(helper) });
      assert.equal(result.expandedExecutableBytes, 2 * app.length + helper.length);
    }
  }

  // Duplicate tar records and hard links with absent targets cannot be staged on disk.
  writeFileSync(join(root, 'debian-binary'), '2.0\n');
  writeFileSync(join(root, 'control.tar.gz'), fixtureTar([{ name: 'control', data: readFileSync(join(stage, 'DEBIAN/control')) }]));
  const deb = join(root, 'members.deb');
  function tarMembers(rows, error) {
    writeFileSync(join(root, 'data.tar.gz'), fixtureTar(rows));
    execFileSync('ar', ['rcs', deb, 'debian-binary', 'control.tar.gz', 'data.tar.gz'], { cwd: root, timeout: 30_000 });
    return inspect(deb, error);
  }
  const base = [{ name: upper, data: app }, { name: helperPath, data: helper }];
  const link = (name, linkpath) => ({ name, type: 'Link', linkpath });
  assert.equal(tarMembers([...base, link(lower, upper)]).checkedExecutables.length, 3);
  for (const target of ['usr/bin/POLARIS', 'USR/bin/Polaris', '../../../outside', 'usr\\bin\\Polaris', '/usr/bin/Polaris']) {
    tarMembers([...base, link(lower, target)], target === 'usr/bin/POLARIS' || target === 'USR/bin/Polaris'
      ? /link target absent/ : /link escapes archive/);
  }
  tarMembers([base[1], link(lower, upper), link(upper, lower)], /link cycle/);
  tarMembers([...base, { name: upper }], /duplicate\/member count/);
  tarMembers([...base, { name: upper + '/child' }], /file\/directory alias collision/);
  for (const name of ['../outside', 'usr//bad', 'bad:name', 'bad\\name', 'bad\x01name', 'trailing.', 'CON', 'e\u0301']) {
    tarMembers([...base, { name }], /unsafe ZIP member|non-canonical ZIP path|Windows ZIP (?:path alias|device name)/);
  }

  const zip = join(root, 'windows.zip');
  writeFileSync(zip, fixtureZip([{ name: 'Polaris.exe', data: app }, { name: 'polaris.exe', data: app },
    { name: 'polaris-helper.exe', data: helper }]));
  inspect(zip, /duplicate\/alias member/);
}));

test('formal DMG selection reads HFS and APFS executable data forks through the carrier reader', { skip: process.platform === 'win32' }, () => fixture(root => {
  const directory = join(root, 'assets'), asset = join(directory, 'fixture.dmg');
  const env = writeFormalAssets(directory, ['fixture.dmg']);
  const reserved = 'Path = .HFS+ Private Directory Data\r\nFolder = +\nSize = ';
  const helper = 'Path = Contents/polaris-helper\nSize = 6\nMode = -rw-r--r--\nSymbolic Link = ';
  const backing = 'Path = Contents/target\nSize = 3';
  const files = { 'Contents/polaris-helper': 'helper', 'Contents/target': 'app' };
  function listing(records, contents = files, newline = '\n', type = 'HFS') {
    writeFileSync(asset, JSON.stringify({ files: contents, listing: ['Type = ' + type, '----------', records.join('\n\n'), ''].join('\n').replaceAll('\n', newline) }));
  }
  for (const [type, newline, dir, link, contents] of [
    ['HFS', '\n', 'Folder = +', '', { ...files, 'Contents/Polaris': 'target' }],
    ['APFS', '\r\n', 'Mode = drwxr-xr-x', '\nSymbolic Link = target', files],
  ]) {
    listing([reserved, 'Path = Contents\n' + dir + '\nSize = ', helper, backing,
      'Path = Contents/Polaris\nMode = lrwxr-xr-x\nSize = 6' + link], contents, newline, type);
    const result = inspect(asset, undefined, env);
    assert.deepEqual(Object.fromEntries(result.checkedExecutables.map(row => [row.member, row.sha256])),
      { 'Contents/Polaris': hash('app'), 'Contents/polaris-helper': hash('helper') });
    assert.equal(result.expandedExecutableBytes, type === 'HFS' ? 15 : 9);
  }
  for (const [record, error] of [
    ['Path = Polaris\nSize = ', /member size differs/],
    ['Path = Polaris\nSize = x', /member size differs/],
    ['Path = Polaris\nSize = 3\nPath = other', /duplicate formal 7-Zip property/],
    ['Path = Polaris\nSize = 3\ninjected newline', /invalid formal DMG property line/],
    ['Path = Polaris\nSize = 3\nMode = prw-------', /nonregular member/],
    ['Path = Polaris\nSize = 3\nFolder = +\nSymbolic Link = target', /directory\/link conflict/],
    ['Path = Polaris\nSize = 3\n\nPath = Polaris\nSize = 3', /duplicate member/],
    ['Path = .HFS+ Private Directory Data\r\nSize = 3', /invalid HFS metadata directory/],
    ['Path = Other\r\nSize = 0', /Windows ZIP path alias/],
    ['Path = Polaris\nSize = 0', /unresolved hard-linked/],
  ]) {
    listing([record, helper]);
    inspect(asset, error, env);
  }
  // Reserved filesystem records still consume the actual production entry budget.
  listing([...Array(10000).fill(reserved), helper]);
  inspect(asset, /member count budget/, env);
}));
