import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync, cpSync, existsSync } from 'node:fs';
import { spawnSync, execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { digest } from './desktop-core/source-graph.mjs';
import test from 'node:test';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  CRONET_TARGETS,
  parseCronetGoModRequires,
  resolveCronetRequest,
  selectCronetPlatforms,
  validateCronetLibraryPins,
} from './lib/cronet-contract.mjs';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const VERSION = 'v0.0.0-20260831030607-f80ef37265e5';
const LINUX = 'github.com/sagernet/cronet-go/lib/linux_amd64';
const WINDOWS = 'github.com/sagernet/cronet-go/lib/windows_amd64';

const block = (linux = VERSION, windows = VERSION) => `
require (
  ${LINUX} ${linux} // indirect
  ${WINDOWS} ${windows}
  go.uber.org/zap v1.27.1
)
`;

test('Cronet manifest pins extracted libraries, not a human Chromium label', () => {
  const manifest = JSON.parse(readFileSync(join(ROOT, 'src-tauri/core-manifest.json'), 'utf8'));
  assert.equal('cronetVersion' in manifest, false);
  assert.equal('cronetModuleVersion' in manifest, false);
  assert.deepEqual(Object.keys(manifest.cronetLibrarySha256).sort(), ['linux', 'win']);
  assert.equal('cronetArchiveSha256' in manifest, false);
  assert.deepEqual(validateCronetLibraryPins(manifest.cronetLibrarySha256), manifest.cronetLibrarySha256);
});

test('Cronet platform selection accepts an exact combination and defaults to all', () => {
  assert.equal(selectCronetPlatforms([]), null);
  assert.deepEqual(selectCronetPlatforms(['--platform=linux,win']), ['linux', 'win']);
});

test('Cronet request resolution maps each selected platform to its exact dynamic library', () => {
  const describe = (argv) => resolveCronetRequest(argv).targets.map(({ key, module, member, out }) => ({ key, module, member, out }));
  assert.deepEqual(CRONET_TARGETS.map(({ key }) => key), ['linux', 'win']);
  assert.deepEqual(describe(['--platform=linux']), [
    { key: 'linux', module: 'linux_amd64', member: 'libcronet.so', out: 'libcronet.so' },
  ]);
  assert.deepEqual(describe(['--platform=win']), [
    { key: 'win', module: 'windows_amd64', member: 'libcronet.dll', out: 'libcronet.dll' },
  ]);
  assert.deepEqual(describe(['--platform=linux,win']), [
    { key: 'linux', module: 'linux_amd64', member: 'libcronet.so', out: 'libcronet.so' },
    { key: 'win', module: 'windows_amd64', member: 'libcronet.dll', out: 'libcronet.dll' },
  ]);
});

test('Cronet platform selection is fail-closed for malformed choices', () => {
  const cases = [
    [['--platform='], /值为空/],
    [['--platform=linux,,win'], /空分段/],
    [['--platform=,linux'], /空分段/],
    [['--platform=linux,'], /空分段/],
    [['--platform=linux,linux'], /重复平台 linux/],
    [['--platform=mac'], /未知平台 mac/],
    [['--platform'], /必须写成/],
    [['--platform=linux', '--platform=bogus'], /只能给一次/],
    [['--platform', '--platform=linux'], /只能给一次/],
  ];
  for (const [argv, error] of cases) {
    assert.throws(() => selectCronetPlatforms(argv), error, argv.join(' '));
  }
  assert.throws(() => resolveCronetRequest(['--skip-gomod-check']), /已移除/);
  assert.throws(() => resolveCronetRequest(['--check-only', '--force']), /互斥/);
  assert.throws(() => resolveCronetRequest(['--unexpected']), /未知参数/);
});

test('Cronet go.mod contract accepts both legal require forms', () => {
  const goMod = `
module example.invalid

require ${LINUX} ${VERSION}
require (
  ${WINDOWS} ${VERSION}
)
`;
  assert.deepEqual(parseCronetGoModRequires(goMod), { linux: VERSION, win: VERSION });
});

test('Cronet go.mod parser accepts whitespace variants of a require block opener', () => {
  for (const opener of ['require  (', 'require\t(', 'require(']) {
    const goMod = `${opener}\n  ${LINUX} ${VERSION}\n  ${WINDOWS} ${VERSION}\n)\n`;
    assert.deepEqual(parseCronetGoModRequires(goMod), { linux: VERSION, win: VERSION }, opener);
  }
});

