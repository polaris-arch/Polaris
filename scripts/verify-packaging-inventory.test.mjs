import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, copyFileSync, cpSync, readdirSync, statSync, symlinkSync, renameSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, after } from 'node:test';
import { copyPackagingSource, writeFormalAssets } from './lib/formal-package-fixtures.test.mjs';
import { cronetFixture, fixtureManifest } from './lib/cronet-payload.test.mjs';
import { assetNames } from './release-assets.mjs';
import { selectedCoreManifest } from './lib/core-identity.mjs';
import { createHash } from 'node:crypto';
import { hasDesktopRelease, loadReleaseContract, releasePayloadMembers } from './desktop-core/release-consumer.mjs';
import { hasTemporaryDesktop, loadTemporaryDesktopSelection, loadCandidate } from './desktop-core/native-dns-candidate.mjs';

const repo = fileURLToPath(new URL('../', import.meta.url));
const cleanRepo=mkdtempSync(join(tmpdir(),'polaris-formal-packaging-source-'));copyPackagingSource(repo,cleanRepo);after(()=>rmSync(cleanRepo,{recursive:true,force:true}));
const verifier = join(cleanRepo, 'scripts/verify-packaging.mjs');
const count = Number(readFileSync(join(repo, 'src-tauri/build.rs'), 'utf8')
  .match(/^const EXPECTED_SRS_COUNT: usize = (\d+);$/m)?.[1]);
assert.ok(count > 0, 'fixture count must come from the actual build source');

