import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, cpSync, statSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { cronetFixture, fixtureManifest } from './lib/cronet-payload.test.mjs';

const repo = fileURLToPath(new URL('../', import.meta.url));
const verifier = join(repo, 'scripts/verify-packaging.mjs');
const count = Number(readFileSync(join(repo, 'src-tauri/build.rs'), 'utf8')
  .match(/^const EXPECTED_SRS_COUNT: usize = (\d+);$/m)?.[1]);
assert.ok(count > 0, 'fixture count must come from the actual build source');

// Inert staging members exercise the actual inventory CLI, not production packages.
function fixture(t, label, extra = []) {
  const root = mkdtempSync(join(tmpdir(), 'polaris-cleaner-inventory-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const core = label === 'windows' ? 'win' : 'linux';
  const suffix = label === 'windows' ? '.exe' : '';
  const members = [
    'core-manifest.json', '_up_/LICENSE', '_up_/NOTICE', '_up_/THIRD-PARTY-LICENSES.md',
    ...Array.from({ length: count }, (_, i) => `_up_/resources/data/fixture-${i}.srs`),
    '_up_/resources/dashboard/index.html', '_up_/resources/dashboard/assets/fixture.js',
    '_up_/resources/dashboard/licenses/fixture.txt', '_up_/resources/dashboard/favicon.ico',
    '_up_/resources/dashboard/favicon.svg', '_up_/resources/dashboard/apple-touch-icon-180x180.png',
    `_up_/resources/${core}/sing-box${suffix}`, `_up_/resources/${core}/polaris-helper${suffix}`,
    `_up_/resources/${core}/libcronet.${label === 'windows' ? 'dll' : 'so'}`, ...extra,
  ];
  const scopes = label === 'windows' ? [root] : [join(root, 'deb/usr/lib/Polaris'), join(root, 'appimage/Polaris.AppDir/usr/lib/Polaris')];
  for (const scope of scopes) for (const member of members) {
    const path = join(scope, member);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, 'inert inventory fixture');
  }
  const result = spawnSync(process.execPath, [verifier, 'inventory', '--label', label, '--root', root], { encoding: 'utf8' });
  return { code: result.status, output: result.stdout + result.stderr };
}
const cleaner = '_up_/resources/win/polaris-cleaner.exe';

test('actual Windows inventory admits exactly the fixed cleaner artifact', (t) => {
  const r = fixture(t, 'windows', [cleaner]);
  assert.equal(r.code, 0, r.output);
  assert.match(r.output, /native-cleaner=1/);
});
test('actual Windows inventory requires the cleaner even when helper and service payload are present', (t) => {
  const r = fixture(t, 'windows');
  assert.equal(r.code, 1, r.output);
  assert.match(r.output, /native-cleaner.*恰 1 份.*实为 0 份/);
});
for (const member of ['_up_/resources/win/unknown.exe', '_up_/resources/win/polaris-cleaner-backup.exe', '_up_/resources/win/nested/polaris-cleaner.exe']) {
  test(`actual Windows inventory rejects extra executable: ${member}`, (t) => {
    const r = fixture(t, 'windows', [cleaner, member]);
    assert.equal(r.code, 1, r.output);
    assert.ok(r.output.includes(member), r.output);
    assert.match(r.output, /未登记/);
  });
}
test('renamed cleaner cannot satisfy its required inventory slot', (t) => {
  const r = fixture(t, 'windows', ['_up_/resources/win/polaris-cleaner.EXE']);
  assert.equal(r.code, 1, r.output);
  assert.match(r.output, /native-cleaner.*实为 0 份/);
  assert.match(r.output, /未登记/);
});
test('Linux inventory remains valid without a Windows cleaner', (t) => {
  const r = fixture(t, 'linux');
  assert.equal(r.code, 0, r.output);
});
test('Windows cleaner never becomes an allowed Linux payload', (t) => {
  const r = fixture(t, 'linux', ['_up_/resources/linux/polaris-cleaner.exe']);
  assert.equal(r.code, 1, r.output);
  assert.match(r.output, /未登记/);
});

function windowsPayloadFixture(t, options = {}) {
  const root = mkdtempSync(join(tmpdir(), 'polaris-cronet-payload-cli-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  cpSync(join(repo, 'scripts'), join(root, 'scripts'), { recursive: true, filter: (p) => !p.includes('node_modules') });
  mkdirSync(join(root, 'src-tauri'));
  const bytes = cronetFixture('win', options);
  const manifest = { ...fixtureManifest(bytes), coreArchiveSha256: { win: 'fixture' } };
  writeFileSync(join(root, 'src-tauri/core-manifest.json'), JSON.stringify(manifest));
  const source = join(root, 'resources/win'), staging = join(root, 'staging/_up_/resources/win');
  mkdirSync(source, { recursive: true }); mkdirSync(staging, { recursive: true });
  for (const name of ['sing-box.exe', 'polaris-helper.exe', 'polaris-cleaner.exe', 'libcronet.dll']) {
    const content = name === 'libcronet.dll' ? bytes : Buffer.from(name);
    writeFileSync(join(source, name), content); writeFileSync(join(staging, name), content);
  }
  const run = () => { const result = spawnSync(process.execPath, [join(root, 'scripts/verify-packaging.mjs'), 'payload', '--label', 'windows', '--root', join(root, 'staging')], { encoding: 'utf8' }); return { code: result.status, output: result.stdout + result.stderr }; };
  return { root, source: join(source, 'libcronet.dll'), payload: join(staging, 'libcronet.dll'), bytes, run };
}

test('actual Windows payload CLI closes same-size Cronet substitution', (t) => {
  const f = windowsPayloadFixture(t); let result = f.run(); assert.equal(result.code, 0, result.output);
  const size = statSync(f.payload).size, changed = Buffer.from(f.bytes); changed[0x1200] ^= 1; writeFileSync(f.payload, changed);
  assert.equal(statSync(f.payload).size, size);
  result = f.run(); assert.equal(result.code, 1, result.output); assert.match(result.output, /final SHA-256/);
});

for (const fault of ['source replacement', 'symlink', 'missing pin', 'wrong architecture', 'unknown dependency', 'missing exported function']) {
  test(`actual Windows payload CLI rejects ${fault}`, (t) => {
    const options = fault === 'wrong architecture' ? { machine: 0xaa64 } : fault === 'unknown dependency' ? { dependency: 'injected.dll' } : fault === 'missing exported function' ? { missingExport: 'Cronet_Engine_Create' } : {};
    const f = windowsPayloadFixture(t, options);
    if (fault === 'source replacement') { const changed = Buffer.from(f.bytes); changed[0x1200] ^= 1; writeFileSync(f.source, changed); writeFileSync(f.payload, changed); }
    if (fault === 'symlink') { rmSync(f.payload); symlinkSync(f.source, f.payload); }
    if (fault === 'missing pin') writeFileSync(join(f.root, 'src-tauri/core-manifest.json'), JSON.stringify({ coreArchiveSha256: { win: 'fixture' } }));
    const result = f.run(); assert.equal(result.code, 1, result.output); assert.match(result.output, /Cronet|cronetLibrarySha256/);
  });
}