test('Cronet selected platforms and go.mod contract compose on the happy path', () => {
  assert.deepEqual(selectCronetPlatforms(['--platform=win']), ['win']);
  assert.deepEqual(parseCronetGoModRequires(block()), { linux: VERSION, win: VERSION });
});

test('Cronet library pins are complete, exact, and fail closed', () => {
  const pins = {
    linux: 'a'.repeat(64),
    win: `sha256:${'b'.repeat(64)}`,
  };
  assert.deepEqual(validateCronetLibraryPins(pins), {
    linux: 'a'.repeat(64),
    win: 'b'.repeat(64),
  });
  assert.throws(() => validateCronetLibraryPins({ linux: 'a'.repeat(64) }), /平台集合/);
  assert.throws(
    () => validateCronetLibraryPins({ linux: 'a'.repeat(64), win: 'B'.repeat(64) }),
    /64 位小写 SHA-256/,
  );
});

test('Cronet go.mod parser rejects a missing module but permits platform version divergence', () => {
  assert.throws(
    () => parseCronetGoModRequires(`require ${LINUX} ${VERSION}\n`),
    new RegExp(`缺少 ${WINDOWS}`),
  );
  assert.throws(
    () => parseCronetGoModRequires(`require ${WINDOWS} ${VERSION}\n`),
    new RegExp(`缺少 ${LINUX}`),
  );
  assert.deepEqual(parseCronetGoModRequires(block(VERSION, 'v0.0.0-other')), {
    linux: VERSION,
    win: 'v0.0.0-other',
  });
});

test('Cronet go.mod parser ignores comments, prefixes, and cross-line bait', () => {
  const decoys = [
    [`// require ${LINUX} ${VERSION}`, new RegExp(`缺少 ${LINUX}`)],
    [`require ${LINUX}-extra ${VERSION}`, new RegExp(`缺少 ${LINUX}`)],
    [`require\n${LINUX} ${VERSION}`, /非合法 require/],
  ];
  for (const [decoy, error] of decoys) {
    const goMod = `${decoy}\nrequire ${WINDOWS} ${VERSION}\n`;
    assert.throws(
      () => parseCronetGoModRequires(goMod),
      error,
      `不应把以下诱饵当作 require：${JSON.stringify(decoy)}`,
    );
  }
});

test('Cronet go.mod parser rejects nested directives and target replacements', () => {
  assert.throws(
    () => parseCronetGoModRequires(`require (\n  require ${LINUX} ${VERSION}\n  ${WINDOWS} ${VERSION}\n)\n`),
    /非法 directive/,
  );
  assert.throws(
    () => parseCronetGoModRequires(`require (\n  ${LINUX} ${VERSION}\n  replace example.invalid/other v1.0.0 => ./local-other\n  ${WINDOWS} ${VERSION}\n)\n`),
    /非法 directive/,
  );
  assert.throws(
    () => parseCronetGoModRequires(`${block()}replace ${LINUX} ${VERSION} => ./local-cronet\n`),
    /replace 改写 Cronet 目标模块/,
  );
  assert.throws(
    () => parseCronetGoModRequires(`${block()}replace (\n  ${WINDOWS} ${VERSION} => example.invalid/cronet ${VERSION}\n)\n`),
    /replace \(\.\.\.\) 改写 Cronet 目标模块/,
  );
});

test('Cronet go.mod parser allows unrelated replace and ignores target names in comments', () => {
  const goMod = `${block()}replace example.invalid/other v1.0.0 => ./local-other\n// replace ${LINUX} ${VERSION} => ./local-cronet\n`;
  assert.deepEqual(parseCronetGoModRequires(goMod), { linux: VERSION, win: VERSION });
});

test('Cronet go.mod parser rejects duplicate and unterminated require declarations', () => {
  assert.throws(
    () => parseCronetGoModRequires(`${block()}require ${LINUX} ${VERSION}\n`),
    /声明重复出现/,
  );
  assert.throws(
    () => parseCronetGoModRequires(`require (\n  ${LINUX} ${VERSION}\n  ${WINDOWS} ${VERSION}\n`),
    /未闭合/,
  );
});


