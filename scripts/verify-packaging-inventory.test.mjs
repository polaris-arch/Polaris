import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, copyFileSync, cpSync, readdirSync, statSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { cronetFixture, fixtureManifest } from './lib/cronet-payload.test.mjs';
import { assetNames } from './release-assets.mjs';
import { selectedCoreManifest } from './lib/core-identity.mjs';
import { createHash } from 'node:crypto';
import { hasDesktopRelease, loadReleaseContract, releasePayloadMembers } from './desktop-core/release-consumer.mjs';
import { hasTemporaryDesktop, loadTemporaryDesktopSelection, loadCandidate } from './desktop-core/native-dns-candidate.mjs';

const repo = fileURLToPath(new URL('../', import.meta.url));
const verifier = join(repo, 'scripts/verify-packaging.mjs');
const count = Number(readFileSync(join(repo, 'src-tauri/build.rs'), 'utf8')
  .match(/^const EXPECTED_SRS_COUNT: usize = (\d+);$/m)?.[1]);
assert.ok(count > 0, 'fixture count must come from the actual build source');

// Inert staging members exercise the actual inventory CLI, not production packages.
function fixture(t, label, extra = [], mutate = () => {}, selectionRepo = repo) {
  const root = mkdtempSync(join(tmpdir(), 'polaris-cleaner-inventory-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const core = label === 'windows' ? 'win' : 'linux';
  const suffix = label === 'windows' ? '.exe' : '';
  const manifest = JSON.parse(readFileSync(join(selectionRepo, 'src-tauri/core-manifest.json')));
  const temporary = hasTemporaryDesktop(manifest);
  if (temporary) loadTemporaryDesktopSelection(selectionRepo, manifest);
  const row = temporary ? loadCandidate(selectionRepo).platforms[core] : null;
  const legalMembers = row ? Object.keys(row.archive.members).map((name) => name.slice(row.archive.root.length + 1))
    .filter((name) => name !== row.core.member && name !== row.sidecar?.member)
    : hasDesktopRelease(manifest) ? releasePayloadMembers(loadReleaseContract(selectionRepo, manifest), core) : [];
  const members = [
    'core-manifest.json', '_up_/LICENSE', '_up_/NOTICE', '_up_/THIRD-PARTY-LICENSES.md',
    ...Array.from({ length: count }, (_, i) => `_up_/resources/data/fixture-${i}.srs`),
    '_up_/resources/dashboard/index.html', '_up_/resources/dashboard/assets/fixture.js',
    '_up_/resources/dashboard/licenses/fixture.txt', '_up_/resources/dashboard/favicon.ico',
    '_up_/resources/dashboard/favicon.svg', '_up_/resources/dashboard/apple-touch-icon-180x180.png',
    `_up_/resources/${core}/core-manifest.json`,
    ...(temporary ? [`_up_/resources/${core}/candidate-carrier.json`] : []),
    `_up_/resources/${core}/sing-box${suffix}`, `_up_/resources/${core}/polaris-helper${suffix}`,
    `_up_/resources/${core}/libcronet.${label === 'windows' ? 'dll' : 'so'}`,
    ...legalMembers.map((member) => `_up_/resources/${core}/${member}`), ...extra,
  ];
  const scopes = label === 'windows' ? [root] : [join(root, 'deb/usr/lib/Polaris'), join(root, 'appimage/Polaris.AppDir/usr/lib/Polaris')];
  for (const scope of scopes) for (const member of members) {
    const path = join(scope, member);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, 'inert inventory fixture');
  }
  mutate(root);
  const result = spawnSync(process.execPath, [join(selectionRepo, 'scripts/verify-packaging.mjs'), 'inventory', '--label', label, '--root', root], { encoding: 'utf8' });
  return { code: result.status, output: result.stdout + result.stderr };
}
const cleaner = '_up_/resources/win/polaris-cleaner.exe';

function temporaryFixture(t, label, extra = [], mutate = () => {}) {
  const selectionRepo = mkdtempSync(join(tmpdir(), 'historical-temporary-inventory-'));
  t.after(() => rmSync(selectionRepo, { recursive: true, force: true }));
  // Copy only the CLI's module dependencies and exact historical binding bytes.
  // These inert inventory fixtures neither acquire nor execute a native payload.
  for (const path of [
    'scripts/verify-packaging.mjs', 'scripts/postprocess-appimage.mjs', 'scripts/release-assets.mjs',
    'scripts/lib/polaris-version.json',
    ...['cronet-payload', 'cronet-contract', 'nsis-core-sweep', 'nsis-helper-cleanup', 'core-identity', 'macos-open-guide']
      .map(name => `scripts/lib/${name}.mjs`),
    ...['source-graph', 'bundle', 'release-consumer', 'native-dns-candidate', 'upstream-release', 'validation-origin', 'native-dns-gates']
      .map(name => `scripts/desktop-core/${name}.mjs`),
    'scripts/desktop-core/native-dns-candidate.json', 'scripts/desktop-core/native-dns-source-binding.json',
    'src-tauri/build.rs', 'src-tauri/tauri.windows.conf.json',
  ]) {
    const destination = join(selectionRepo, path);
    mkdirSync(dirname(destination), { recursive: true });
    copyFileSync(join(repo, path), destination);
  }
  const current = JSON.parse(readFileSync(join(repo, 'src-tauri/core-manifest.json')));
  const manifest = { desktopTemporary: current.historicalDesktopTemporary };
  writeFileSync(join(selectionRepo, 'src-tauri/core-manifest.json'), JSON.stringify(manifest));
  assert.equal(loadTemporaryDesktopSelection(selectionRepo).sourceCommit, 'fe182431495711c09bdb3b074ed6aa6309c00cf8');
  return fixture(t, label, extra, mutate, selectionRepo);
}

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
  const pin = name => { const value = readFileSync(join(source, name)); return { member: name, bytes: value.length, sha256: createHash('sha256').update(value).digest('hex') }; };
  const installation = JSON.stringify(selectedCoreManifest({ version: '1.0.0-fixture', platforms: { win: {
    providerFingerprint: 'a'.repeat(64), core: pin('sing-box.exe'), sidecar: pin('libcronet.dll'),
  } } }, 'win', '1.0.0-fixture', pin('polaris-helper.exe').sha256, pin('polaris-cleaner.exe').sha256));
  for (const directory of [source, staging]) writeFileSync(join(directory, 'core-manifest.json'), installation);
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

// S02: complete valid fixture plus unseen members must never become a green inventory.
test('depth 25 hard-fails instead of silently discarding unregistered content', t => {
  const member = `${Array(25).fill('deep').join('/')}/unregistered.txt`;
  const r = fixture(t, 'windows', [cleaner, member]);
  assert.equal(r.code, 1, r.output);
  assert.match(r.output, /depth limit 24 exceeded/);
});
test('depth 24 is traversed and reported as an unregistered resource', t => {
  const member = `_up_/${Array(23).fill('deep').join('/')}/unregistered.txt`;
  const r = fixture(t, 'windows', [cleaner, member]);
  assert.equal(r.code, 1, r.output);
  assert.doesNotMatch(r.output, /depth limit/);
  assert.ok(r.output.includes(member), r.output);
});
test('symlink ancestor cycle hard-fails without hiding the loop', { skip: process.platform === 'win32' }, t => {
  const r = fixture(t, 'windows', [cleaner], root => symlinkSync(root, join(root, 'loop')));
  assert.equal(r.code, 1, r.output);
  assert.match(r.output, /symlink cycle/);
});
test('separate directory aliases are enumerated rather than globally deduplicated', { skip: process.platform === 'win32' }, t => {
  let aliasedMembers;
  const r = fixture(t, 'windows', [cleaner], root => {
    const original = join(root, '_up_/resources/win');
    aliasedMembers = readdirSync(original, { recursive: true, withFileTypes: true }).filter(entry => entry.isFile()).length;
    symlinkSync(original, join(root, '_up_/alias'));
  });
  assert.equal(r.code, 1, r.output);
  // All aliased members, including the cleaner, must be counted. The diagnostic
  // prints only its first 60 paths, so a large legal inventory can hide its name.
  assert.equal(Number(/包内容白名单：(\d+) 个文件\*\*未登记/.exec(r.output)?.[1]), aliasedMembers, r.output);
  assert.match(r.output, /_up_\/alias\/core-manifest\.json/);
  assert.doesNotMatch(r.output, /实为 0 份/, 'the original directory must still satisfy every required member');
  assert.doesNotMatch(r.output, /symlink cycle/);
});

// G-R1: the real assets caller scans bundle/, including deep retained staging.
function linuxAssetFixture(t, extraDepth = 0) {
  const root = mkdtempSync(join(tmpdir(), 'polaris-linux-assets-depth-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const version = JSON.parse(readFileSync(join(repo, 'src-tauri/tauri.conf.json'), 'utf8')).version;
  for (const name of assetNames(version, 'linux')) {
    const target = join(root, name.endsWith('.deb') ? 'deb' : 'appimage', name);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, 'inert asset bytes');
  }
  for (const staging of [
    'deb/Polaris-package/data/usr/lib/Polaris/_up_/resources/dashboard/assets/main.js',
    'appimage/Polaris.AppDir/usr/lib/Polaris/_up_/resources/dashboard/assets/main.js',
  ]) {
    const target = join(root, staging);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, 'inert staging resource');
  }
  if (extraDepth) {
    const target = join(root, ...Array(extraDepth).fill('deep'), 'member.txt');
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, 'inert depth boundary member');
  }
  const result = spawnSync(process.execPath,
    [verifier, 'assets', '--label', 'linux', '--dir', root], { encoding: 'utf8' });
  return { code: result.status, output: result.stdout + result.stderr };
}
test('Linux assets gate accepts realistic deep deb and AppDir staging', t => {
  const result = linuxAssetFixture(t);
  assert.equal(result.code, 0, result.output);
  assert.match(result.output, /verify-packaging assets/);
});
test('Linux assets gate visits its explicit depth 24 boundary', t => {
  const result = linuxAssetFixture(t, 24);
  assert.equal(result.code, 0, result.output);
});
test('Linux assets gate still fails closed beyond its depth 24 budget', t => {
  const result = linuxAssetFixture(t, 25);
  assert.equal(result.code, 1, result.output);
  assert.match(result.output, /depth limit 24 exceeded/);
});


