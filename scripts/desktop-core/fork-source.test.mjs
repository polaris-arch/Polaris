// Tiny local Git objects and a fake compiler prove contract wiring, never a kernel verdict.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { produceDesktopCore } from './build-core.mjs';
import { consumeDesktopBundle, coreFilename, verifyPackagedSource, writeBundleInventory } from './bundle.mjs';
import { buildInfoFingerprint, canonical, DESKTOP_TARGETS, desktopSourceManifestPath, digest, expectedTags,
  platformSourceIdentity, desktopSourceGoVersion, observeMacCodeSignature, validateMacCodeSignature, validateSourceManifest, validateSourcePins, validateSourceReceipt } from './source-graph.mjs';

const repo = join(dirname(fileURLToPath(import.meta.url)), '../..');
const candidate = 'c'.repeat(40);
const forkURL = 'https://github.com/polaris-arch/sing-box';
const write = (path, bytes) => { mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, bytes); };
const json = (path) => JSON.parse(readFileSync(path));
const git = (cwd, ...args) => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: 'pipe' }).trim();
function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'polaris-fork-contract-'));
  try {
    const repository = join(root, 'objects');
    mkdirSync(repository);
    git(repository, 'init', '--quiet');
    write(join(repository, 'go.mod'), 'module example.com/fork-fixture\n\ngo 1.25.5\n');
    write(join(repository, 'go.sum'), '');
    write(join(repository, 'release/DEFAULT_BUILD_TAGS'), expectedTags('mac-x64').join(','));
    write(join(repository, 'release/DEFAULT_BUILD_TAGS_WINDOWS'), expectedTags('win').join(','));
    write(join(repository, 'release/LDFLAGS'), '-checklinkname=0');
    git(repository, 'add', '-A');
    git(repository, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'synthetic source only');
    git(repository, 'tag', 'v1.0.0');
    git(repository, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'tag', '-a', 'polaris-v1.0.0-1', '-m', `Synthetic desktop fixture; no release capability\n\nRole: desktop\nUpstream-Tag: v1.0.0\nUpstream-Commit: ${git(repository, 'rev-parse', 'HEAD')}\nQueue: synthetic fixture only`);
    const source = { schema: 'polaris-desktop-fork-source-v1', role: 'desktop', sourceURL: forkURL,
      sourceCommit: git(repository, 'rev-parse', 'HEAD'), sourceTree: git(repository, 'rev-parse', 'HEAD^{tree}'),
      sourceTag: 'polaris-v1.0.0-1', sourceTagObject: git(repository, 'rev-parse', 'refs/tags/polaris-v1.0.0-1'),
      upstreamTag: 'v1.0.0', upstreamCommit: git(repository, 'rev-parse', 'HEAD'),
      goVersion: '1.25.5', patches: [], dependencyPatches: [] };
    const sourcePath = join(root, 'scripts/desktop-core/source-manifest.json');
    write(sourcePath, JSON.stringify(source));
    const provider = join(root, 'scripts/core-source-provision.py');
    mkdirSync(dirname(provider), { recursive: true });
    copyFileSync(join(repo, 'scripts/core-source-provision.py'), provider);
    const receipt = JSON.parse(execFileSync('python3', ['-B', provider, '--manifest', sourcePath,
      '--source', repository, '--checkout', join(root, 'initial-checkout')], { encoding: 'utf8', stdio: 'pipe' }));
    const spec = { sourceMode: 'fork-commit-v1', sourceManifestSha256: receipt.sourceManifestSha256,
      provisionerSha256: receipt.provisionerSha256, sourceReceiptFingerprint: receipt.fingerprint,
      moduleGraphSha256: receipt.moduleGraphSha256, patchedSourceTree: source.sourceTree, buildTree: source.sourceTree,
      version: '1.0.0.polaris.1', dependencyModules: [], transportPins: { 'example.com/transport': 'v1.0.0' },
      platforms: Object.fromEntries(Object.keys(DESKTOP_TARGETS).map((key) => [key, { buildTree: source.sourceTree,
        patchedModules: { requiredLinked: [], allowedAbsent: [] },
        transportModules: { requiredLinked: ['example.com/transport'], confirmedAbsent: [] }, binarySha256: null }])) };
    const manifest = { bundledCoreVersion: '1.0.0', sourceBuild: spec };
    return { root, repository, source, receipt, manifest };
  } catch (error) { rmSync(root, { recursive: true, force: true }); throw error; }
}
function metadata(key, file) {
  const target = DESKTOP_TARGETS[key];
  return `${file}: go1.25.5\n\tpath\texample.com/fork-fixture\n\tmod\texample.com/fork-fixture\t(devel)\n`
    + '\tdep\texample.com/transport\tv1.0.0\th1:synthetic\n'
    + `\tbuild\tGOOS=${target.goos}\n\tbuild\tGOARCH=${target.goarch}\n\tbuild\tCGO_ENABLED=${target.cgo}\n`
    + `\tbuild\t-tags=${expectedTags(key).join(',')}\n`;
}
function runner(f, key, mutate = {}) {
  const calls = [];
  const run = (command, args, options = {}) => {
    calls.push({ command, args });
    if (command === 'codesign') return { status: 0, stdout: '', stderr: args[0] === '--display' ? `CDHash=${'a'.repeat(40)}\n` : '' };
    if (command === 'git') {
      if (options.cwd === f.root) return args[0] === 'rev-parse' ? candidate : '';
      const query = args[0] === '-C' && args[2] === 'rev-parse' ? args[3] : undefined;
      if (query && Object.hasOwn(mutate, query)) return mutate[query];
      if (args[2] === 'cat-file' && mutate.tagType) return mutate.tagType;
      if (args[2] === 'for-each-ref' && mutate.annotation !== undefined) return mutate.annotation;
      // A fixed test transport maps the declared URL to tiny local objects; no network.
      const localArgs = args.map((arg) => arg === forkURL + '.git' ? f.repository : arg);
      return execFileSync(command, localArgs, { ...options, encoding: 'utf8', stdio: 'pipe' });
    }
    if (args[0]?.endsWith('core-source-provision.py')) {
      return execFileSync('python3', ['-B', ...args], { ...options, encoding: 'utf8', stdio: 'pipe' });
    }
    if (args[0] === 'env') return '/synthetic-go';
    if (args[0] === 'version' && args.length === 1) return 'go version go1.25.5 linux/amd64';
    if (args[0] === 'version' && args[1] === '-m') return metadata(key, args[2]);
    if (args[0] === 'tool' && args[1] === 'buildid') {
      const receipt = existsSync(`${args[2]}.source-receipt.json`) ? json(`${args[2]}.source-receipt.json`).sourceReceipt : f.receipt;
      return platformSourceIdentity(receipt, f.source, f.manifest.sourceBuild, key).buildID;
    }
    if (args[0] === 'build') { write(args[args.indexOf('-o') + 1], `synthetic ${key}`); return ''; }
    if (args[0] === 'test') return ''; // Unit-test command wiring only; no Windows test executable runs.
    throw new Error(`Unexpected command ${command} ${args}`);
  };
  return { run, calls };
}
function refresh(f) {
  const raw = JSON.stringify(f.source);
  write(join(f.root, 'scripts/desktop-core/source-manifest.json'), raw);
  f.manifest.sourceBuild.sourceManifestSha256 = digest(raw);
}