// Execute the real CLI with a local curl transport and real zip extraction.
// Synthetic pins here are not production D release/tag receipts.
function cronetCliFixture() {
  const root = mkdtempSync(join(tmpdir(), 'polaris-cronet-cli-'));
  const write = (relative, bytes, options) => {
    const path = join(root, relative); mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, bytes, options);
  };
  cpSync(join(ROOT, 'scripts'), join(root, 'scripts'), { recursive: true });
  const source = { schema: 'polaris-desktop-fork-source-v1', role: 'desktop',
    sourceURL: 'https://github.com/polaris-arch/polaris-box', sourceCommit: 'a'.repeat(40), sourceTree: 'b'.repeat(40),
    sourceTag: 'polaris-v2.0.0-alpha.11-1', sourceTagObject: 'c'.repeat(40), upstreamTag: 'v2.0.0-alpha.11',
    upstreamCommit: 'd'.repeat(40), goVersion: '1.25.5', patches: [], dependencyPatches: [] };
  write('scripts/desktop-core/source-manifest.json', JSON.stringify(source));
  const library = Buffer.from('synthetic current Cronet library');
  const goMod = `module example.invalid/fixture\n\ngo 1.25.5\n${block()}`;
  write('fixture.go.mod', goMod);
  const manifest = JSON.parse(readFileSync(join(ROOT, 'src-tauri/core-manifest.json')));
  manifest.desktopSourceBuild = { sourceMode: 'fork-commit-v1', upstreamVersion: '2.0.0-alpha.11',
    version: '2.0.0-alpha.11.polaris.1', graphScope: 'core-source-only',
    sourceManifestSha256: digest(JSON.stringify(source)), provisionerSha256: digest(readFileSync(join(root, 'scripts/core-source-provision.py'))),
    sourceReceiptFingerprint: 'e'.repeat(64), moduleGraphSha256: digest('[]'), mainGoModSha256: digest(goMod),
    mainGoSumSha256: digest(''), patchedSourceTree: source.sourceTree, buildTree: source.sourceTree,
    dependencyModules: [], transportPins: { 'example.invalid/transport': 'v1.0.0' },
    platforms: Object.fromEntries(['linux', 'win', 'mac-x64', 'mac-arm64'].map((key) => [key, {
      buildTree: source.sourceTree, binarySha256: null, patchedModules: { requiredLinked: [], allowedAbsent: [] },
      transportModules: { requiredLinked: ['example.invalid/transport'], confirmedAbsent: [] } }])) };
  delete manifest.windowsBuild;
  manifest.cronetLibrarySha256 = { linux: digest(library), win: 'f'.repeat(64) };
  write('bin/curl', `#!/usr/bin/env node\nconst fs=require('fs'),path=require('path'); const root=${JSON.stringify(root)};\n`
    + `const args=process.argv.slice(2); fs.appendFileSync(path.join(root,'curl-calls.jsonl'), JSON.stringify(args)+'\\n');\n`
    + `const out=args.indexOf('-o'); if(out>=0) fs.copyFileSync(path.join(root,'fixture.zip'),args[out+1]); else process.stdout.write(fs.readFileSync(path.join(root,'fixture.go.mod')));\n`, { mode: 0o755 });
  const calls = () => existsSync(join(root, 'curl-calls.jsonl')) ? readFileSync(join(root, 'curl-calls.jsonl'), 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse) : [];
  const run = (...args) => {
    write('src-tauri/core-manifest.json', JSON.stringify(manifest));
    write('curl-calls.jsonl', '');
    return spawnSync(process.execPath, [join(root, 'scripts/fetch-cronet.mjs'), ...args], {
      cwd: root, encoding: 'utf8', env: { ...process.env, PATH: `${join(root, 'bin')}:${process.env.PATH}` },
    });
  };
  const zip = (bytes) => {
    write('fixture-library', bytes);
    execFileSync('python3', ['-c', "import sys,zipfile; z=zipfile.ZipFile(sys.argv[1],'w'); z.write(sys.argv[2],'github.com/sagernet/cronet-go/lib/linux_amd64@fixture/libcronet.so'); z.close()", join(root, 'fixture.zip'), join(root, 'fixture-library')]);
  };
  return { root, source, manifest, library, goMod, write, calls, run, zip, close: () => rmSync(root, { recursive: true, force: true }) };
}

