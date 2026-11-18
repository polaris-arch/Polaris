import test from 'node:test';
import assert from 'node:assert/strict';
import {
  appendFileSync, existsSync, fstatSync, linkSync, mkdtempSync, mkdirSync, readdirSync, readFileSync, readSync, realpathSync, rmSync, statSync,
  symlinkSync, truncateSync, writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { crc32 } from 'node:zlib';
import { inspectFormalArtifact, inspectPortableZip, verifyFormalArtifactBytes } from './lib/formal-package-selection.mjs';
import { fixtureTar, fixtureZip, writeFormalAssets, writeLinuxFixture } from './lib/formal-package-fixtures.mjs';
import { LOCAL_MARKER, compilerSelectionSource, localReceipt, rejectLocalDistribution, prepareLocalInstallation } from './desktop-core/local-development.mjs';
import { selectedCoreManifest, writeSelectedCoreManifest } from './lib/core-identity.mjs';
import { createHash } from 'node:crypto';
const hash = (value) => createHash('sha256').update(value).digest('hex');
async function fixture(action) {
  const root = mkdtempSync(join(realpathSync(tmpdir()), 'polaris-local-selection-'));
  try { await action(root); } finally { rmSync(root, { recursive: true, force: true }); }
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
test('local compiler pins bind the exact selection bytes and both source trees', () => {
  const coreSource = { commit: 'a'.repeat(40), tree: 'b'.repeat(40) };
  const producerSource = { commit: 'c'.repeat(40), tree: 'd'.repeat(40) };
  const contractSha256 = 'e'.repeat(64);
  const core = { bytes: 7, sha256: '1'.repeat(64) };
  const sidecar = { bytes: 11, sha256: '2'.repeat(64) };
  const document = { context: { coreSource, producerSource }, row: { target: 'linux/amd64', goVersion: 'go1.27.2',
    tags: 'with_purego', core, sidecar }, policy: { purposes: ['CheckConfig'] }, contractSha256 };
  const template = 'existing Rust definitions\r\npub const COMPILED:previous selection';
  const original = Buffer.from(JSON.stringify(document, null, 2) + '\n');
  const expected = 'existing Rust definitions\npub const COMPILED:Option<DevelopmentSelection>=Some(DevelopmentSelection{' +
    'source_commit:"' + coreSource.commit + '",source_tree:"' + coreSource.tree + '",producer_commit:"' + producerSource.commit +
    '",producer_tree:"' + producerSource.tree + '",target:"linux/amd64",go_version:"go1.27.2",tags:"with_purego",core_bytes:7,core_sha256:"' +
    core.sha256 + '",sidecar_bytes:11,sidecar_sha256:"' + sidecar.sha256 + '",selection_sha256:"' + hash(original) +
    '",contract_sha256:"' + contractSha256 + '"});\n';
  assert.equal(compilerSelectionSource(template, original), expected);
  document.policy.purposes.push('RunMain');
  const changed = Buffer.from(JSON.stringify(document, null, 2) + '\n');
  assert.equal(compilerSelectionSource(template, changed), expected.replace(hash(original), hash(changed)));
});
test('local receipts retain Python byte identity and reject altered authorities', () => {
  const coreSource = { commit: 'a'.repeat(40), tree: 'b'.repeat(40) };
  const producerSource = { commit: 'c'.repeat(40), tree: 'd'.repeat(40) };
  const context = { schema: 'polaris-local-development-execution-context-v1', coreSource, producerSource,
    host: { os: 'linux', arch: 'amd64' } };
  const core = { member: 'sing-box', bytes: 87724295, sha256: '1'.repeat(64) };
  const sidecar = { member: 'libcronet.so', bytes: 12370520, sha256: '2'.repeat(64) };
  const archive = { name: 'sing-box-test-linux-amd64.tar.gz', bytes: 456789, sha256: '3'.repeat(64),
    root: 'sing-box-test-linux-amd64',
    members: { 'sing-box-test-linux-amd64/sing-box': core.sha256, 'sing-box-test-linux-amd64/libcronet.so': sidecar.sha256 } };
  const recipe = { coreSource, producerSource, tags: ['with_purego'], goVersion: 'go1.27.2',
    version: '1.15.0-polaris-dev.control', target: 'linux/amd64', sidecar };
  const row = { version: recipe.version, target: recipe.target, sourceCommit: coreSource.commit, sourceTree: coreSource.tree,
    goVersion: recipe.goVersion, tags: recipe.tags.join(','), variant: {}, buildGraph: { settings: { GOOS: 'linux' }, normalModules: [] },
    rawBuildInfo: 'sing-box: go1.27.2\n', archive, core, sidecar, finalPackageCronetSmoke: { status: 'passed' } };
  const proof = { coreSeals: 15, sidecarSeals: 15, tlsProbe: { coreBytes: core.bytes,
    configurationChecks: [{ output: '连接\u007f🛠\t"\\\n', exitCode: -9, expectedPass: false }] } };
  const recipeSha256 = '0'.repeat(64);
  const document = { schema: 'polaris-local-development-selection-v1', configurationSha256: recipeSha256,
    context, row, proof, policy: { purposes: ['CheckConfig'] } };
  const template = 'existing Rust definitions\npub const COMPILED:previous selection';
  const facts = { recipe, recipeSha256 };
  const { selection, contract, compiled, providerFingerprint } = localReceipt(template, { ...facts, draft: JSON.stringify(document) });
  // Golden hashes from the previous Python producer's exact v1 JSON encoding.
  assert.equal(providerFingerprint, '5c784edfb284bbeb196c91f3b388b01d1241d3d2b10a4c95420edbc82a6bf7f6');
  assert.equal(hash(contract), '8fad6e2942564c7488060803f46076535cc98ec4ffd24e8982502614008b9e6c');
  assert.equal(hash(selection), 'ae44abeb39a82fccc0c63068a4749ab66d2d3f8103d8f28bbc939921b67fe89f');
  const verify = (input = {}) => localReceipt(compiled, { ...facts, selection, contract, ...input });
  assert.equal(verify(), null);
  assert.throws(() => verify({ selection: selection + '\n' }), /exact producer encoding/);
  assert.throws(() => verify({ contract: contract + '\n' }), /compiled contract differs/);
  assert.throws(() => verify({ selection: selection.replace(providerFingerprint, '4'.repeat(64)) }), /protected facts\/provider differs/);
  assert.throws(() => verify({ selection: selection.replace(producerSource.tree, '4'.repeat(40)) }), /source context differs/);
  assert.throws(() => verify({ selection: selection.replace('87724295,', '87724295.0,') }), /exact integer/);
  assert.throws(() => localReceipt(template, { ...facts, draft: JSON.stringify({ ...document, workflow: {} }) }), /selection shape differs/);
});
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

async function inspect(asset, expectedError) {
  const result = inspectFormalArtifact(asset);
  if (expectedError) {
    await assert.rejects(result, expectedError);
    return;
  }
  return result;
}

test('Linux formal packages keep case-distinct paths and bind the exact executable members', { skip: process.platform !== 'linux' }, () => fixture(async root => {
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
      const result = await inspect(asset, error);
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
  async function tarMembers(rows, error) {
    writeFileSync(join(root, 'data.tar.gz'), fixtureTar(rows));
    execFileSync('ar', ['rcs', deb, 'debian-binary', 'control.tar.gz', 'data.tar.gz'], { cwd: root, timeout: 30_000 });
    return inspect(deb, error);
  }
  const base = [{ name: upper, data: app }, { name: helperPath, data: helper }];
  const link = (name, linkpath) => ({ name, type: 'Link', linkpath });
  assert.equal((await tarMembers([...base, link(lower, upper)])).checkedExecutables.length, 3);
  const paxPath = 'usr/' + 'long-directory'.repeat(13) + '/Polaris';
  const pax = await tarMembers([{ name: paxPath, data: app }, base[1],
    { name: 'raw/backing', data: 'different ordinary file' },
    { ...link(lower, 'raw/backing'), pax: { linkpath: paxPath } }]);
  assert.deepEqual(Object.fromEntries(pax.checkedExecutables.map(row => [row.member, row.sha256])),
    { [paxPath]: hash(app), [lower]: hash(app), [helperPath]: hash(helper) });
  const global = await tarMembers([{ name: 'raw/app', data: app, globalPax: { path: upper } },
    { name: 'raw/helper', data: helper, pax: { path: helperPath } }]);
  assert.deepEqual(Object.fromEntries(global.checkedExecutables.map(row => [row.member, row.sha256])),
    { [upper]: hash(app), [helperPath]: hash(helper) });
  for (const target of ['usr/bin/POLARIS', 'USR/bin/Polaris', '../../../outside', 'usr\\bin\\Polaris', '/usr/bin/Polaris']) {
    await tarMembers([...base, link(lower, target)], target === 'usr/bin/POLARIS' || target === 'USR/bin/Polaris'
      ? /link target absent/ : /link escapes archive/);
  }
  await tarMembers([base[1], link(lower, upper), link(upper, lower)], /link cycle/);
  await tarMembers([...base, { name: upper }], /duplicate\/member count/);
  await tarMembers([...base, { name: upper + '/child' }], /file\/directory alias collision/);
  for (const name of ['../outside', 'usr//bad', 'bad:name', 'bad\\name', 'bad\x01name', 'trailing.', 'CON', 'e\u0301']) {
    await tarMembers([...base, { name }], /unsafe ZIP member|non-canonical ZIP path|Windows ZIP (?:path alias|device name)/);
  }
}));

test('compressed ZIP and APK selection checks executable bytes and archive paths', () => fixture(async root => {
  const markerError = /LocalDevelopmentSelection executable/;
  for (const [extension, app] of [['zip', 'polaris.exe'], ['apk', 'lib/arm64-v8a/libpolaris_lib.so']]) {
    const asset = join(root, 'fixture.' + extension);
    const helper = extension === 'zip' ? [{ name: 'polaris-helper.exe', data: 'helper', method: 8 }] : [];
    for (const marked of [false, true]) {
      // Use the bytes emitted by the Rust build, independently of the scanner's constant.
      const payload = Buffer.from(marked ? 'POLARIS_SELECTION_LOCAL_DEVELOPMENT_V1' : 'app');
      const data = Buffer.concat([Buffer.alloc(65530), payload]);
      writeFileSync(asset, fixtureZip([{ name: app, data, method: 8 }, ...helper]));
      const result = await inspect(asset, marked ? markerError : undefined);
      if (!marked) {
        assert.equal(result.checkedExecutables.length, 1 + helper.length);
        assert.equal(result.checkedExecutables.find(row => row.member === app).sha256, hash(data));
      }
    }
  }
  const zip = join(root, 'windows.zip');
  for (const [name, error] of [['Polaris.exe', /duplicate\/alias member/],
    ['../polaris.exe', /unsafe ZIP member|non-canonical ZIP path/],
    ['polaris.exe/../polaris.exe', /unsafe ZIP member|non-canonical ZIP path/],
    ['Polaris.exe.', /Windows ZIP path alias/]]) {
    writeFileSync(zip, fixtureZip([name, 'polaris.exe', 'polaris-helper.exe'].map(name => ({ name, data: 'entry' }))));
    await inspect(zip, error);
  }
  const bytes = fixtureZip([{ name: 'polaris.exe', data: 'app' }, { name: 'polaris-helper.exe', data: 'helper' }]);
  bytes[30 + 'polaris.exe'.length] ^= 1;
  writeFileSync(zip, bytes);
  await inspect(zip, /ZIP member CRC differs/);
  for (const flags of [1, 0x20, 0x40]) {
    writeFileSync(zip, fixtureZip([{ name: 'polaris.exe', data: 'app', flags }, { name: 'polaris-helper.exe', data: 'helper' }]));
    await inspect(zip, /encrypted\/patched member/);
  }
}));

test('portable ZIP readback verifies every byte and the exact frozen member inventory', () => fixture(async root => {
  const archive = join(root, 'windows-portable.zip');
  const payload = {
    'polaris.exe': 'original App', 'portable.marker': '便携版标记', 'core-manifest.json': '{}',
    LICENSE: 'license', NOTICE: 'notice', 'THIRD-PARTY-LICENSES.md': 'third party license',
    'resources/win/core-manifest.json': '{"core":true}', 'resources/win/core-release/source/runtime.sh': 'signed source',
    'resources/win/polaris-helper.exe': 'helper', 'resources/win/polaris-helper-setup.exe': 'cleaner',
    'resources/win/sing-box.exe': 'exact core', 'resources/win/libcronet.dll': 'release Cronet',
    'resources/dashboard/assets/current.js': 'dashboard', 'resources/data/geosite.srs': 'rule-set',
    'resources/许可证/straße.txt': 'Unicode path',
  };
  const expected = Object.fromEntries(Object.entries(payload).map(([name, data]) => [name, {
    bytes: Buffer.byteLength(data), sha256: hash(data),
  }]));
  const entries = Object.entries(payload).map(([name, data]) => ({ name, data, method: 8, flags: name.includes('许可证') ? 0x800 : 0 }));
  const write = (rows) => writeFileSync(archive, fixtureZip(rows));
  write([...entries, { name: 'resources/', data: '', mode: 0o040755 }]);
  const reader = fileURLToPath(new URL('./lib/formal-package-selection.mjs', import.meta.url));
  const output = execFileSync(process.execPath, [reader, 'portable-zip'], { input: JSON.stringify({ archive, expected }) });
  assert.deepEqual(JSON.parse(output), expected);
  for (const name of Object.keys(payload)) {
    write(entries.filter(row => row.name !== name));
    await assert.rejects(inspectPortableZip(archive, expected), /member set differs/, name);
    write(entries.map(row => row.name === name ? { ...row, data: Buffer.alloc(expected[name].bytes, 120) } : row));
    await assert.rejects(inspectPortableZip(archive, expected), /member SHA\/size differs/, name);
  }
  for (const name of ['resources/dashboard/screenshot.png', 'old-debug.ps1', 'resources', 'POLARIS.EXE', 'polaris.exe.',
    'resources//win/extra', 'resources\\win\\extra', '../outside', 'unknown/', 'Resources/', 'resources/COM².txt',
    'resources/许可证/STRASSE.txt', 'resources/e\u0301.txt', 'resources/a\0b.txt']) {
    write([...entries, { name, data: '', flags: 0x800 }]);
    await assert.rejects(inspectPortableZip(archive, expected), /member|directory|ZIP path|device name/, name);
  }
  for (const row of [{ ...entries[0] }, { name: 'resources/', data: 'nonzero', mode: 0o040755 },
    { name: 'unexpected-link', data: 'polaris.exe', mode: 0o120777 }, { name: 'special', data: '', mode: 0o010644 }]) {
    write([...entries, row]);
    await assert.rejects(inspectPortableZip(archive, expected), /duplicate entry|unregistered ZIP directory|nonregular member/, row.name);
  }
  const crcMismatch = fixtureZip([{ name: 'LICENSE', data: 'license' }]);
  crcMismatch[30 + 'LICENSE'.length] ^= 1;
  writeFileSync(archive, crcMismatch);
  await assert.rejects(inspectPortableZip(archive, { LICENSE: expected.LICENSE }), /ZIP member CRC differs/);
  const alias = join(root, 'hardlink.zip');
  linkSync(archive, alias);
  await assert.rejects(inspectPortableZip(archive, { LICENSE: expected.LICENSE }), /single-link|byte budget or regular file/);
}));

test('portable ZIP readback rejects alternate entry names and enforces three byte and count bounds', () => fixture(async root => {
  const zip = join(root, 'limits.zip');
  const entry = { name: 'NOTICE', data: 'notice' };
  const expected = { NOTICE: { bytes: 6, sha256: hash(entry.data) } };
  const original = fixtureZip([entry]);
  const eocd = original.length - 22, central = original.readUInt32LE(eocd + 16);
  const unicode = Buffer.concat([Buffer.from([0x75, 0x70, 10, 0, 1]), Buffer.alloc(4), Buffer.from('ALIAS')]);
  unicode.writeUInt32LE(crc32(Buffer.from(entry.name)), 5);
  original.writeUInt16LE(unicode.length, central + 30);
  original.writeUInt32LE(original.readUInt32LE(eocd + 12) + unicode.length, eocd + 12);
  writeFileSync(zip, Buffer.concat([original.subarray(0, eocd), unicode, original.subarray(eocd)]));
  await assert.rejects(inspectPortableZip(zip, expected), /Unicode path differs from original/);

  const differingEncoding = fixtureZip([{ name: 'é', data: '', flags: 0x800 }]);
  differingEncoding.writeUInt16LE(0, 6);
  writeFileSync(zip, differingEncoding);
  await assert.rejects(inspectPortableZip(zip, {}), /local and central headers differ/);

  const limit = 512 * 1024 ** 2;
  writeFileSync(zip, fixtureZip([entry]));
  truncateSync(zip, limit + 1);
  await assert.rejects(inspectPortableZip(zip, expected), /byte budget/);
  const oversized = (count, bytes) => {
    const data = fixtureZip(Array.from({ length: count }, (_, i) => ({ name: `file-${i}`, data: '' })));
    let at = data.readUInt32LE(data.length - 22 + 16);
    for (let i = 0; i < count; i++) {
      data.writeUInt32LE(bytes, at + 24);
      at += 46 + data.readUInt16LE(at + 28);
    }
    return data;
  };
  writeFileSync(zip, oversized(1, limit + 1));
  await assert.rejects(inspectPortableZip(zip, { 'file-0': {} }), /oversized ZIP member/);
  writeFileSync(zip, oversized(5, limit));
  await assert.rejects(inspectPortableZip(zip, {}), /expanded budget/);
  writeFileSync(zip, oversized(10001, 0));
  await assert.rejects(inspectPortableZip(zip, {}), /entry limit/);
}));

test('the formal carrier snapshot rejects growth and expiry before writing the next block', () => fixture(async root => {
  const app = Buffer.alloc(65536 + 17, 97), helper = Buffer.from('ordinary Helper');
  const bytes = fixtureZip([{ name: 'polaris.exe', data: app }, { name: 'polaris-helper.exe', data: helper }]);
  for (const fault of ['none', 'growth', 'deadline']) {
    const asset = join(root, fault + '.zip');
    writeFileSync(asset, bytes);
    let snapshot, observed, reads = 0;
    const result = inspectFormalArtifact(asset, {
      retainedPath(_fd, temporary) { snapshot = join(temporary, 'carrier.zip'); return null; },
      clock: () => fault === 'deadline' && reads === 2 ? 180_001 : 0,
      readFrom(fd, offset, requested) {
        const buffer = Buffer.alloc(requested);
        const count = readSync(fd, buffer, 0, requested, offset);
        reads++;
        if (reads === 1 && fault === 'growth') appendFileSync(asset, Buffer.alloc(65536));
        if (reads === 2) observed = statSync(snapshot).size;
        return buffer.subarray(0, count);
      },
    });
    if (fault === 'none') {
      const checked = await result;
      assert.equal(checked.artifactSha256, hash(bytes));
      assert.deepEqual(checked.checkedExecutables.map(row => row.sha256), [hash(app), hash(helper)]);
      verifyFormalArtifactBytes(asset, checked);
      writeFileSync(asset, Buffer.from(bytes.map((byte, index) => index === 0 ? byte ^ 1 : byte)));
      assert.throws(() => verifyFormalArtifactBytes(asset, checked), /differs from distribution bytes/);
    } else {
      await assert.rejects(result, fault === 'growth' ? /snapshot byte budget exceeded/ : /inspection deadline exceeded/);
      assert.equal(reads, 2);
    }
    assert.equal(observed, 65536);
    assert.equal(existsSync(snapshot), false);
  }
}));

test('formal carrier scan binds the opened identity and compares like stat views', () => fixture(async root => {
  const app = Buffer.from('inert ordinary App'), helper = Buffer.from('inert ordinary Helper');
  const bytes = fixtureZip([{ name: 'polaris.exe', data: app }, { name: 'polaris-helper.exe', data: helper }]);
  const asset = join(root, 'carrier.zip');
  writeFileSync(asset, bytes);
  for (const fault of ['none', 'ino', 'size', 'ctimeNs', 'mtimeNs', 'nlink']) {
    let calls = 0;
    const result = inspectFormalArtifact(asset, {
      openedStat(fd) {
        calls++;
        const value = fstatSync(fd, { bigint: true });
        // Windows pathname and descriptor views can project different ctime.
        if (process.platform === 'win32') value.ctimeNs += 100n;
        const fieldIsInitial = fault === 'ino' || fault === 'size';
        if (fault !== 'none' && calls === (fieldIsInitial ? 1 : 2)) value[fault]++;
        return value;
      },
    });
    const badOpen = ['ino', 'size'].includes(fault);
    if (fault === 'none') {
      const checked = await result;
      assert.equal(checked.artifactSha256, hash(bytes));
      assert.deepEqual(checked.checkedExecutables.map(row => row.sha256), [hash(app), hash(helper)]);
    } else {
      await assert.rejects(result, badOpen ? /replaced before open/ : /changed during actual object check/);
    }
    assert.equal(calls, badOpen ? 1 : 2);
  }
}));

test('interrupting a formal ZIP scan discards its private snapshot', { skip: process.platform === 'win32' }, () => fixture(root => {
  const asset = join(root, 'carrier.zip'), name = 'polaris.exe';
  writeFileSync(asset, fixtureZip([{ name, data: Buffer.alloc(8 * 1024 ** 2), method: 8 },
    { name: 'polaris-helper.exe', data: 'ordinary Helper' }]));
  const source = new URL('./lib/formal-package-selection.mjs', import.meta.url).href;
  const read = spawnSync(process.execPath, ['--input-type=module', '-e', `
    import { readSync } from 'node:fs';
    import { inspectFormalArtifact } from ${JSON.stringify(source)};
    await inspectFormalArtifact(process.argv[1], {
      retainedPath: () => null,
      readFrom(fd, offset, length, remaining) {
        remaining();
        if (offset === ${30 + name.length}) process.kill(process.pid, 'SIGINT');
        const block = Buffer.alloc(length);
        return block.subarray(0, readSync(fd, block, 0, length, offset));
      },
    });
  `, asset], { env: { ...process.env, TMPDIR: root }, encoding: 'utf8', timeout: 10_000 });
  assert.ifError(read.error);
  assert.equal(read.status, 1, read.stderr);
  assert.match(read.stderr, /AbortError/);
  assert.deepEqual(readdirSync(root), ['carrier.zip']);
}));

test('formal DMG and NSIS selection reads executable bytes through the carrier reader', { skip: process.platform === 'win32' }, () => fixture(async root => {
  const directory = join(root, 'assets'), asset = join(directory, 'fixture.dmg');
  const env = writeFormalAssets(directory, ['fixture.dmg', 'fixture.exe']);
  const originalPath = process.env.PATH;
  process.env.PATH = env.PATH;
  try {
    for (const path of [asset, join(directory, 'fixture.exe')]) {
      const carrier = JSON.parse(readFileSync(path, 'utf8'));
      assert.equal((await inspect(path)).checkedExecutables.length, 2);
      const appName = Object.keys(carrier.files).find(name => name.endsWith('polaris') || name.endsWith('polaris.exe'));
      carrier.files[appName] = LOCAL_MARKER.toString();
      writeFileSync(path, JSON.stringify(carrier));
      await inspect(path, /LocalDevelopmentSelection executable/);
    }
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
      const result = await inspect(asset);
      assert.deepEqual(Object.fromEntries(result.checkedExecutables.map(row => [row.member, row.sha256])),
        { 'Contents/Polaris': hash('app'), 'Contents/polaris-helper': hash('helper') });
      assert.equal(result.expandedExecutableBytes, type === 'HFS' ? 15 : 9);
    }
    const inner = JSON.stringify({ kind: 'HFS', files: { 'Contents/Polaris': 'app', 'Contents/polaris-helper': 'helper' } });
    writeFileSync(asset, JSON.stringify({ kind: 'Dmg', files: { 'volume.hfs': inner } }));
    const wrapped = await inspect(asset);
    assert.equal(wrapped.expandedExecutableBytes, Buffer.byteLength(inner) + 9);
    assert.deepEqual(wrapped.checkedExecutables.map(row => row.sha256), [hash('app'), hash('helper')]);
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
      await inspect(asset, error);
    }
    // Reserved filesystem records still consume the actual production entry budget.
    listing([...Array(10000).fill(reserved), helper]);
      await inspect(asset, /member count budget/);
  } finally {
    process.env.PATH = originalPath;
  }
}));