// Legacy manifests/provider remain byte-pinned; this batch cannot activate production consumption.
test('current production mode and provisioner pins remain intact', () => {
  const manifest = json(join(repo, 'src-tauri/core-manifest.json'));
  assert.equal(desktopSourceManifestPath(manifest.sourceBuild), 'scripts/libbox-patches/source-manifest.json');
  assert.equal(digest(readFileSync(join(repo, 'scripts/core-source-provision.py'))), manifest.sourceBuild.provisionerSha256);
  assert.equal(manifest.sourceBuild.sourceMode, undefined);
  validateSourcePins(manifest, 'linux', false);
  validateSourceManifest(json(join(repo, 'scripts/libbox-patches/source-manifest.json')), manifest.sourceBuild);
});

test('fork contract rejects wrong repository, role, pins, overlays and shared mobile graph before commands', () => {
  const f = fixture();
  try {
    const cases = [
      ['URL', (s) => { s.sourceURL = 'https://github.com/SagerNet/sing-box'; }],
      ['role', (s) => { s.role = 'mobile'; }],
      ['schema', (s) => { s.schema = 'unknown'; }],
      ['branch head', (s) => { s.sourceTag = 'polaris-main'; }],
      ['missing tag object', (s) => { delete s.sourceTagObject; }],
      ['bad commit', (s) => { s.sourceCommit = 'HEAD'; }],
      ['upstream version', (s) => { s.upstreamTag = 'v2.0.0'; }],
      ['missing upstream', (s) => { delete s.upstreamCommit; }],
      ['source tree', (s) => { s.sourceTree = 'd'.repeat(40); }],
      ['App patch', (s) => { s.patches = [{ file: 'extra.patch', sha256: 'f'.repeat(64) }]; }],
      ['dependency patch', (s) => { s.dependencyPatches = [{}]; }],
      ['Windows overlay tree', (_, spec) => { spec.platforms.win.buildTree = 'e'.repeat(40); }],
      ['mixed mobile tree', (_, spec) => { spec.platforms['mac-arm64'].buildTree = 'e'.repeat(40); }],
      ['unknown mode', (_, spec) => { spec.sourceMode = 'fork-latest'; }],
      ['missing activation', (_, spec) => { delete spec.sourceMode; }],
    ];
    assert.throws(() => validateSourcePins({ ...f.manifest, windowsBuild: {} }, 'linux', false), /Windows overlay/);
    for (const [name, mutate] of cases) {
      const source = structuredClone(f.source), spec = structuredClone(f.manifest.sourceBuild);
      mutate(source, spec);
      const attempt = { ...f, source, manifest: { ...f.manifest, sourceBuild: spec } };
      refresh(attempt);
      const r = runner(attempt, 'linux');
      assert.throws(() => produceDesktopCore(f.root, attempt.manifest, 'linux', join(f.root, 'rejected'), candidate, r.run), undefined, name);
      assert.equal(r.calls.length, 0, name);
      assert.equal(existsSync(join(f.root, 'rejected')), false, name);
    }
  } finally { rmSync(f.root, { recursive: true, force: true }); }
});