test('temporary default inventory requires its exact carrier receipt beside compiled identity', (t) => {
  assert.equal(hasTemporaryDesktop(JSON.parse(readFileSync(join(repo, 'src-tauri/core-manifest.json')))), false);
  const valid = temporaryFixture(t, 'windows', [cleaner]);
  assert.equal(valid.code, 0, valid.output);
  assert.match(valid.output, /temporary-app-carrier=1/);
  const missing = temporaryFixture(t, 'windows', [cleaner], root => rmSync(join(root, '_up_/resources/win/candidate-carrier.json')));
  assert.equal(missing.code, 1, missing.output);
  assert.match(missing.output, /temporary-app-carrier.*实为 0 份/);
});
for (const member of ['other-carrier.json', 'candidate-carrier.JSON', 'nested/candidate-carrier.json', '.native-dns-candidate/receipt.json']) {
  test(`temporary carrier does not allow arbitrary metadata: ${member}`, t => {
    const result = temporaryFixture(t, 'windows', [cleaner, `_up_/resources/win/${member}`]);
    assert.equal(result.code, 1, result.output); assert.match(result.output, /未登记/);
  });
}

test('temporary default keeps exact original legal member paths without formal-release relabeling', t => {
  const result = temporaryFixture(t, 'windows', [cleaner], root => rmSync(join(root, '_up_/resources/win/TLS-DEPENDENCY-NOTICES.txt')));
  assert.equal(result.code, 1, result.output);
  assert.match(result.output, /release-member:TLS-DEPENDENCY-NOTICES.txt.*实为 0 份/);
  const relabeled = temporaryFixture(t, 'windows', [cleaner, '_up_/resources/win/core-release/TLS-DEPENDENCY-NOTICES.txt']);
  assert.equal(relabeled.code, 1, relabeled.output); assert.match(relabeled.output, /未登记/);
});