// Inert staging members exercise the actual inventory CLI, not production packages.
function fixture(t, label, extra = [], mutate = () => {}, selectionRepo = repo, isStatic = false) {
  if (selectionRepo === repo) selectionRepo=cleanRepo;
  const root = mkdtempSync(join(tmpdir(), 'polaris-cleaner-inventory-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  if (isStatic) {
    const original = selectionRepo;
    selectionRepo = join(root, 'selection');
    cpSync(join(original, 'scripts'), join(selectionRepo, 'scripts'), { recursive: true, filter: p => !p.includes('node_modules') });
    for (const name of ['core-manifest.json', 'build.rs', 'tauri.conf.json',
      'tauri.linux.conf.json', 'tauri.windows.conf.json', 'tauri.macos-x64.conf.json', 'tauri.macos-arm64.conf.json']) {
      mkdirSync(join(selectionRepo, 'src-tauri'), { recursive: true });
      copyFileSync(join(original, 'src-tauri', name), join(selectionRepo, 'src-tauri', name));
    }
  }
  const core = { linux: 'linux', windows: 'win', 'macos-x64': 'mac-x64', 'macos-arm64': 'mac-arm64' }[label];
  const suffix = label === 'windows' ? '.exe' : '';
  const manifest = JSON.parse(readFileSync(join(selectionRepo, 'src-tauri/core-manifest.json')));
  const temporary = hasTemporaryDesktop(manifest);
  const release = hasDesktopRelease(manifest) ? loadReleaseContract(selectionRepo, manifest) : null;
  if (temporary) loadTemporaryDesktopSelection(selectionRepo, manifest);
  const row = temporary ? loadCandidate(selectionRepo).platforms[core] : null;
  const legalMembers = row ? Object.keys(row.archive.members).map((name) => name.slice(row.archive.root.length + 1))
    .filter((name) => name !== row.core.member && name !== row.sidecar?.member)
    : release ? releasePayloadMembers(release, core) : [];
  const members = [
    'core-manifest.json', '_up_/LICENSE', '_up_/NOTICE', '_up_/THIRD-PARTY-LICENSES.md',
    ...Array.from({ length: count }, (_, i) => `_up_/resources/data/fixture-${i}.srs`),
    '_up_/resources/dashboard/index.html', '_up_/resources/dashboard/assets/fixture.js',
    '_up_/resources/dashboard/licenses/fixture.txt', '_up_/resources/dashboard/favicon.ico',
    '_up_/resources/dashboard/favicon.svg', '_up_/resources/dashboard/apple-touch-icon-180x180.png',
    `_up_/resources/${core}/core-manifest.json`,
    ...(temporary ? [`_up_/resources/${core}/candidate-carrier.json`] : []),
    `_up_/resources/${core}/sing-box${suffix}`, `_up_/resources/${core}/polaris-helper${suffix}`,
    ...(['linux', 'windows'].includes(label) ? [`_up_/resources/${core}/libcronet.${label === 'windows' ? 'dll' : 'so'}`] : []),
    ...(release ? [`_up_/resources/core-recovery/${core}/inventory.json`,
      `_up_/resources/core-recovery/${core}/${release.platforms[core].archive.name}`] : []),
    ...legalMembers.map((member) => `_up_/resources/${core}/${member}`), ...extra,
  ];
  const scopes = isStatic ? [selectionRepo] : label === 'windows' ? [root]
    : label.startsWith('macos-') ? [join(root, 'macos/Polaris.app/Contents/Resources')]
      : [join(root, 'deb/usr/lib/Polaris'), join(root, 'appimage/Polaris.AppDir/usr/lib/Polaris')];
  for (const scope of scopes) for (const member of members) {
    if (isStatic && member === 'core-manifest.json') continue; // Keep the real selection JSON.
    const path = join(scope, isStatic ? member.replace(/^_up_\//, '') : member);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, 'inert inventory fixture');
  }
  if (!isStatic) {
    const main=join(root,label==='linux'?'deb/usr/bin/polaris':label==='windows'?'polaris.exe':'macos/Polaris.app/Contents/MacOS/Polaris');
    mkdirSync(dirname(main),{recursive:true});writeFileSync(main,'ordinary App outside resources');
  }
  mutate(isStatic ? selectionRepo : root);
  const result = spawnSync(process.execPath, [join(selectionRepo, 'scripts/verify-packaging.mjs'), 'inventory', '--label', label,
    ...(isStatic ? ['--static'] : ['--root', root])], { encoding: 'utf8' });
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
    ...['source-graph', 'bundle', 'release-consumer', 'recovery', 'native-dns-candidate', 'upstream-release', 'validation-origin', 'native-dns-gates', 'local-development']
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

for (const fault of ['source replacement', 'symlink', 'symlink ancestor', 'missing pin', 'wrong architecture', 'unknown dependency', 'missing exported function']) {
  test(`actual Windows payload CLI rejects ${fault}`, (t) => {
    const options = fault === 'wrong architecture' ? { machine: 0xaa64 } : fault === 'unknown dependency' ? { dependency: 'injected.dll' } : fault === 'missing exported function' ? { missingExport: 'Cronet_Engine_Create' } : {};
    const f = windowsPayloadFixture(t, options);
    if (fault === 'source replacement') { const changed = Buffer.from(f.bytes); changed[0x1200] ^= 1; writeFileSync(f.source, changed); writeFileSync(f.payload, changed); }
    if (fault === 'symlink') { rmSync(f.payload); symlinkSync(f.source, f.payload); }
    if (fault === 'symlink ancestor') {
      const directory = dirname(f.payload), original = `${directory}-original`;
      renameSync(directory, original);
      symlinkSync(original, directory, process.platform === 'win32' ? 'junction' : 'dir');
    }
    if (fault === 'missing pin') writeFileSync(join(f.root, 'src-tauri/core-manifest.json'), JSON.stringify({ coreArchiveSha256: { win: 'fixture' } }));
    const result = f.run(); assert.equal(result.code, 1, result.output); assert.match(result.output, /Cronet|cronetLibrarySha256/);
  });
}

// Pure identity regression, not a fake filesystem or a substitute for the real
// CLI cycle/alias fixtures below. Adjacent NTFS IDs can collide as Numbers.
test('inventory directory identity retains adjacent unsafe-integer IDs exactly', () => {
  const source = readFileSync(verifier, 'utf8');
  const start = source.indexOf('function inventoryDirectoryKey(');
  const end = source.indexOf('\nfunction walk2(', start);
  assert.ok(start >= 0 && end > start, 'use the actual inventory identity function');
  const identity = Function('InputError', `${source.slice(start, end)}; return inventoryDirectoryKey;`)(Error);
  const first = { dev: 7n, ino: 9007199254740992n };
  const next = { dev: 7n, ino: first.ino + 1n };
  assert.equal(Number(first.ino), Number(next.ino), 'the old Number identity collides');
  assert.notEqual(identity(first), identity(next));
  assert.equal(identity(first), identity({ ...first }));
  assert.notEqual(identity(first), identity({ ...first, dev: 8n }));
  for (const lossy of [{ ...first, dev: 7 }, { ...first, ino: Number(first.ino) }]) {
    assert.throws(() => identity(lossy), /exact bigint directory identity required/);
  }
});

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
test('symlink ancestor cycle hard-fails without hiding the loop', t => {
  const r = fixture(t, 'windows', [cleaner], root =>
    symlinkSync(root, join(root, 'loop'), process.platform === 'win32' ? 'junction' : 'dir'));
  assert.equal(r.code, 1, r.output);
  assert.match(r.output, /symlink cycle/);
});
test('separate directory aliases are enumerated rather than globally deduplicated', t => {
  let aliasedMembers;
  const r = fixture(t, 'windows', [cleaner], root => {
    const original = join(root, '_up_/resources/win');
    aliasedMembers = readdirSync(original, { recursive: true, withFileTypes: true }).filter(entry => entry.isFile()).length;
    symlinkSync(original, join(root, '_up_/alias'), process.platform === 'win32' ? 'junction' : 'dir');
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
    if (extraDepth > 24) writeFileSync(target,'depth-only inert asset; walk must reject before object inspection');
    else writeFormalAssets(dirname(target), [name]);
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
test('Linux assets gate accepts realistic deep deb and AppDir staging', { skip: process.platform !== 'linux' }, t => {
  const result = linuxAssetFixture(t);
  assert.equal(result.code, 0, result.output);
  assert.match(result.output, /verify-packaging assets/);
});
test('Linux assets gate visits its explicit depth 24 boundary', { skip: process.platform !== 'linux' }, t => {
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
    const result = member === 'candidate-carrier.JSON'
      ? temporaryFixture(t, 'windows', [cleaner], root => {
        const directory = join(root, '_up_/resources/win');
        const canonical = join(directory, 'candidate-carrier.json');
        const moving = join(directory, 'owned-case-change.tmp');
        // Two distinct renames construct an uppercase-only entry on both case
        // sensitive and insensitive volumes. An extra write can merely alias
        // the already-created canonical file and never exercise the negative.
        renameSync(canonical, moving);
        renameSync(moving, join(directory, member));
        const names = readdirSync(directory);
        assert(names.includes(member), 'actual uppercase directory entry required');
        assert(!names.includes('candidate-carrier.json'), 'canonical entry must be absent');
        assert(!names.includes('owned-case-change.tmp'));
      })
      : temporaryFixture(t, 'windows', [cleaner, `_up_/resources/win/${member}`]);
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
  assert.equal(loadReleaseContract(repo, manifest).sourceCommit, '3155c9151f553967aabcee93f09e51084fc89899');
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
// Exercise the real confs callee with real config JSON; other conf checks need
// downloaded resources and CI context, while this branch is entirely data-only.
function recoveryConfFailures(p, mutate = () => {}) {
  const source = readFileSync(verifier, 'utf8');
  const start = source.indexOf('function coreDirOf('), end = source.indexOf('function stripYamlComment(', start);
  assert.ok(start >= 0 && end > start, 'actual conf registration callee boundaries');
  const helpers = source.slice(start, end);
  const failures = [];
  const check = Function('fail', `${helpers}; return checkPlatformResourceEntries;`)(message => failures.push(message));
  const names = { linux: 'linux', win: 'windows', 'mac-x64': 'macos-x64', 'mac-arm64': 'macos-arm64' };
  const confName = `tauri.${names[p]}.conf.json`;
  const res = JSON.parse(readFileSync(join(repo, 'src-tauri', confName))).bundle.resources;
  const base = JSON.parse(readFileSync(join(repo, 'src-tauri/tauri.conf.json'))).bundle.resources;
  mutate(res, base);
  check(confName, res, base, p, Object.keys(names), true);
  return failures;
}
for (const [label, p] of [['linux', 'linux'], ['windows', 'win'], ['macos-x64', 'mac-x64'], ['macos-arm64', 'mac-arm64']]) {
  test(`real ${p} conf registration admits only one exact recovery subtree`, () => {
    assert.deepEqual(recoveryConfFailures(p), []);
    for (const change of ['missing', 'duplicate', 'wrong-platform', 'parent', 'child', 'common-bypass']) {
      const failures = recoveryConfFailures(p, (res, base) => {
        const entry = `../resources/core-recovery/${p}/`, index = res.indexOf(entry);
        if (change === 'missing') res.splice(index, 1);
        else if (change === 'duplicate') res.push(entry);
        else if (change === 'common-bypass') base.push('../resources/core-recovery/');
        else res[index] = change === 'parent' ? '../resources/core-recovery/'
          : change === 'child' ? entry + 'inventory.json'
            : `../resources/core-recovery/${p === 'win' ? 'linux' : 'win'}/`;
      });
      assert.ok(failures.length > 0, change);
    }
  });
  for (const isStatic of [false, true]) test(`actual ${p} ${isStatic ? 'static' : 'artifact'} inventory requires the two exact recovery data members`, t => {
    const extras = label === 'windows' ? [cleaner] : [];
    const good = fixture(t, label, extras, () => {}, repo, isStatic);
    assert.equal(good.code, 0, good.output);
    const manifest = JSON.parse(readFileSync(join(repo, 'src-tauri/core-manifest.json')));
    const name = loadReleaseContract(repo, manifest).platforms[p].archive.name;
    const relative = `resources/core-recovery/${p}/`;
    const scopes = root => isStatic ? [root] : label === 'windows' ? [root]
      : label.startsWith('macos-') ? [join(root, 'macos/Polaris.app/Contents/Resources')]
        : [join(root, 'deb/usr/lib/Polaris'), join(root, 'appimage/Polaris.AppDir/usr/lib/Polaris')];
    for (const missing of [name, 'inventory.json']) {
      const result = fixture(t, label, extras, root => {
        for (const scope of scopes(root)) rmSync(join(scope, isStatic ? relative : '_up_/' + relative, missing));
      }, repo, isStatic);
      assert.equal(result.code, 1, result.output);
      assert.match(result.output, /core-recovery-(archive|inventory).*实为 0 份/);
    }
    const other = p === 'win' ? 'linux' : 'win', crossPlatform = `resources/core-recovery/${other}/${name}`;
    const confName = `tauri.${label === 'windows' ? 'windows' : label}.conf.json`;
    for (const extra of [relative + name + '.backup', relative + 'extra.json', crossPlatform]) {
      const result = fixture(t, label, [...extras, '_up_/' + extra], root => {
        if (isStatic && extra === crossPlatform) {
          // A real unwanted candidate payload: the extra subtree must actually
          // be mapped before the static inventory can see and reject it.
          const file = join(root, 'src-tauri', confName), conf = JSON.parse(readFileSync(file));
          conf.bundle.resources.push(`../resources/core-recovery/${other}/`);
          writeFileSync(file, JSON.stringify(conf));
        }
      }, repo, isStatic);
      assert.equal(result.code, 1, result.output); assert.match(result.output, /未登记/);
    }
    if (isStatic) {
      const unmapped = fixture(t, label, [...extras, '_up_/' + crossPlatform], () => {}, repo, true);
      assert.equal(unmapped.code, 0, unmapped.output); // Unmapped source files never enter this package.
      const duplicate = fixture(t, label, extras, root => {
        const file = join(root, 'src-tauri', confName);
        const conf = JSON.parse(readFileSync(file)); conf.bundle.resources.push(`../resources/core-recovery/${p}/`);
        writeFileSync(file, JSON.stringify(conf));
      }, repo, true);
      assert.equal(duplicate.code, 1, duplicate.output);
      assert.match(duplicate.output, /core-recovery-(archive|inventory).*实为 2 份/);
    }
  });
}

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

for (const label of ['linux','windows','macos-x64','macos-arm64']) test(`actual ${label} inventory rejects a Local App outside otherwise complete Formal resources`, t => {
  const r=fixture(t,label,label==='windows'?[cleaner]:[],root=>{
    const main=join(root,label==='linux'?'deb/usr/bin/polaris':label==='windows'?'polaris.exe':'macos/Polaris.app/Contents/MacOS/Polaris');
    mkdirSync(dirname(main),{recursive:true});writeFileSync(main,Buffer.concat([Buffer.alloc(65530),Buffer.from('POLARIS_SELECTION_LOCAL_DEVELOPMENT_V1')]));
  });
  assert.equal(r.code,1,r.output);assert.match(r.output,/LocalDevelopmentSelection executable/);
});
test('actual Linux assets entry rejects compressed Local App despite clean ROOT and recomputed whole-file SHA',{ skip: process.platform !== 'linux' },t=>{
  const root=mkdtempSync(join(tmpdir(),'local-object-assets-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
  const version=JSON.parse(readFileSync(join(cleanRepo,'src-tauri/tauri.conf.json'))).version;
  const names=assetNames(version,'linux');writeFormalAssets(root,names);
  // Replace the actual App in a real deb, preserving complete inert Formal Helper resources.
  const mutate=spawnSync('python3',['-B','-c',`import subprocess,tempfile
from pathlib import Path
import sys
p=Path(sys.argv[1])
with tempfile.TemporaryDirectory() as t:
 subprocess.run(['dpkg-deb','--raw-extract',str(p),t],check=True)
 (Path(t)/'usr/bin/polaris').write_bytes(b'POLARIS_SELECTION_LOCAL_DEVELOPMENT_V1')
 subprocess.run(['dpkg-deb','--build','--root-owner-group',t,str(p)],check=True)`,join(root,names.find(n=>n.endsWith('.deb')))],{encoding:'utf8'});
  assert.equal(mutate.status,0,mutate.stderr);
  writeFileSync(join(root,'SHA256SUMS'),names.map(n=>createHash('sha256').update(readFileSync(join(root,n))).digest('hex')+'  '+n).join('\n')+'\n');
  const r=spawnSync(process.execPath,[verifier,'assets','--label','linux','--dir',root],{encoding:'utf8'});
  assert.equal(r.status,1,r.stdout+r.stderr);assert.match(r.stdout+r.stderr,/LocalDevelopmentSelection executable/);
});