test('named tag object, annotation, commit, upstream and tree are verified before provisioning', () => {
  const f = fixture();
  try {
    const cases = [
      { [`refs/tags/${f.source.sourceTag}`]: 'e'.repeat(40) },
      { tagType: 'commit' },
      { annotation: `Role: mobile\nUpstream-Tag: ${f.source.upstreamTag}\nUpstream-Commit: ${f.source.upstreamCommit}` },
      { annotation: `Role: desktop\nRole: desktop\nUpstream-Tag: ${f.source.upstreamTag}\nUpstream-Commit: ${f.source.upstreamCommit}` },
      { annotation: `Role: desktop\nUpstream-Tag: v2.0.0\nUpstream-Commit: ${f.source.upstreamCommit}` },
      { [`refs/tags/${f.source.sourceTag}^{commit}`]: 'e'.repeat(40) },
      { [`refs/tags/${f.source.upstreamTag}^{commit}`]: 'e'.repeat(40) },
      { [`${f.source.sourceCommit}^{tree}`]: 'e'.repeat(40) },
    ];
    for (const mutate of cases) {
      const r = runner(f, 'linux', mutate);
      assert.throws(() => produceDesktopCore(f.root, f.manifest, 'linux', join(f.root, 'rejected'), candidate, r.run), /Fork/);
      assert.equal(r.calls.some((call) => call.args[0]?.endsWith('core-source-provision.py')), false);
      assert.equal(existsSync(join(f.root, 'rejected')), false);
    }
  } finally { rmSync(f.root, { recursive: true, force: true }); }
});