test('formal inventory requires exact archived legal paths and rejects temporary carrier relabels', t => {
  const manifest = JSON.parse(readFileSync(join(repo, 'src-tauri/core-manifest.json')));
  assert.equal(loadReleaseContract(repo, manifest).sourceCommit, '0cfa24573ff34c26608151068334329268cfdf14');
  const missing = fixture(t, 'windows', [cleaner], root =>
    rmSync(join(root, '_up_/resources/win/core-release/TLS-DEPENDENCY-NOTICES.txt')));
  assert.equal(missing.code, 1, missing.output);
  assert.match(missing.output, /release-member:core-release\/TLS-DEPENDENCY-NOTICES\.txt.*实为 0 份/);
  for (const member of ['TLS-DEPENDENCY-NOTICES.txt', 'candidate-carrier.json']) {
    const extra = fixture(t, 'windows', [cleaner, `_up_/resources/win/${member}`]);
    assert.equal(extra.code, 1, extra.output);
    assert.match(extra.output, /未登记/);
    assert.ok(extra.output.includes(`_up_/resources/win/${member}`), extra.output);
  }
});

// Exercise the actual registry and binding branch without acquiring native payloads.
// Other conf checks require downloaded resources; these tests isolate only CI routing.
function platformBindingFixture(mutate = () => {}) {
  const source = readFileSync(verifier, 'utf8');
  const registry = source.slice(source.indexOf('const NON_CORE_CONFS ='), source.indexOf('\nconst errors ='));
  const specs = Function(`${registry}; return NON_CORE_CONFS;`)();
  const start = source.indexOf('    if (workflows !== null)', source.indexOf('// ④e'));
  const end = source.indexOf('\n  }\n\n  if (errors.length === errorsBefore)', start);
  const check = Function('workflows', 'name', 'spec', 'readFileSync', 'join', 'ROOT', 'fail', source.slice(start, end));
  const workflows = new Map(readdirSync(join(repo, '.github/workflows')).map(name =>
    [name, readFileSync(join(repo, '.github/workflows', name), 'utf8')]));
  const inputs = new Map(specs['tauri.ios.conf.json'].ciLeg.chain.map(([path]) =>
    [join(repo, path), readFileSync(join(repo, path), 'utf8')]));
  mutate({ workflows, inputs, specs });
  const failures = [];
  for (const [name, spec] of Object.entries(specs)) check(workflows, name, spec,
    path => { if (!inputs.has(path)) throw new Error('missing fixture source'); return inputs.get(path); },
    join, repo, message => failures.push(message));
  return failures;
}

