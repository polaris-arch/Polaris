import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, copyFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { coreIdentityId, canonicalCoreIdentity, selectedCoreManifest, writeSelectedCoreManifest, verifySelectedCoreManifest } from './lib/core-identity.mjs';

const sourceText = path => readFileSync(new URL(path, import.meta.url), 'utf8').replaceAll('\r\n', '\n');
const buildEntry = sourceText('../src-tauri/build.rs');
const buildSource = sourceText('../crates/core-supervisor/build/selected_core.rs');
// Keep offsets/newlines while excluding Rust comments and literal contents from
// declaration/call evidence. This is lexical masking, not cfg or flow analysis.
function rustMask(source, literals = true) {
  const out = source.split('');
  const erase = (from, to) => {
    for (let at = from; at < to; at++) if (!/[\r\n]/.test(out[at])) out[at] = ' ';
  };
  for (let at = 0; at < source.length;) {
    const start = at;
    if (source.startsWith('//', at)) {
      at = source.indexOf('\n', at);
      if (at < 0) at = source.length;
      erase(start, at);
    } else if (source.startsWith('/*', at)) {
      let depth = 1; at += 2;
      while (at < source.length && depth) {
        if (source.startsWith('/*', at)) { depth++; at += 2; }
        else if (source.startsWith('*/', at)) { depth--; at += 2; }
        else at++;
      }
      assert.equal(depth, 0, 'unclosed Rust comment');
      erase(start, at);
    } else {
      const tail = source.slice(at);
      const raw = /^(?:br|cr|r)(#*)"/.exec(tail);
      const quoted = /^(?:b|c)?"/.exec(tail);
      const character = /^(?:b)?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|[^\r\n])|[^'\\\r\n])'/u.exec(tail);
      if (raw) {
        const end = source.indexOf('"' + raw[1], at + raw[0].length);
        assert.notEqual(end, -1, 'unclosed Rust raw literal');
        at = end + raw[1].length + 1;
      } else if (quoted) {
        at += quoted[0].length;
        let closed = false;
        while (at < source.length) {
          if (source[at] === '\\') at += 2;
          else if (source[at++] === '"') { closed = true; break; }
        }
        assert.ok(closed, 'unclosed Rust literal');
      } else if (character) at += character[0].length;
      else { at++; continue; } // Lifetimes are not character literals.
      if (literals) erase(start, at);
    }
  }
  return out.join('');
}
function rustFunction(source, name) {
  const code = rustMask(source);
  const declarations = [...code.matchAll(new RegExp(`\\bfn\\s+${name}\\s*\\(`, 'g'))]
    .filter(match => {
      let depth = 0;
      for (const ch of code.slice(0, match.index)) {
        if (ch === '{') depth++;
        else if (ch === '}') depth--;
      }
      return depth === 0;
    });
  assert.equal(declarations.length, 1, `expected one actual top-level Rust function: ${name}`);
  const start = declarations[0].index, open = code.indexOf('{', start);
  assert.notEqual(open, -1, `missing Rust function body: ${name}`);
  let depth = 1, end = open + 1;
  while (end < code.length && depth) {
    if (code[end] === '{') depth++;
    else if (code[end] === '}') depth--;
    end++;
  }
  assert.equal(depth, 0, `unclosed Rust function: ${name}`);
  // Other existing predicates need actual literal bytes. Retain them only after
  // binding this span on the code mask; executable-call predicates mask again.
  return rustMask(source, false).slice(start, end);
}
function assertBuildDelegation(entry, selected) {
  const code = rustMask(entry), comments = rustMask(entry, false);
  const declarations = [...code.matchAll(/#\[path\s*=\s*[ \t]+\]\s*mod selected_core\s*;/g)];
  assert.equal(declarations.length, 1, 'one actual selected_core path/module declaration');
  assert.equal((code.match(/\bmod\s+selected_core\s*;/g) ?? []).length, 1);
  const declaration = declarations[0];
  const path = comments.slice(declaration.index, declaration.index + declaration[0].indexOf(']') + 1);
  assert.match(path, /^#\[path\s*=\s*"\.\.\/crates\/core-supervisor\/build\/selected_core\.rs"\s*\]$/);
  const main = rustMask(rustFunction(entry, 'main')).replace(/\s+/g, '');
  assert.equal((main.match(/(?<![\w:])selected_core::emit\(\);/g) ?? []).length, 1);
  const emit = rustMask(rustFunction(selected, 'emit')).replace(/\s+/g, '');
  for (const name of ['embed_validation_core_source', 'embed_selected_core_identity'])
    assert.equal((emit.match(new RegExp(`(?<![\\w:])${name}\\(\\);`, 'g')) ?? []).length, 1);
}
test('App build delegates selection to the exact shared module and emit entry', () => {
  assertBuildDelegation(buildEntry, buildSource);
  for (const [before, after] of [
    ['../crates/core-supervisor/build/selected_core.rs', '../other.rs'],
    ['mod selected_core;', 'mod another_core;'],
    ['selected_core::emit();', 'other_core::emit();'],
  ]) {
    const changed = buildEntry.replace(before, after);
    assert.notEqual(changed, buildEntry);
    assert.throws(() => assertBuildDelegation(changed, buildSource), assert.AssertionError);
  }
  for (const name of ['embed_validation_core_source', 'embed_selected_core_identity']) {
    const changed = buildSource.replace(`${name}();`, 'unreviewed_selection();');
    assert.notEqual(changed, buildSource);
    assert.throws(() => assertBuildDelegation(buildEntry, changed), assert.AssertionError);
  }
});
test('shared build delegation rejects commented, literal and duplicate Rust evidence', () => {
  const main = rustFunction(buildEntry, 'main'), emit = rustFunction(buildSource, 'emit');
  const missingMain = buildEntry.replace('fn main(', 'fn renamed_main(');
  const missingCall = buildEntry.replace('selected_core::emit();', 'other_core::emit();');
  const fakeMain = 'fn main() { selected_core::emit(); }';
  const harmlessDecoys = `/* outer /* nested */ ${fakeMain} */\nconst DECOY: &str = r###"${fakeMain}"###;\n`;
  assertBuildDelegation(harmlessDecoys + buildEntry, buildSource);
  for (const changed of [
    `/*\n${fakeMain}\n*/\n${missingMain}`,
    `const FAKE: &str = r#"${fakeMain}"#;\n${missingMain}`,
    missingCall.replace('other_core::emit();', 'other_core::emit(); /* selected_core::emit(); */'),
    missingCall.replace('other_core::emit();', 'other_core::emit(); let _ = "selected_core::emit();";'),
    `${buildEntry}\n${main}`,
    missingMain + `\nmod unrelated { ${fakeMain} }`,
    buildEntry.replace('#[path = "../crates/core-supervisor/build/selected_core.rs"]\nmod selected_core;',
      '/* #[path = "../crates/core-supervisor/build/selected_core.rs"]\nmod selected_core; */'),
    buildEntry.replace('#[path = "../crates/core-supervisor/build/selected_core.rs"]\nmod selected_core;',
      'const FAKE: &str = r#"#[path = "../crates/core-supervisor/build/selected_core.rs"]\nmod selected_core;"#;'),
  ]) assert.throws(() => assertBuildDelegation(changed, buildSource), assert.AssertionError);
  for (const changed of [
    `/*\n${emit}\n*/\n` + buildSource.replace('fn emit(', 'fn renamed_emit('),
    `const FAKE: &str = r###"${emit}"###;\n` + buildSource.replace('fn emit(', 'fn renamed_emit('),
    `${buildSource}\n${emit}`,
    ...['embed_validation_core_source', 'embed_selected_core_identity'].flatMap(name => [
      buildSource.replace(`${name}();`, `/* ${name}(); */ unreviewed_selection();`),
      buildSource.replace(`${name}();`, `let _ = "${name}();"; unreviewed_selection();`),
    ]),
  ]) assert.throws(() => assertBuildDelegation(buildEntry, changed), assert.AssertionError);
});
function assertBuildDigestSource(source) {
  const helper = rustFunction(source, 'sha256_hex');
  assert.match(helper, /for byte in sha2::Sha256::digest\(bytes\)\s*\{/);
  assert.match(helper, /write!\(&mut hex, "\{byte:02x\}"\)/,
    'sha2 digest arrays require per-byte lowercase, zero-padded hex');
  const identity = rustFunction(source, 'embed_selected_core_identity');
  assert.match(identity, /InstalledCoreManifest::parse\(&raw, sha256_hex\)/,
    'manifest identity and candidate fingerprint must share the encoder');
  assert.match(identity, /if !temporary_default && std::env::var_os\("POLARIS_NATIVE_DNS_CANDIDATE"\)\.is_none\(\) \{\s*let candidate_fingerprint = sha256_hex\(\s*&std::fs::read\(\s*repository_root\(\)\.join\("scripts\/desktop-core\/native-dns-candidate\.json"\),?\s*\)\s*\.expect\("candidate contract"\),?\s*\);\s*assert_ne!\(\s*manifest\.core\.provider_fingerprint, candidate_fingerprint,/,
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
  ['different source bytes', /repository_root\(\)\.join\("scripts\/desktop-core\/native-dns-candidate\.json"\)/,
    'repository_root().join("scripts/desktop-core/release-contract.json")'],
  ['constant fingerprint', /sha256_hex\(\s*&std::fs::read\([\s\S]*?\.expect\("candidate contract"\),?\s*\)/, 'sha256_hex(b"")'],
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
  assert.match(rustFunction(source, 'desktop_platform'), /std::env::var\("CARGO_CFG_TARGET_ARCH"\)/);
  assert.match(rustFunction(source, 'temporary_desktop_default'), /desktop_platform\(\)\.is_some\(\) && manifest\.get\("desktopTemporary"\)\.is_some\(\)/);
  const validation = rustFunction(source, 'embed_validation_core_source');
  assert.match(validation, /assert_eq!\(\s*source\["schema"\], "polaris-native-dns-compiled-source-v2"\)/);
  assert.match(validation, /assert_eq!\(source\["validationOnly"\], false\)/);
  assert.match(validation, /assert_eq!\(source\["purpose"\], "temporary-default"\)/);
  assert.match(validation, /assert_eq!\(source\["context"\]\["mode"\], "temporary-default"\)/);
  source = rustFunction(source, 'embed_selected_core_identity');
  assert.match(source, /\(temporary_default \|\| formal_default\)\.then\(\|\| \{\s*root\.join\("resources"\)\s*\.join\(desktop_platform\(\)\.expect\("desktop target"\)\)/);
  assert.match(source, /assert_eq!\(\s*manifest\.core\.provider_fingerprint,\s*sha256_hex\(\s*&std::fs::read\(\s*repository_root\(\)\s*\.join\("scripts\/desktop-core\/native-dns-candidate\.json"\)/);
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
  const pending = rustFunction(source, 'temporary_installation_pending');
  assert.match(pending, /!temporary_default/);
  assert.match(pending, /var_os\("POLARIS_NATIVE_DNS_CANDIDATE"\)\.is_some\(\)/);
  assert.match(pending, /var_os\("POLARIS_CORE_IDENTITY_FILE"\)\.is_some\(\)/);
  assert.match(pending, /\["core-manifest.json", "candidate-carrier.json"\]/);
  assert.match(pending, /symlink_metadata\(&path\)\.is_ok\(\) \{\s*return false;/);
  assert.match(pending, /try_exists\(\)\s*\.expect\("inspect selected installation path"\)/);
  assert.match(source, /const TEMPORARY_UNAVAILABLE: &\[u8\] =\s*br#"\{"schema":"polaris-temporary-desktop-unavailable-v1"\}"#/);
  assert.equal((rustFunction(source, 'embed_validation_core_source').match(/temporary_installation_pending\(temporary_default\)/g) ?? []).length, 1);
  assert.match(rustFunction(source, 'embed_selected_core_identity'), /temporary_installation_pending\(temporary_default \|\| formal_default\)/);
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
  const selection = rustFunction(source, 'upstream_desktop_default');
  assert.match(selection, /desktop_platform\(\)\.is_none\(\)/);
  assert.match(selection, /manifest\.get\("desktopTemporary"\)\.is_none\(\)/);
  assert.match(selection, /selection\["contractSha256"\]\.as_str\(\),\s*Some\(sha256_hex\(&raw\)\.as_str\(\)\)/);
  assert.match(selection, /contract\["schema"\] != "polaris-upstream-native-release-consumer-v1"/);
  assert.match(selection, /assert_eq!\(\s*path,\s*"scripts\/desktop-core\/release-contract.json"/);
  source = rustFunction(source, 'embed_selected_core_identity');
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


test('App check identity uses shared compiled inputs and retains its Linux x86_64 test-only caller', () => {
  const runtime = sourceText('../src-tauri/src/runtime/core_identity.rs');
  const selected = sourceText('../crates/core-supervisor/src/selected_core.rs');
  const caller = sourceText('../src-tauri/src/runtime/proxy/mesh_apply/candidate/check.rs');
  const cfg = '#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]';
  const declaration = '#[cfg(all(target_os = "linux", target_arch = "x86_64"))]\n#[cfg(test)]\npub(crate) use polaris_core_supervisor::selected_core::selected_check_source;';
  const sourceDeclaration = 'pub(crate) fn selected_source_commit(';
  const itemAttributes = (source, item) => {
    const code = rustMask(source), at = code.indexOf(item);
    assert.ok(at >= 0, `missing actual Rust item: ${item}`);
    assert.equal(code.indexOf(item, at + item.length), -1, `duplicate Rust item: ${item}`);
    return rustMask(source, false).slice(0, at).match(/(?:^|\n)((?:#\[[^\n]*\]\n)*)$/)?.[1] ?? '';
  };
  const check = (source, shared = selected, actualCaller = caller) => {
    assert.ok(source.includes(declaration), 'App check identity must re-export the exact shared provider only in Linux x86_64 tests');
    const functionAt = source.indexOf('pub(crate) use polaris_core_supervisor::selected_core::selected_check_source;');
    const attributes = source.slice(source.lastIndexOf('\n\n', functionAt) + 2, functionAt);
    assert.doesNotMatch(attributes, /#\[(?:allow|expect)\(dead_code\)\]/);
    assert.equal(itemAttributes(source, 'pub(crate) use polaris_core_supervisor::selected_core::selected_check_source;'),
      declaration.slice(0, declaration.indexOf('pub(crate)')));
    assert.equal(itemAttributes(source, sourceDeclaration), '', 'real desktop TLS selection requires the portable production wrapper');
    const wrapper = rustMask(rustFunction(source, 'selected_source_commit'));
    assert.match(wrapper, /check_compiled_selection\(\)\?;\s*polaris_core_supervisor::selected_core::selected_source_commit\(platform\)/);
    const provider = rustMask(rustFunction(shared, 'selected_check_source'));
    assert.match(provider, /let selected = selected_manifest\(platform\)\?;/);
    assert.match(provider, /let source = selected_source_commit\(platform\)\?;/);
    assert.match(provider, /sha256_hex\(selected_contract\(\)\?\.as_bytes\(\)\)/);
    assert.match(provider, /compiled_source\(COMPILED_VALIDATION_SOURCE, platform\)\?/);
    assert.match(provider, /digest != selected\.provider_fingerprint/);
    assert.doesNotMatch(provider, /std::env|var_os|std::fs/);
    assert.match(rustFunction(shared, 'selected_source_commit'), /selected_source_from\(COMPILED_VALIDATION_SOURCE, platform\)/);
    assert.match(rustFunction(shared, 'selected_source_from'), /compiled_source\(compiled, platform\)\?/);
    assert.ok(actualCaller.includes(`${cfg}\npub(super) fn selected_core_for_test()`));
    const callerAt = actualCaller.indexOf('pub(super) fn selected_core_for_test()');
    assert.doesNotMatch(actualCaller.slice(actualCaller.lastIndexOf('\n\n', callerAt) + 2, callerAt),
      /#\[(?:allow|expect)\(dead_code\)\]/);
    assert.equal((actualCaller.match(/core_identity::selected_check_source\("linux"\)/g) ?? []).length, 1);
  };
  check(runtime);
  const actualProvider = rustFunction(selected, 'selected_check_source');
  const renamedProvider = selected.replace('fn selected_check_source(', 'fn renamed_provider(');
  for (const changed of [
    `/*\n${actualProvider}\n*/\n${renamedProvider}`,
    `const FAKE: &str = r####"${actualProvider}"####;\n${renamedProvider}`,
    `${selected}\n${actualProvider}`,
  ]) assert.throws(() => check(runtime, changed), assert.AssertionError);
  for (const replacement of ['#[cfg(test)]', '#[cfg(target_os = "linux")]',
    '#[cfg(all(test, target_os = "linux", target_arch = "aarch64"))]',
    '#[allow(dead_code)]\n' + cfg]) {
    const changed = caller.replace(`${cfg}\npub(super) fn selected_core_for_test()`, `${replacement}\npub(super) fn selected_core_for_test()`);
    assert.notEqual(changed, caller);
    assert.throws(() => check(runtime, selected, changed));
  }
  for (const replacement of ['', '#[cfg(test)]\n', cfg + '\n', '#[allow(dead_code)]\n' + declaration.slice(0, declaration.indexOf('pub(crate)'))]) {
    const changed = runtime.replace(declaration, replacement + declaration.slice(declaration.indexOf('pub(crate)')));
    assert.notEqual(changed, runtime);
    assert.throws(() => check(changed));
  }
  for (const [before, after] of [
    ['let selected = selected_manifest(platform)?;', 'let selected = unverified_manifest(platform)?;'],
    ['let source = selected_source_commit(platform)?;', 'let source = env_source(platform)?;'],
    ['sha256_hex(selected_contract()?.as_bytes())', 'sha256_hex(b"unverified")'],
    ['digest != selected.provider_fingerprint', 'false'],
  ]) {
    const changed = selected.replace(before, after); assert.notEqual(changed, selected);
    assert.throws(() => check(runtime, changed));
  }
  for (const attribute of ['#[cfg(any(windows, test))]', cfg, '#[cfg(feature = "native-production-verification")]']) {
    const narrowed = runtime.replace(sourceDeclaration, `${attribute}\n${sourceDeclaration}`);
    assert.notEqual(narrowed, runtime);
    assert.throws(() => check(narrowed), 'portable production selection must not be narrowed to a helper or verification build');
  }
});