test('all four fork producers, bundle consumer and packaging use the same pinned graph without overlay replay', () => {
  const f = fixture();
  try {
    const bundle = join(f.root, 'bundle');
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      const r = runner(f, key);
      const receipt = produceDesktopCore(f.root, f.manifest, key, join(bundle, key, coreFilename(key)), candidate, r.run);
      assert.deepEqual(receipt.overlays, []);
      assert.equal(receipt.sourceReceipt.sourceURL, forkURL);
      assert.equal(r.calls.some((call) => call.args.includes('apply')), false);
      assert.equal(r.calls.filter((call) => call.args.includes('test')).length, key === 'win' ? 1 : 0);
    }
    writeBundleInventory(bundle, candidate);
    const inspect = (command, args, options) => command === 'git' ? (args[0] === 'rev-parse' ? candidate : '')
      : runner(f, args[2].split(/[/\\]/).at(-2)).run(command, args, options);
    consumeDesktopBundle(f.root, f.manifest, bundle, candidate, ['linux'], inspect);
    verifyPackagedSource(f.root, f.manifest, 'linux', join(f.root, 'resources/linux/sing-box'));
    assert.equal(existsSync(join(f.root, 'resources/win/sing-box.exe')), false);
    assert.equal(readFileSync(join(f.root, 'resources/linux/sing-box'), 'utf8'), 'synthetic linux');
    // A self-consistent receipt rewrite cannot change the pinned source graph.
    const bad = structuredClone(f.receipt);
    bad.upstreamTree = 'e'.repeat(40);
    const { fingerprint, ...facts } = bad;
    bad.fingerprint = digest(canonical(facts));
    assert.throws(() => validateSourceReceipt(bad, f.source, { ...f.manifest.sourceBuild, sourceReceiptFingerprint: bad.fingerprint }), /commit tree/);
    assert.equal(buildInfoFingerprint(metadata('linux', 'a')), buildInfoFingerprint(metadata('linux', 'b')));
  } finally { rmSync(f.root, { recursive: true, force: true }); }
});

test('desktop Go pin reads the selected desktop manifest, rejects mobile role and missing byte pin', () => {
  const f = fixture();
  try {
    write(join(f.root, 'scripts/libbox-patches/source-manifest.json'), JSON.stringify({ goVersion: '1.26.0', role: 'mobile' }));
    assert.equal(desktopSourceGoVersion(f.root, f.manifest), '1.25.5');
    const selected = join(f.root, 'scripts/desktop-core/source-manifest.json');
    write(selected, JSON.stringify({ ...f.source, goVersion: '1.26.0' }));
    assert.throws(() => desktopSourceGoVersion(f.root, f.manifest), /SHA-256/);
    f.source.role = 'mobile'; refresh(f);
    assert.throws(() => desktopSourceGoVersion(f.root, f.manifest), /role/);
    f.source.role = 'desktop'; refresh(f);
    delete f.manifest.sourceBuild.sourceManifestSha256;
    assert.throws(() => desktopSourceGoVersion(f.root, f.manifest), /SHA-256/);
  } finally { rmSync(f.root, { recursive: true, force: true }); }
});

test('Mac observation requires strict success/unique CDHash or the precise thin Intel unsigned exception', () => {
  const signedRun = (_command, args) => ({ status: 0, stdout: '', stderr: args[0] === '--display' ? `CDHash=${'A'.repeat(40)}\n` : '' });
  assert.deepEqual(observeMacCodeSignature('/synthetic', 'mac-arm64', signedRun), { state: 'signed', cdHash: 'a'.repeat(40) });
  for (const key of ['mac-x64', 'mac-arm64']) {
    for (const signature of [undefined, {}, { state: 'invalid', cdHash: null }, { state: 'signed', cdHash: null },
      { state: 'signed', cdHash: 'a'.repeat(40), extra: true }]) assert.throws(() => validateMacCodeSignature(signature, key));
  }
  assert.throws(() => validateMacCodeSignature({ state: 'unsigned', cdHash: null }, 'mac-arm64'));
  assert.throws(() => validateMacCodeSignature({ state: 'signed', cdHash: 'a'.repeat(40) }, 'linux'));
  for (const kind of ['Mach-O universal x86_64 arm64', 'Mach-O fat x86_64', 'ASCII text', 'Mach-O arm64']) {
    const unsignedRun = (command) => command === 'file' ? { status: 0, stdout: kind, stderr: '' }
      : { status: 1, stdout: '', stderr: 'core: code object is not signed at all' };
    assert.throws(() => observeMacCodeSignature('/synthetic', 'mac-x64', unsignedRun), /thin Intel/);
  }
  assert.throws(() => observeMacCodeSignature('/synthetic', 'mac-x64', () => ({ status: 1, stderr: 'invalid signature' })), /invalid or missing/);
  assert.throws(() => observeMacCodeSignature('/synthetic', 'mac-x64', () => ({ status: null, error: Error('missing tool') })), /Cannot inspect/);
  assert.throws(() => observeMacCodeSignature('/synthetic', 'mac-arm64', (_command, args) => ({ status: 0, stderr:
    args[0] === '--display' ? `CDHash=${'a'.repeat(40)}\nCDHash=${'b'.repeat(40)}\n` : '' })), /unique actual/);
});