test('platform registry recognizes the real indirect Apple configuration consumer and direct Android calls', () => {
  assert.deepEqual(platformBindingFixture(), []);
});
for (const part of ['entry', 'carrier', 'app-builder', 'config-read', 'config-env']) {
  test(`platform registry rejects a broken Apple configuration link: ${part}`, () => {
    const failures = platformBindingFixture(({ workflows, inputs, specs }) => {
      const leg = specs['tauri.ios.conf.json'].ciLeg;
      if (part === 'entry') workflows.set(leg.workflow, workflows.get(leg.workflow).replaceAll(leg.entry, 'missing-carrier'));
      else {
        const index = ['carrier', 'app-builder', 'config-read', 'config-env'].indexOf(part);
        const [path, binding] = leg.chain[index];
        inputs.set(join(repo, path), inputs.get(join(repo, path)).replace(binding, 'missing-binding'));
      }
    });
    assert.ok(failures.length > 0, part);
  });
}
test('platform registry rejects an unregistered workflow using the indirect Apple entry', () => {
  assert.ok(platformBindingFixture(({ workflows, specs }) =>
    workflows.set('extra.yml', specs['tauri.ios.conf.json'].ciLeg.entry)).length > 0);
});
test('platform registry fails closed if an indirect consumer source is missing', () => {
  assert.ok(platformBindingFixture(({ inputs }) => inputs.clear()).length > 0);
});