test('actual Cronet CLI binds raw D go.mod before check-only and cache/stamp admission', () => {
  const f = cronetCliFixture();
  try {
    const valid = f.run('--check-only');
    assert.equal(valid.status, 0, valid.stderr);
    assert.match(valid.stdout, /v2.0.0-alpha.11/);
    assert.deepEqual(f.calls().map((args) => args.at(-1)), [`https://raw.githubusercontent.com/polaris-arch/polaris-box/${f.source.sourceCommit}/go.mod`]);
    const stamp = JSON.stringify({ 'cronet:linux': `${VERSION}|${digest(f.library)}`, unrelated: 'retained' });
    f.write('resources/linux/libcronet.so', f.library);
    f.write('resources/.fetch-stamp.json', stamp);
    for (const args of [['--check-only'], ['--platform=linux']]) {
      f.write('fixture.go.mod', f.goMod + '\n');
      const result = f.run(...args);
      assert.equal(result.status, 1);
      assert.match(result.stderr, /Fork Cronet go.mod SHA-256/);
      assert.equal(f.calls().length, 1, 'no archive download after bad raw bytes');
      assert.equal(readFileSync(join(f.root, 'resources/.fetch-stamp.json'), 'utf8'), stamp);
      assert.deepEqual(readFileSync(join(f.root, 'resources/linux/libcronet.so')), f.library);
    }
    f.write('fixture.go.mod', f.goMod);
    const cached = f.run('--platform=linux');
    assert.equal(cached.status, 0, cached.stderr);
    assert.match(cached.stdout, /skip \(up to date\)/);
    assert.equal(f.calls().length, 1, 'even a current stamp fetches and verifies D go.mod');
  } finally { f.close(); }
});

test('actual Cronet CLI rejects D/M source swap, bad manifest bytes and invalid D before curl', () => {
  const f = cronetCliFixture();
  try {
    const spec = structuredClone(f.manifest.desktopSourceBuild);
    for (const invalid of [null, [], {}, f.manifest.sourceBuild]) {
      f.manifest.desktopSourceBuild = invalid;
      assert.equal(f.run('--check-only').status, 1);
      assert.deepEqual(f.calls(), []);
    }
    f.manifest.desktopSourceBuild = spec;
    f.write('scripts/desktop-core/source-manifest.json', JSON.stringify({ ...f.source, role: 'mobile' }));
    assert.match(f.run('--check-only').stderr, /SHA-256/);
    assert.deepEqual(f.calls(), []);
    for (const source of [{ ...f.source, role: 'mobile' }, { ...f.source, sourceURL: 'https://github.com/SagerNet/sing-box' }]) {
      f.write('scripts/desktop-core/source-manifest.json', JSON.stringify(source));
      spec.sourceManifestSha256 = digest(JSON.stringify(source));
      assert.match(f.run('--check-only').stderr, /repository\/schema\/role/);
      assert.deepEqual(f.calls(), []);
    }
  } finally { f.close(); }
});

test('actual Cronet CLI retains both module checks and replace rejection after valid raw hash', () => {
  const f = cronetCliFixture();
  try {
    for (const text of [block() + `replace ${LINUX} ${VERSION} => ./local\n`, `require ${LINUX} ${VERSION}\n`, `require ${WINDOWS} ${VERSION}\n`]) {
      f.write('fixture.go.mod', text);
      f.manifest.desktopSourceBuild.mainGoModSha256 = digest(text);
      const result = f.run('--check-only');
      assert.equal(result.status, 1);
      assert.match(result.stderr, /replace|缺少/);
      assert.equal(f.calls().length, 1);
    }
    delete f.manifest.cronetLibrarySha256.win;
    assert.equal(f.run('--check-only').status, 1);
    assert.deepEqual(f.calls(), []);
  } finally { f.close(); }
});

test('actual Cronet CLI verifies cached/extracted library bytes and stamps only successful atomic replacement', () => {
  const f = cronetCliFixture();
  try {
    const old = Buffer.from('stale or tampered cached library');
    const stamp = JSON.stringify({ 'cronet:linux': `${VERSION}|${digest(f.library)}`, unrelated: 'retained' });
    f.write('resources/linux/libcronet.so', old);
    f.write('resources/.fetch-stamp.json', stamp);
    f.zip(Buffer.from('wrong extracted library'));
    const rejected = f.run('--platform=linux');
    assert.equal(rejected.status, 1, rejected.stdout);
    assert.match(rejected.stderr, /sha256 不符/);
    assert.equal(f.calls().length, 2, 'tampered cache cannot reuse matching stamp');
    assert.deepEqual(readFileSync(join(f.root, 'resources/linux/libcronet.so')), old);
    assert.equal(readFileSync(join(f.root, 'resources/.fetch-stamp.json'), 'utf8'), stamp);
    assert.equal(existsSync(join(f.root, 'resources/linux/libcronet.so.tmp')), false);
    f.zip(f.library);
    const accepted = f.run('--platform=linux');
    assert.equal(accepted.status, 0, accepted.stderr);
    assert.deepEqual(readFileSync(join(f.root, 'resources/linux/libcronet.so')), f.library);
    assert.deepEqual(JSON.parse(readFileSync(join(f.root, 'resources/.fetch-stamp.json'))), JSON.parse(stamp));
    assert.equal(existsSync(join(f.root, 'resources/linux/libcronet.so.tmp')), false);
  } finally { f.close(); }
});
