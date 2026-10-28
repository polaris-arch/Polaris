import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, copyFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { coreIdentityId, canonicalCoreIdentity, selectedCoreManifest, writeSelectedCoreManifest, verifySelectedCoreManifest } from './lib/core-identity.mjs';

const buildSource = readFileSync(new URL('../src-tauri/build.rs', import.meta.url), 'utf8');
function assertBuildDigestSource(source) {
  const helper = source.match(/^fn sha256_hex\(bytes: &\[u8\]\) -> String \{\n[\s\S]*?^\}/m)?.[0];
  assert.ok(helper, 'build digest helper must hash the original bytes');
  assert.match(helper, /for byte in sha2::Sha256::digest\(bytes\)\s*\{/);
  assert.match(helper, /write!\(&mut hex, "\{byte:02x\}"\)/,
    'sha2 digest arrays require per-byte lowercase, zero-padded hex');
  assert.match(source, /InstalledCoreManifest::parse\(&raw, sha256_hex\)/,
    'manifest identity and candidate fingerprint must share the encoder');
  assert.match(source, /if !temporary_default && std::env::var_os\("POLARIS_NATIVE_DNS_CANDIDATE"\)\.is_none\(\) \{\s*let candidate_fingerprint = sha256_hex\(include_bytes!\(\s*"\.\.\/scripts\/desktop-core\/native-dns-candidate\.json"\s*\)\);\s*assert_ne!\(\s*manifest\.core\.provider_fingerprint, candidate_fingerprint,/,
    'formal builds must reject the fingerprint of the exact candidate contract bytes');
}
test('build identity shares byte-wise SHA-256 encoding and keeps the candidate exclusion', () => {
  assertBuildDigestSource(buildSource);
});
for (const [name, before, after] of [
  ['digest-array LowerHex', /for byte in sha2::Sha256::digest\(bytes\) \{[\s\S]*?\n    \}\n    hex/,
    'format!("{:x}", sha2::Sha256::digest(bytes))'],
  ['unpadded hex', '"{byte:02x}"', '"{byte:x}"'],
  ['uppercase hex', '"{byte:02x}"', '"{byte:02X}"'],
  ['different manifest encoder', 'InstalledCoreManifest::parse(&raw, sha256_hex)', 'InstalledCoreManifest::parse(&raw, other_hash)'],
  ['different source bytes', '../scripts/desktop-core/native-dns-candidate.json', '../scripts/desktop-core/release-contract.json'],
  ['constant fingerprint', /sha256_hex\(include_bytes!\([\s\S]*?\)\)/, 'sha256_hex(b"")'],
  ['reversed fingerprint comparison', /assert_ne!\(\s*manifest\.core\.provider_fingerprint/, 'assert_eq!(manifest.core.provider_fingerprint'],
  ['inverted formal-mode guard', /if !temporary_default && std::env::var_os\("POLARIS_NATIVE_DNS_CANDIDATE"\)\.is_none\(\)/,
    'if !temporary_default && std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_some()'],
]) test(`build digest source control rejects ${name}`, () => {
  const changed = buildSource.replace(before, after);
  assert.notEqual(changed, buildSource, 'negative control must change the build source');
  assert.throws(() => assertBuildDigestSource(changed), assert.AssertionError);
});

const core = { platform: 'linux', version: '1.15.0-alpha.12.polaris.1', providerFingerprint: 'a'.repeat(64),
  files: [{ name: 'sing-box', bytes: 10, sha256: 'b'.repeat(64) }, { name: 'libcronet.so', bytes: 20, sha256: 'c'.repeat(64) }], helperSha256: null };
test('framed identity is stable across file order and helper installation metadata', () => {
  assert.equal(coreIdentityId(core), coreIdentityId({ ...core, files: [...core.files].reverse(), helperSha256: 'd'.repeat(64) }));
  const changed = structuredClone(core); changed.files[0].bytes++;
  assert.notEqual(coreIdentityId(core), coreIdentityId(changed));
});
test('unknown payload, absent sidecar and invalid id inputs are refused', () => {
  for (const name of ['../sing-box', 'polaris-helper', 'libcronet.dll.']) {
    const changed = structuredClone(core); changed.files[0].name = name;
    assert.throws(() => canonicalCoreIdentity(changed));
  }
  assert.throws(() => canonicalCoreIdentity({ ...core, files: core.files.slice(0, 1) }));
});
test('selected input preserves actual producer hashes without synthesizing helper bytes', () => {
  const row = { providerFingerprint: core.providerFingerprint, core: { member: 'sing-box', ...core.files[0] },
    sidecar: { member: 'libcronet.so', ...core.files[1] } };
  const value = selectedCoreManifest({ version: core.version, platforms: { linux: row } }, 'linux', '1.0.0');
  assert.equal(value.core.helperSha256, null);
  assert.equal(value.id, coreIdentityId(core));
});

test('final installer hashes reject changed helper, payload and self-consistent replacement manifests', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'polaris-installed-core-identity-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const source = join(root, 'source'), final = join(root, 'final'); mkdirSync(source); mkdirSync(final);
  const hash = (value) => createHash('sha256').update(value).digest('hex');
  const data = { 'sing-box': Buffer.from('core bytes'), 'libcronet.so': Buffer.from('library bytes'), 'polaris-helper': Buffer.from('helper bytes') };
  for (const [name, raw] of Object.entries(data)) { writeFileSync(join(source, name), raw); writeFileSync(join(final, name), raw); }
  const file = (member) => ({ member, bytes: data[member].length, sha256: hash(data[member]) });
  const contract = { version: '1.15.0-alpha.12', platforms: { linux: { providerFingerprint: 'a'.repeat(64), core: file('sing-box'), sidecar: file('libcronet.so') } } };
  writeSelectedCoreManifest(source, contract, 'linux', '1.0.0');
  copyFileSync(join(source, 'core-manifest.json'), join(final, 'core-manifest.json'));
  assert.equal(verifySelectedCoreManifest(final, source, 'linux').core.helperSha256, hash(data['polaris-helper']));
  for (const name of ['polaris-helper', 'sing-box', 'libcronet.so']) {
    writeFileSync(join(final, name), 'altered bytes');
    assert.throws(() => verifySelectedCoreManifest(final, source, 'linux'));
    writeFileSync(join(final, name), data[name]);
  }
  const replacement = JSON.parse(readFileSync(join(final, 'core-manifest.json')));
  replacement.core.helperSha256 = hash('altered bytes');
  writeFileSync(join(final, 'polaris-helper'), 'altered bytes');
  writeFileSync(join(final, 'core-manifest.json'), JSON.stringify(replacement));
  assert.throws(() => verifySelectedCoreManifest(final, source, 'linux'), /compiled installation input/);
});


test('candidate carrier cannot synthesize missing helper or Windows cleaner hashes', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'polaris-candidate-carrier-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const hash = (value) => createHash('sha256').update(value).digest('hex');
  const files = { 'sing-box.exe': 'inert core', 'libcronet.dll': 'inert companion',
    'polaris-helper.exe': 'same build helper', 'polaris-cleaner.exe': 'same build cleaner' };
  const pin = (member) => ({ member, bytes: Buffer.byteLength(files[member]), sha256: hash(files[member]) });
  const contract = { version: '1.15.0-polaris-alpha.12', platforms: { win: {
    providerFingerprint: hash('distinct temporary candidate contract'), core: pin('sing-box.exe'), sidecar: pin('libcronet.dll') } } };
  for (const name of ['sing-box.exe', 'libcronet.dll']) writeFileSync(join(root, name), files[name]);
  assert.throws(() => writeSelectedCoreManifest(root, contract, 'win', '1.0.0'), /ENOENT/);
  writeFileSync(join(root, 'polaris-helper.exe'), files['polaris-helper.exe']);
  assert.throws(() => writeSelectedCoreManifest(root, contract, 'win', '1.0.0'), /ENOENT/);
  writeFileSync(join(root, 'polaris-cleaner.exe'), files['polaris-cleaner.exe']);
  const selected = writeSelectedCoreManifest(root, contract, 'win', '1.0.0');
  assert.equal(selected.core.helperSha256, hash(files['polaris-helper.exe']));
  assert.equal(selected.core.cleanerSha256, hash(files['polaris-cleaner.exe']));
  verifySelectedCoreManifest(root, root, 'win');
  for (const name of Object.keys(files)) {
    writeFileSync(join(root, name), 'other source/build');
    assert.throws(() => verifySelectedCoreManifest(root, root, 'win'));
    writeFileSync(join(root, name), files[name]);
  }
  const missing = structuredClone(selected); missing.core.helperSha256 = null;
  writeFileSync(join(root, 'core-manifest.json'), JSON.stringify(missing));
  assert.throws(() => verifySelectedCoreManifest(root, root, 'win'), /actual helper pin/);
  missing.core.helperSha256 = selected.core.helperSha256; missing.core.cleanerSha256 = null;
  writeFileSync(join(root, 'core-manifest.json'), JSON.stringify(missing));
  assert.throws(() => verifySelectedCoreManifest(root, root, 'win'), /actual cleaner pin/);
});

function assertTemporaryBuildSource(source) {
  assert.match(source, /fn desktop_platform\(\) -> Option<&'static str>/);
  assert.match(source.match(/^fn desktop_platform\(\)[\s\S]*?^\}/m)?.[0] ?? '', /std::env::var\("CARGO_CFG_TARGET_ARCH"\)/);
  assert.match(source, /desktop_platform\(\)\.is_some\(\) && manifest\.get\("desktopTemporary"\)\.is_some\(\)/);
  assert.match(source, /\(temporary_default \|\| formal_default\)\.then\(\|\| \{\s*root\.join\("resources"\)\s*\.join\(desktop_platform\(\)\.expect\("desktop target"\)\)/);
  assert.match(source, /assert_eq!\(\s*source\["schema"\], "polaris-native-dns-compiled-source-v2"\)/);
  assert.match(source, /assert_eq!\(source\["validationOnly"\], false\)/);
  assert.match(source, /assert_eq!\(source\["purpose"\], "temporary-default"\)/);
  assert.match(source, /assert_eq!\(source\["context"\]\["mode"\], "temporary-default"\)/);
  assert.match(source, /assert_eq!\(\s*manifest\.core\.provider_fingerprint,\s*sha256_hex\(include_bytes!\(\s*"\.\.\/scripts\/desktop-core\/native-dns-candidate\.json"/);
  const temporaryVerify = source.slice(source.indexOf('if temporary_default || std::env::var_os'), source.indexOf('if formal_default &&'));
  assert.match(temporaryVerify, /assert_eq!\(\s*path,\s*directory\.join\("core-manifest.json"\)/);
  assert.match(temporaryVerify, /\.arg\("--mode=verify"\)/);
  assert.match(source, /assert!\(\s*verified\.status\.success\(\),\s*"candidate App identity verification failed/);
  assert.match(source, /raw,\s*std::fs::read\(&path\)\.expect\("verified identity remains readable"\)/);
}
test('temporary desktop compilation binds target, actual source, exact path and verified carrier', () => {
  assertTemporaryBuildSource(buildSource);
});
for (const [name, before, after] of [
  ['host rather than Cargo target', /std::env::var\("CARGO_CFG_TARGET_ARCH"\)/, 'std::env::var("HOST_ARCH")'],
  ['mobile source embedding', 'desktop_platform().is_some() && manifest', 'true && manifest'],
  ['missing default installation fallback', '(temporary_default || formal_default).then(||', 'false.then(||'],
  ['candidate source substituted by formal', 'polaris-native-dns-compiled-source-v2', 'formal-source'],
  ['mislabelled temporary source', 'source["validationOnly"], false', 'source["validationOnly"], true'],
  ['wrong selection purpose', 'source["purpose"], "temporary-default"', 'source["purpose"], "formal"'],
  ['different provider bytes', /assert_eq!\(\s*manifest\.core\.provider_fingerprint,\s*sha256_hex/, 'assert_eq!(manifest.core.provider_fingerprint, other_hash'],
  ['arbitrary installation path', /assert_eq!\(\s*path,\s*directory/, 'assert_ne!(path, directory'],
  ['skip actual carrier verification', '.arg("--mode=verify")', '.arg("--mode=prepare")'],
  ['ignore actual carrier failure', /verified\.status\.success\(\),\s*"candidate App identity/, 'true, "candidate App identity'],
  ['manifest replacement after verification', 'std::fs::read(&path).expect("verified identity remains readable")', 'raw.clone()'],
]) test(`temporary compilation control rejects ${name}`, () => {
  const changed = buildSource.replace(before, after);
  assert.notEqual(changed, buildSource);
  assert.throws(() => assertTemporaryBuildSource(changed), assert.AssertionError);
});

function assertUnavailableAnalysisSource(source) {
  const pending = source.match(/^fn temporary_installation_pending\([\s\S]*?^\}/m)?.[0] ?? '';
  assert.match(pending, /!temporary_default/);
  assert.match(pending, /var_os\("POLARIS_NATIVE_DNS_CANDIDATE"\)\.is_some\(\)/);
  assert.match(pending, /var_os\("POLARIS_CORE_IDENTITY_FILE"\)\.is_some\(\)/);
  assert.match(pending, /\["core-manifest.json", "candidate-carrier.json"\]/);
  assert.match(pending, /symlink_metadata\(&path\)\.is_ok\(\) \{\s*return false;/);
  assert.match(pending, /try_exists\(\)\s*\.expect\("inspect selected installation path"\)/);
  assert.match(source, /const TEMPORARY_UNAVAILABLE: &\[u8\] =\s*br#"\{"schema":"polaris-temporary-desktop-unavailable-v1"\}"#/);
  assert.equal((source.match(/temporary_installation_pending\(temporary_default\)/g) ?? []).length, 1);
  assert.match(source, /temporary_installation_pending\(temporary_default \|\| formal_default\)/);
}
test('analysis image cannot silently use a missing or partially frozen desktop identity', () => {
  assertUnavailableAnalysisSource(buildSource);
  for (const [before, after] of [
    ['!temporary_default', 'false'],
    ['var_os("POLARIS_CORE_IDENTITY_FILE").is_some()', 'var_os("POLARIS_CORE_IDENTITY_FILE").is_none()'],
    ['["core-manifest.json", "candidate-carrier.json"]', '["core-manifest.json"]'],
    ['symlink_metadata(&path).is_ok()', 'symlink_metadata(&path).is_err()'],
    [/try_exists\(\)\s*\.expect\("inspect selected installation path"\)/, 'try_exists().unwrap_or(false)'],
    ['polaris-temporary-desktop-unavailable-v1', 'polaris-native-dns-compiled-source-v2'],
  ]) {
    const changed = buildSource.replace(before, after);
    assert.notEqual(changed, buildSource);
    assert.throws(() => assertUnavailableAnalysisSource(changed), assert.AssertionError);
  }
});
test('temporary default freezes staged signed helper bytes once, while manual preparation remains isolated', () => {
  const source = readFileSync(new URL('./prepare-core-identity.mjs', import.meta.url), 'utf8');
  const start = source.indexOf("if (mode === 'prepare' && temporaryDefault && !candidateMode) {");
  const end = source.indexOf("} else if (mode === 'prepare') {", start);
  assert.ok(start >= 0 && end > start);
  const staged = source.slice(start, end);
  assert.match(staged, /actualCoreFile\(join\(directory, name\)\)/);
  assert.match(staged, /!existsSync\(join\(directory, 'candidate-carrier.json'\)\)/);
  assert.match(staged, /!existsSync\(join\(directory, 'core-manifest.json'\)\)/);
  assert.doesNotMatch(staged, /freezeCandidateBuildOutput|copyFileSync|target\/debug/);
  assert.match(source.slice(end), /freezeCandidateBuildOutput\(root, directory, name\)/);
});

function assertFormalBuildSource(source) {
  const selection = source.match(/^fn upstream_desktop_default\([\s\S]*?^\}/m)?.[0] ?? '';
  assert.match(selection, /desktop_platform\(\)\.is_none\(\)/);
  assert.match(selection, /manifest\.get\("desktopTemporary"\)\.is_none\(\)/);
  assert.match(selection, /selection\["contractSha256"\]\.as_str\(\),\s*Some\(sha256_hex\(&raw\)\.as_str\(\)\)/);
  assert.match(selection, /contract\["schema"\] != "polaris-upstream-native-release-consumer-v1"/);
  assert.match(selection, /assert_eq!\(\s*path,\s*"scripts\/desktop-core\/release-contract.json"/);
  assert.match(source, /temporary_installation_pending\(temporary_default \|\| formal_default\)/);
  assert.match(source, /\(temporary_default \|\| formal_default\)\.then\(/);
  const verify = source.match(/if formal_default &&[\s\S]*?(?=\n            assert_eq!\(\s*manifest.core.platform)/)?.[0] ?? '';
  assert.match(verify, /directory = root.join\("resources"\).join\(platform\)/);
  assert.match(verify, /assert_eq!\(\s*path,\s*directory.join\("core-manifest.json"\)/);
  assert.match(verify, /\.arg\("--mode=verify"\)/);
  assert.match(verify, /verified.status.success\(\)/);
  assert.match(verify, /raw,\s*std::fs::read\(&path\).expect\("verified formal identity readable"\)/);
}
test('formal default compiles only exact verified installation with real helper and cleaner bytes', () => {
  assertFormalBuildSource(buildSource);
  for (const [before, after] of [
    ['Some(sha256_hex(&raw).as_str())', 'Some("unverified")'],
    ['(temporary_default || formal_default).then(||', 'temporary_default.then(||'],
    ['std::fs::read(&path).expect("verified formal identity readable")', 'raw.clone()'],
  ]) {
    const changed = buildSource.replace(before, after); assert.notEqual(changed, buildSource);
    assert.throws(() => assertFormalBuildSource(changed));
  }
  const formalAt = buildSource.indexOf('if formal_default &&');
  for (const [before, after] of [
    [/assert_eq!\(\s*path,\s*directory/, 'assert_ne!(path, directory'],
    ['.arg("--mode=verify")', '.arg("--mode=prepare")'],
    ['verified.status.success()', 'true'],
  ]) {
    const tail = buildSource.slice(formalAt), changedTail = tail.replace(before, after);
    assert.notEqual(changedTail, tail);
    assert.throws(() => assertFormalBuildSource(buildSource.slice(0, formalAt) + changedTail));
  }
  const prepare = readFileSync(new URL('./prepare-core-identity.mjs', import.meta.url), 'utf8');
  assert.match(prepare, /contract = releaseInstallationContract\(root, input\);[\s\S]*?verifyPackagedRelease\(root, input, platform, join\(directory, contract.platforms\[platform\].core.member\)\)/);
  assert.match(prepare, /actualCoreFile\(join\(directory, helper\)\).sha256 !== old.core.helperSha256/);
  assert.match(prepare, /actualCoreFile\(join\(directory, 'polaris-cleaner.exe'\)\).sha256 !== old.core.cleanerSha256/);
});


test('private check identity is compiled only for its Linux x86_64 test caller', () => {
  const runtime = readFileSync(new URL('../src-tauri/src/runtime/core_identity.rs', import.meta.url), 'utf8');
  const caller = readFileSync(new URL('../src-tauri/src/runtime/proxy/mesh_apply/candidate/check.rs', import.meta.url), 'utf8');
  const cfg = '#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]';
  const declaration = `${cfg}\npub(crate) fn selected_check_source(`;
  const check = (source) => {
    assert.ok(source.includes(declaration), 'check-only declaration must match its sole caller platform');
    const functionAt = source.indexOf('pub(crate) fn selected_check_source(');
    const attributes = source.slice(source.lastIndexOf('\n\n', functionAt) + 2, functionAt);
    assert.doesNotMatch(attributes, /#\[(?:allow|expect)\(dead_code\)\]/);
    // Adjacent helpers also serve Windows production and portable unit tests.
    for (const signature of ['fn selected_source_from(', 'pub(crate) fn selected_source_commit('])
      assert.ok(source.includes(`#[cfg(any(windows, test))]\n${signature}`));
  };
  check(runtime);
  assert.ok(caller.includes(`${cfg}\npub(super) fn selected_core_for_test()`));
  assert.equal((caller.match(/core_identity::selected_check_source\("linux"\)/g) ?? []).length, 1);
  for (const replacement of ['#[cfg(test)]', '#[cfg(target_os = "linux")]',
    '#[cfg(all(test, target_os = "linux", target_arch = "aarch64"))]',
    '#[allow(dead_code)]\n' + cfg]) {
    const changed = runtime.replace(declaration, `${replacement}\npub(crate) fn selected_check_source(`);
    assert.notEqual(changed, runtime);
    assert.throws(() => check(changed));
  }
  const narrowed = runtime.replace('#[cfg(any(windows, test))]\nfn selected_source_from(', `${cfg}\nfn selected_source_from(`);
  assert.throws(() => check(narrowed), 'portable/Windows helper coverage must not be narrowed with the Linux-only helper');
});
