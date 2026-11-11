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
  assert.equal((emit.match(/(?<![\w:])embed_selected_core_identity\(\);/g) ?? []).length, 1);
}
test('App build delegates selection to the exact shared module and emit entry', () => {
  assertBuildDelegation(buildEntry, buildSource);
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
    buildSource.replace('embed_selected_core_identity();', '/* embed_selected_core_identity(); */ unreviewed_selection();'),
    buildSource.replace('embed_selected_core_identity();', 'let _ = "embed_selected_core_identity();"; unreviewed_selection();'),
  ]) assert.throws(() => assertBuildDelegation(buildEntry, changed), assert.AssertionError);
});
function assertBuildDigestSource(source) {
  const helper = rustFunction(source, 'sha256_hex');
  assert.match(helper, /for byte in sha2::Sha256::digest\(bytes\)\s*\{/);
  assert.match(helper, /write!\(&mut hex, "\{byte:02x\}"\)/,
    'sha2 digest arrays require per-byte lowercase, zero-padded hex');
  const identity = rustFunction(source, 'embed_selected_core_identity');
  assert.match(identity, /InstalledCoreManifest::parse\(&raw, sha256_hex\)/,
    'manifest identity must use the shared encoder');
}
test('build identity shares byte-wise SHA-256 encoding', () => {
  assertBuildDigestSource(buildSource);
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

function assertUnavailableAnalysisSource(source) {
  const pending = rustFunction(source, 'installation_pending');
  assert.match(pending, /!formal_default/);
  assert.match(pending, /var_os\("POLARIS_CORE_IDENTITY_FILE"\)\.is_some\(\)/);
  assert.match(pending, /\.join\("core-manifest.json"\)/);
  assert.match(pending, /symlink_metadata\(&path\)\.is_ok\(\) \{\s*return false;/);
  assert.match(pending, /try_exists\(\)\s*\.expect\("inspect selected installation path"\)/);
  assert.match(source, /const FORMAL_UNAVAILABLE: &\[u8\] =\s*br#"\{"schema":"polaris-formal-desktop-unavailable-v1"\}"#/);
  assert.match(rustFunction(source, 'embed_selected_core_identity'), /installation_pending\(formal_default\)/);
}
test('analysis image cannot silently use a missing or partially frozen desktop identity', () => {
  assertUnavailableAnalysisSource(buildSource);
});
function assertFormalBuildSource(source) {
  const selection = rustFunction(source, 'formal_desktop_selection');
  assert.match(selection, /desktop_platform\(\)\.is_none\(\)/);
  assert.match(selection, /selection\["contractSha256"\]\.as_str\(\),\s*Some\(sha256_hex\(&raw\)\.as_str\(\)\)/);
  assert.match(selection, /assert_eq!\(\s*selection\["contractPath"\]\.as_str\(\),\s*Some\(CONTRACT_PATH\)/);
  assert.match(source, /const CONTRACT_PATH: &str = "scripts\/desktop-core\/release-contract.json";/);
  source = rustFunction(source, 'embed_selected_core_identity');
  assert.match(source, /installation_pending\(formal_default\)/);
  assert.match(source, /formal_default\.then\(/);
  const verify = source.match(/if formal_default \{[\s\S]*?(?=\n            assert_eq!\(\s*manifest.core.platform)/)?.[0] ?? '';
  assert.match(verify, /directory = root.join\("resources"\).join\(platform\)/);
  assert.match(verify, /assert_eq!\(\s*path,\s*directory.join\("core-manifest.json"\)/);
  assert.match(verify, /\.arg\("--mode=verify"\)/);
  assert.match(verify, /verified.status.success\(\)/);
  assert.match(verify, /raw,\s*std::fs::read\(&path\).expect\("verified formal identity readable"\)/);
}
test('formal default compiles only exact verified installation with real helper and cleaner bytes', () => {
  assertFormalBuildSource(buildSource);
  const prepare = readFileSync(new URL('./prepare-core-identity.mjs', import.meta.url), 'utf8');
  assert.match(prepare, /contract = releaseInstallationContract\(root, input\);[\s\S]*?verifyPackagedRelease\(root, input, platform, join\(directory, contract.platforms\[platform\].core.member\)\)/);
  assert.match(prepare, /actualCoreFile\(join\(directory, helper\)\).sha256 !== old.core.helperSha256/);
  assert.match(prepare, /actualCoreFile\(join\(directory, 'polaris-cleaner.exe'\)\).sha256 !== old.core.cleanerSha256/);
});

