// Tiny local Git objects and a fake compiler prove contract wiring, never a kernel verdict.
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { produceDesktopCore, withProviderMetadataOutside } from './build-core.mjs';
import { consumeDesktopBundle, coreFilename, verifyPackagedSource, writeBundleInventory } from './bundle.mjs';
import { buildInfoFingerprint, canonical, DESKTOP_TARGETS, desktopSourceManifestPath, digest, expectedTags,
  platformSourceIdentity, desktopSourceGoVersion, observeMacCodeSignature, validateMacCodeSignature, validateSourceManifest, validateSourcePins, validateSourceReceipt, validateForkNotice, sourceOwnedReplacements, validateBuildInfo, verifySourceOwnedCheckout, producerContext } from './source-graph.mjs';

const repo = join(dirname(fileURLToPath(import.meta.url)), '../..');
const candidate = 'c'.repeat(40);
const forkURL = 'https://github.com/polaris-arch/polaris-box';
const upstreamURL = 'https://github.com/SagerNet/sing-box.git';
const write = (path, bytes) => { mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, bytes); };
const json = (path) => JSON.parse(readFileSync(path));
const git = (cwd, ...args) => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: 'pipe' }).trim();
function fixture(owned = false) {
  const root = mkdtempSync(join(tmpdir(), 'polaris-fork-contract-'));
  try {
    const upstreamRepository = join(root, 'upstream-objects');
    mkdirSync(upstreamRepository);
    git(upstreamRepository, 'init', '--quiet');
    write(join(upstreamRepository, 'go.mod'), owned ? 'module github.com/sagernet/sing-box\n\ngo 1.25.5\n' : 'module example.com/fork-fixture\n\ngo 1.25.5\n');
    write(join(upstreamRepository, 'go.sum'), '');
    // Match alpha.11: the shared default no longer includes gvisor.
    write(join(upstreamRepository, 'release/DEFAULT_BUILD_TAGS'), expectedTags('mac-x64').filter((tag) => tag !== 'with_gvisor').join(','));
    write(join(upstreamRepository, 'release/DEFAULT_BUILD_TAGS_WINDOWS'), expectedTags('win').join(','));
    write(join(upstreamRepository, 'release/LDFLAGS'), '-checklinkname=0');
    git(upstreamRepository, 'add', '-A');
    git(upstreamRepository, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'synthetic upstream only');
    git(upstreamRepository, 'tag', 'v1.0.0');
    const upstreamCommit = git(upstreamRepository, 'rev-parse', 'HEAD');
    const repository = join(root, 'objects');
    git(root, 'clone', '--quiet', '--no-hardlinks', upstreamRepository, repository);
    git(repository, 'tag', '-d', 'v1.0.0');
    write(join(repository, 'fork-fixture.txt'), 'synthetic reviewed desktop change\n');
    if (owned) {
      for (let i = 0; i < 256; i++) write(join(repository, 'third_party/sing-tun', `file-${i}.go`), `// synthetic ${i}\n`);
      write(join(repository, 'release/dependency-sources/sing-tun-replacement.json'), 'synthetic binding\n');
      write(join(repository, 'release/dependency-sources/sing-tun-windows-auto-route.patch'), 'synthetic patch\n');
    }
    git(repository, 'add', '-A');
    git(repository, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'synthetic fork only');
    git(repository, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'tag', '-a', 'polaris-v1.0.0-1', '-m', `Synthetic desktop fixture; no release capability\n\nRole: desktop\nUpstream-Tag: v1.0.0\nUpstream-Commit: ${upstreamCommit}\nQueue: synthetic fixture only`);
    const source = { schema: 'polaris-desktop-fork-source-v1', role: 'desktop', sourceURL: forkURL,
      sourceCommit: git(repository, 'rev-parse', 'HEAD'), sourceTree: git(repository, 'rev-parse', 'HEAD^{tree}'),
      sourceTag: 'polaris-v1.0.0-1', sourceTagObject: git(repository, 'rev-parse', 'refs/tags/polaris-v1.0.0-1'),
      upstreamTag: 'v1.0.0', upstreamCommit,
      goVersion: '1.25.5', patches: [], dependencyPatches: [] };
    if (owned) source.sourceOwnedReplacements = [{ schemaVersion: 1, module: 'github.com/sagernet/sing-tun',
      version: 'v0.9.7-0.20261009022811-5c2edb183cc9', replacementPath: './third_party/sing-tun',
      replacementTree: git(repository, 'rev-parse', 'HEAD:third_party/sing-tun'), replacementFileCount: 256,
      baselineOriginCommit: 'b'.repeat(40), baselineZipSha256: 'a'.repeat(64), baselineModuleSum: `h1:${'A'.repeat(43)}=`,
      baselineFiles: 253, changedBaselineFiles: ['tun_windows.go'], addedFiles: ['windows_dns_configuration.go',
        'windows_dns_configuration_test.go', 'windows_lifecycle_guard_windows_test.go'], other252BaselineFilesByteExact: true,
      windowsAfterDNSSuffixUnchanged: true, patchSha256: digest('synthetic patch\n'),
      fingerprintSchema: 'fork-source-fingerprint-v1', fourKernelPolicy: 'synthetic fixture only; all four required',
      bindingSha256: digest('synthetic binding\n'), bindingPath: 'release/dependency-sources/sing-tun-replacement.json',
      patchPath: 'release/dependency-sources/sing-tun-windows-auto-route.patch' }];
    const sourcePath = join(root, 'scripts/desktop-core/source-manifest.json');
    write(sourcePath, JSON.stringify(source));
    const provider = join(root, 'scripts/core-source-provision.py');
    mkdirSync(dirname(provider), { recursive: true });
    copyFileSync(join(repo, 'scripts/core-source-provision.py'), provider);
    const receipt = JSON.parse(execFileSync('python3', ['-B', provider, '--manifest', sourcePath,
      '--source', repository, '--checkout', join(root, 'initial-checkout')], { encoding: 'utf8', stdio: 'pipe' }));
    const spec = { sourceMode: 'fork-commit-v1', upstreamVersion: '1.0.0', graphScope: 'core-source-only',
      mainGoModSha256: receipt.mainGoModSha256, mainGoSumSha256: receipt.mainGoSumSha256, sourceManifestSha256: receipt.sourceManifestSha256,
      provisionerSha256: receipt.provisionerSha256, sourceReceiptFingerprint: receipt.fingerprint,
      moduleGraphSha256: receipt.moduleGraphSha256, patchedSourceTree: source.sourceTree, buildTree: source.sourceTree,
      version: '1.0.0.polaris.1', dependencyModules: [], transportPins: { 'example.com/transport': 'v1.0.0' },
      platforms: Object.fromEntries(Object.keys(DESKTOP_TARGETS).map((key) => [key, { buildTree: source.sourceTree,
        patchedModules: { requiredLinked: [], allowedAbsent: [] },
        transportModules: { requiredLinked: ['example.com/transport'], confirmedAbsent: [] }, binarySha256: null }])) };
    if (owned) spec.sourceOwnedReplacements = structuredClone(source.sourceOwnedReplacements);
    const manifest = { ...json(join(repo, 'src-tauri/core-manifest.json')), desktopSourceBuild: spec };
    delete manifest.windowsBuild;
    write(join(root, 'src-tauri/core-manifest.json'), JSON.stringify(manifest));
    return { root, repository, upstreamRepository, source, receipt, manifest };
  } catch (error) { rmSync(root, { recursive: true, force: true }); throw error; }
}
function metadata(key, file, source) {
  const target = DESKTOP_TARGETS[key];
  if (source?.sourceOwnedReplacements) return `${file}: go1.25.5\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n\tmod\tgithub.com/sagernet/sing-box\tv0.0.0-fixture\n`
    + `\tdep\tgithub.com/sagernet/sing-tun\t${source.sourceOwnedReplacements[0].version}\n\t=>\t./third_party/sing-tun\t(devel)\n\t\n`
    + '\tdep\texample.com/transport\tv1.0.0\th1:synthetic\n'
    + `\tbuild\tGOOS=${target.goos}\n\tbuild\tGOARCH=${target.goarch}\n\tbuild\tCGO_ENABLED=${target.cgo}\n`
    + `\tbuild\t-tags=${expectedTags(key).join(',')}\n\tbuild\tvcs.revision=${source.sourceCommit}\n\tbuild\tvcs.modified=false\n`;
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
      const localArgs = args.map((arg) => arg === forkURL + '.git' ? f.repository
        : arg === upstreamURL ? f.upstreamRepository : arg);
      return execFileSync(command, localArgs, { ...options, encoding: 'utf8', stdio: 'pipe' });
    }
    if (args[0]?.endsWith('core-source-provision.py')) {
      return execFileSync('python3', ['-B', ...args], { ...options, encoding: 'utf8', stdio: 'pipe' });
    }
    if (args[0] === 'env') return '/synthetic-go';
    if (args[0] === 'version' && args.length === 1) return 'go version go1.25.5 linux/amd64';
    if (args[0] === 'version' && args[1] === '-m') return mutate.buildInfo ?? metadata(key, args[2], f.source);
    if (args[0] === 'tool' && args[1] === 'buildid') {
      const receipt = existsSync(`${args[2]}.source-receipt.json`) ? json(`${args[2]}.source-receipt.json`).sourceReceipt : f.receipt;
      return platformSourceIdentity(receipt, f.source, f.manifest.desktopSourceBuild, key).buildID;
    }
    if (args[0] === 'build') {
      if (f.source.sourceOwnedReplacements) {
        assert.equal(existsSync(join(options.cwd, '.polaris-source-receipt.json')), false);
        assert.equal(options.env.GOENV, 'off'); assert.equal(options.env.GOWORK, 'off');
        assert.equal(options.env.GOTOOLCHAIN, 'local'); assert.equal(options.env.GOFLAGS, '');
        assert.ok(args.includes('-buildvcs=true'));
      }
      if (mutate.buildFailure) throw Error('synthetic compile failure');
      if (mutate.duringBuild) mutate.duringBuild(options.cwd);
      write(args[args.indexOf('-o') + 1], `synthetic ${key}`); return ''; }
    if (args[0] === 'test') return ''; // Unit-test command wiring only; no Windows test executable runs.
    throw new Error(`Unexpected command ${command} ${args}`);
  };
  return { run, calls };
}
function refresh(f) {
  const raw = JSON.stringify(f.source);
  write(join(f.root, 'scripts/desktop-core/source-manifest.json'), raw);
  f.manifest.desktopSourceBuild.sourceManifestSha256 = digest(raw);
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
      ...['https://github.com/SagerNet/sing-box', 'https://github.com/polaris-arch/sing-box',
        forkURL + '.git', forkURL + '/', 'https://github.com/other/polaris-box',
        'https://github.com/polaris-arch/polaris-box-other'].map((url) =>
        [`URL ${url}`, (s) => { s.sourceURL = url; }]),
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
      const source = structuredClone(f.source), spec = structuredClone(f.manifest.desktopSourceBuild);
      mutate(source, spec);
      const attempt = { ...f, source, manifest: { ...f.manifest, desktopSourceBuild: spec } };
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
    assert.equal(git(f.repository, 'tag', '--list', f.source.upstreamTag), '', 'fork carries no upstream tag');
    assert.equal(git(f.upstreamRepository, 'tag', '--list', f.source.sourceTag), '', 'upstream carries no fork tag');
    assert.notEqual(f.source.sourceCommit, f.source.upstreamCommit);
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      const r = runner(f, key);
      const receipt = produceDesktopCore(f.root, f.manifest, key, join(bundle, key, coreFilename(key)), candidate, r.run);
      assert.deepEqual(receipt.overlays, []);
      assert.equal(receipt.sourceReceipt.sourceURL, forkURL);
      const fetches = r.calls.filter(({ command, args }) => command === 'git' && args[2] === 'fetch');
      assert.deepEqual(fetches.map(({ args }) => args.slice(3)), [
        ['--depth=1', forkURL + '.git', `${f.source.sourceCommit}:refs/heads/polaris-source`],
        ['--depth=1', '--no-tags', forkURL + '.git', `refs/tags/${f.source.sourceTag}:refs/tags/${f.source.sourceTag}`],
        ['--depth=1', '--no-tags', upstreamURL, `refs/tags/${f.source.upstreamTag}:refs/tags/${f.source.upstreamTag}`],
      ]);
      const build = r.calls.find(({ args }) => args[0] === 'build');
      assert.deepEqual(build.args[build.args.indexOf('-tags') + 1].split(','), expectedTags(key));
      assert.deepEqual(receipt.tags, expectedTags(key));
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
    assert.throws(() => validateSourceReceipt(bad, f.source, { ...f.manifest.desktopSourceBuild, sourceReceiptFingerprint: bad.fingerprint }), /commit tree/);
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
    delete f.manifest.desktopSourceBuild.sourceManifestSha256;
    assert.throws(() => desktopSourceGoVersion(f.root, f.manifest), /not frozen/);
  } finally { rmSync(f.root, { recursive: true, force: true }); }
});

test('actual five workflow Go pin commands reject wrong source bytes, mobile role and missing pin', () => {
  const commands = ['desktop-core', 'package', 'release-risk'].flatMap((name) =>
    readFileSync(join(repo, '.github/workflows', `${name}.yml`), 'utf8').split('\n')
      .filter((line) => line.includes('run: node') && line.includes('version='))
      .map((line) => line.trim().slice('run: '.length)));
  assert.equal(commands.length, 5);
  const f = fixture();
  try {
    copyFileSync(join(repo, 'scripts/desktop-core/source-graph.mjs'), join(f.root, 'scripts/desktop-core/source-graph.mjs'));
    write(join(f.root, 'scripts/libbox-patches/source-manifest.json'), JSON.stringify({ goVersion: '1.26.0', role: 'mobile' }));
    const selected = join(f.root, 'scripts/desktop-core/source-manifest.json');
    const output = join(f.root, 'github-output');
    const runCommands = (accepted, message) => {
      write(join(f.root, 'src-tauri/core-manifest.json'), JSON.stringify(f.manifest));
      for (const command of commands) {
        write(output, '');
        const result = spawnSync('bash', ['-c', command], {
          cwd: f.root, encoding: 'utf8', env: { ...process.env, GITHUB_OUTPUT: output },
        });
        if (accepted) {
          assert.equal(result.status, 0, result.stderr);
          assert.equal(readFileSync(output, 'utf8'), 'version=1.25.5\n');
        } else {
          assert.notEqual(result.status, 0);
          assert.match(result.stderr, message);
          assert.equal(readFileSync(output, 'utf8'), '', 'rejected selection must not emit a usable Go version');
        }
      }
    };
    runCommands(true);
    write(selected, JSON.stringify({ ...f.source, goVersion: '1.26.0' }));
    runCommands(false, /SHA-256/);
    f.source.role = 'mobile'; refresh(f);
    runCommands(false, /role/);
    f.source.role = 'desktop'; refresh(f);
    delete f.manifest.desktopSourceBuild.sourceManifestSha256;
    runCommands(false, /not frozen/);
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


test('present desktop block is fail-closed at every producer before any command', () => {
  const f = fixture();
  try {
    const mobile = canonical(f.manifest.sourceBuild);
    for (const invalid of [null, [], 'bad', {}, { ...f.manifest.desktopSourceBuild, sourceMode: undefined },
      { ...f.manifest.desktopSourceBuild, upstreamVersion: undefined }, f.manifest.sourceBuild,
      { ...f.manifest.desktopSourceBuild, mainGoModSha256: null }]) {
      const manifest = { ...f.manifest, desktopSourceBuild: invalid };
      for (const key of Object.keys(DESKTOP_TARGETS)) {
        const r = runner(f, key);
        assert.throws(() => produceDesktopCore(f.root, manifest, key, join(f.root, 'reject', key), candidate, r.run));
        assert.equal(r.calls.length, 0, key);
        assert.equal(existsSync(join(f.root, 'reject', key)), false);
      }
    }
    assert.equal(canonical(f.manifest.sourceBuild), mobile);
    for (const key of Object.keys(DESKTOP_TARGETS)) assert.equal(validateSourcePins(f.manifest, key, false), f.manifest.desktopSourceBuild);
    const legacy = { ...f.manifest }; delete legacy.desktopSourceBuild;
    assert.equal(validateSourcePins(legacy, 'linux', false), f.manifest.sourceBuild);
    const swap = { ...legacy, sourceBuild: f.manifest.desktopSourceBuild };
    assert.throws(() => validateSourcePins(swap, 'linux', false), /Legacy/);
  } finally { rmSync(f.root, { recursive: true, force: true }); }
});

test('packaging CLI rejects a present invalid desktop block in all four modes', () => {
  const f = fixture();
  try {
    cpSync(join(repo, 'scripts'), join(f.root, 'scripts'), { recursive: true });
    const path = join(f.root, 'src-tauri/core-manifest.json');
    for (const invalid of [null, [], {}, f.manifest.sourceBuild]) {
      write(path, JSON.stringify({ ...f.manifest, desktopSourceBuild: invalid }));
      for (const mode of ['confs', 'payload', 'assets', 'inventory']) {
        const result = spawnSync(process.execPath, [join(f.root, 'scripts/verify-packaging.mjs'), mode], { encoding: 'utf8', cwd: f.root });
        assert.equal(result.status, 1, result.stderr);
        assert.match(result.stderr, /Desktop source admission failed/);
        assert.doesNotMatch(result.stdout, /全部不变量成立/);
      }
    }
    // Equal-sized staging bytes cannot replace a mandatory D source receipt.
    write(path, JSON.stringify(f.manifest));
    write(join(f.root, 'resources/win/sing-box.exe'), 'synthetic equal-size core');
    write(join(f.root, 'staging/_up_/resources/win/sing-box.exe'), 'synthetic equal-size core');
    const result = spawnSync(process.execPath, [join(f.root, 'scripts/verify-packaging.mjs'),
      'payload', '--label', 'windows', '--root', 'staging'], { encoding: 'utf8', cwd: f.root });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /frozen source artifact\/receipt mismatch:.*source-receipts.*win\.json/);
    assert.doesNotMatch(result.stdout, /全部不变量成立/);
  } finally { rmSync(f.root, { recursive: true, force: true }); }
});

test('fork NOTICE binds desktop, Android and Apple corresponding sources inside their own sections', () => {
  const f = fixture();
  try {
    const spec = f.manifest.desktopSourceBuild;
    const mobile = { android: { sourceCommit: 'a'.repeat(40), manifestSha256: 'b'.repeat(64) },
      apple: { sourceCommit: 'c'.repeat(40), manifestSha256: 'd'.repeat(64) } };
    const desktop = `  - 桌面 sing-box v${spec.upstreamVersion} (GPLv3) — ${f.source.sourceURL}\n`
      + [`上游基线源码：https://github.com/SagerNet/sing-box/tree/${f.source.upstreamTag}`,
        `桌面源码修复构建：${spec.version}`, `固定源码：${f.source.sourceURL}/tree/${f.source.sourceCommit}`,
        `固定消费标签：${f.source.sourceURL}/tree/${f.source.sourceTag}`, `消费标签对象：${f.source.sourceTagObject}`,
        `共同 source manifest SHA-256：${spec.sourceManifestSha256}`].map((line) => `    ${line}\n`).join('');
    const section = (label, identity) => `  - ${label} v${f.manifest.bundledCoreVersion} (GPLv3) — https://github.com/SagerNet/sing-box\n`
      + [`上游基线源码：https://github.com/SagerNet/sing-box/tree/v${f.manifest.bundledCoreVersion}`,
        `固定源码：https://github.com/SagerNet/sing-box/tree/${identity.sourceCommit}`,
        `source manifest SHA-256：${identity.manifestSha256}`].map((line) => `    ${line}\n`).join('');
    const android = section('Android libbox 基线', mobile.android);
    const apple = section('Apple libbox 基线', mobile.apple);
    const patchBase = 'https://github.com/polaris-arch/Polaris/tree/4563b1fc1ecb23cb7f1141a37a2cab144891ecd6/scripts';
    const guidance = `════ 对应源码 / Corresponding Source ════\n`
      + `桌面 D 完整对应源码：${f.source.sourceURL}/tree/${f.source.sourceCommit}\n`
      + `桌面 D 源码资产：${f.source.sourceURL}/releases/tag/${f.source.sourceTag}\n`
      + `Android 旧补丁目录：${patchBase}/libbox-patches\n`
      + `Apple 旧补丁目录：${patchBase}/libbox-ios-patches\n`;
    const notice = desktop + android + apple + guidance;
    validateForkNotice(notice, f.manifest, f.source, mobile);
    const fixed = `    固定源码：${f.source.sourceURL}/tree/${f.source.sourceCommit}\n`;
    const invalid = [desktop.replace(fixed, '') + android + fixed + apple,
      desktop + android.replace(mobile.android.sourceCommit, mobile.apple.sourceCommit) + apple,
      desktop + android + apple.replace(mobile.apple.manifestSha256, mobile.android.manifestSha256),
      notice + apple, notice.replace(spec.version, f.manifest.sourceBuild.version),
      notice.replace(spec.sourceManifestSha256, mobile.android.manifestSha256),
      desktop + `    固定源码：https://github.com/SagerNet/sing-box/tree/${mobile.android.sourceCommit}\n` + android + apple];
    for (const text of invalid) assert.throws(() => validateForkNotice(text, f.manifest, f.source, mobile), /NOTICE/);
    for (const oldClaim of ['Windows 另加固定 Windows 附加补丁目录中的补丁。',
      'Windows additionally uses the fixed Windows overlay directory.',
      'Verify patches in the fixed Windows overlay directory.']) {
      assert.throws(() => validateForkNotice(notice + oldClaim, f.manifest, f.source, mobile), /retired Windows overlay/);
    }
    for (const bad of [notice.replace(`${patchBase}/libbox-ios-patches`, `${patchBase}/libbox-patches`),
      notice.replace(`桌面 D 完整对应源码：${f.source.sourceURL}/tree/${f.source.sourceCommit}`, `桌面 D 完整对应源码：${patchBase}/libbox-patches`),
      desktop + android + apple]) assert.throws(() => validateForkNotice(bad, f.manifest, f.source, mobile), /NOTICE/);
  } finally { rmSync(f.root, { recursive: true, force: true }); }
});

// New shapes below are synthetic fixtures, not a final source/tag or native proof.
import { desktopProducerPlan, writeValidationBundle, validateBundleOrigin, validateCurrentProducer } from './validation-origin.mjs';
const ownedEnv = { GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: 'polaris-arch/Polaris', GITHUB_REPOSITORY_ID: '1347028220',
  GITHUB_SHA: candidate, GITHUB_REF: 'refs/heads/collab/fk02-native-payload-validation-20261010',
  GITHUB_WORKFLOW_REF: 'polaris-arch/Polaris/.github/workflows/release-risk.yml@refs/heads/collab/fk02-native-payload-validation-20261010',
  GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_RUN_ID: '12345', GITHUB_RUN_ATTEMPT: '2', POLARIS_NATIVE_PAYLOAD_VALIDATION: '1' };
test('source-owned producer context accepts exact existing Risk and Package event/ref combinations', () => {
  const cases = [
    ['release-risk', 'workflow_dispatch', ownedEnv.GITHUB_REF],
    ['release-risk', 'push', 'refs/heads/main'],
    ['release-risk', 'pull_request', 'refs/pull/27/merge'],
    ['release-risk', 'merge_group', `refs/heads/gh-readonly-queue/main/pr-27-${candidate}`],
    ['package', 'workflow_dispatch', ownedEnv.GITHUB_REF],
    ['package', 'workflow_dispatch', 'refs/tags/v1.0.0'],
    ['package', 'push', 'refs/tags/v1.0.0'],
  ];
  for (const [caller, event, ref] of cases) {
    const env = { ...ownedEnv, GITHUB_EVENT_NAME: event, GITHUB_REF: ref,
      GITHUB_SHA: event === 'pull_request' ? 'd'.repeat(40) : candidate,
      GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/${caller}.yml@${ref}` };
    const context = producerContext(candidate, env);
    assert.deepEqual(context, { repository: env.GITHUB_REPOSITORY, repositoryId: env.GITHUB_REPOSITORY_ID,
      candidate, runId: env.GITHUB_RUN_ID, runAttempt: env.GITHUB_RUN_ATTEMPT,
      workflowRef: env.GITHUB_WORKFLOW_REF, workflowHead: env.GITHUB_SHA, ref, event });
    const facts = { schema: 'polaris-desktop-core-v1', candidate, producerContext: context };
    const receipt = { ...facts, fingerprint: digest(canonical(facts)) };
    assert.deepEqual(validateCurrentProducer(receipt, candidate, env), context);
    for (const field of ['candidate', 'runId', 'runAttempt', 'workflowRef', 'workflowHead', 'ref', 'event', 'repository', 'repositoryId']) {
      const stale = refingerprint({ ...receipt, producerContext: { ...context, [field]: 'stale' } });
      assert.throws(() => validateCurrentProducer(stale, candidate, env), /producer context/, `${caller}/${event}/${field}`);
    }
  }
});

test('source-owned producer context rejects wrong caller/event/ref without widening publication admission', () => {
  const cases = [
    ['release-risk', 'push', ownedEnv.GITHUB_REF],
    ['release-risk', 'push', 'refs/tags/v1.0.0'],
    ['release-risk', 'workflow_dispatch', 'refs/tags/v1.0.0'],
    ['release-risk', 'pull_request', 'refs/heads/main'],
    ['release-risk', 'pull_request', 'refs/pull/27/head'],
    ['release-risk', 'merge_group', 'refs/heads/main'],
    ['release-risk', 'merge_group', 'refs/heads/gh-readonly-queue/other/pr-27'],
    ['package', 'push', 'refs/heads/main'],
    ['package', 'push', 'refs/tags/other-1.0.0'],
    ['package', 'workflow_dispatch', 'refs/pull/27/merge'],
    ['package', 'workflow_dispatch', 'refs/tags/other-1.0.0'],
    ['package', 'pull_request', 'refs/pull/27/merge'],
    ['package', 'merge_group', 'refs/heads/gh-readonly-queue/main/pr-27'],
    ['desktop-core', 'workflow_dispatch', ownedEnv.GITHUB_REF],
    ['ci', 'push', 'refs/heads/main'],
    ['release-risk', 'workflow_run', 'refs/heads/main'],
    ['release-risk', 'workflow_dispatch', 'refs/heads/'],
    ['release-risk', 'merge_group', 'refs/heads/gh-readonly-queue/main/'],
  ];
  for (const [caller, event, ref] of cases) {
    assert.throws(() => producerContext(candidate, { ...ownedEnv, GITHUB_EVENT_NAME: event, GITHUB_REF: ref,
      GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/${caller}.yml@${ref}` }), /producer workflow context/, `${caller}/${event}/${ref}`);
  }
  for (const change of [{ GITHUB_WORKFLOW_REF: ownedEnv.GITHUB_WORKFLOW_REF.replace(ownedEnv.GITHUB_REF, 'refs/heads/main') },
    { GITHUB_REPOSITORY: 'other/Polaris' }, { GITHUB_REPOSITORY_ID: '1' }, { GITHUB_ACTIONS: 'false' },
    { GITHUB_SHA: 'not-a-commit' }, { GITHUB_RUN_ID: '0' }, { GITHUB_RUN_ATTEMPT: '0' }]) {
    assert.throws(() => producerContext(candidate, { ...ownedEnv, ...change }), /producer workflow context/);
  }
  assert.throws(() => producerContext('not-a-candidate', ownedEnv), /producer workflow context/);
});

function withOwnedEnv(operation) {
  const saved = Object.fromEntries([...Object.keys(ownedEnv), 'POLARIS_NO_KERNEL_RUN'].map((key) => [key, process.env[key]]));
  Object.assign(process.env, ownedEnv); delete process.env.POLARIS_NO_KERNEL_RUN;
  try { return operation(); } finally {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
  }
}
function freezeFixture(f) { refresh(f); write(join(f.root, 'src-tauri/core-manifest.json'), JSON.stringify(f.manifest)); }
const refingerprint = (receipt) => { const { fingerprint, ...facts } = receipt; return { ...facts, fingerprint: digest(canonical(facts)) }; };

test('source-owned declaration is explicit, singular and mirrored; historical identity is unchanged', () => {
  const f = fixture(true), legacy = fixture();
  try {
    const spec = f.manifest.desktopSourceBuild;
    validateSourceManifest(f.source, spec); validateSourcePins(f.manifest, 'linux', false);
    assert.throws(() => validateSourcePins(f.manifest, 'linux'), /four output/);
    assert.throws(() => sourceOwnedReplacements({ ...spec, sourceOwnedReplacements: [] }), /replacement required/);
    assert.throws(() => sourceOwnedReplacements({ ...spec, sourceOwnedReplacements: null }), /replacement required/);
    assert.throws(() => sourceOwnedReplacements({ ...spec, sourceOwnedReplacements: [...spec.sourceOwnedReplacements, ...spec.sourceOwnedReplacements] }));
    assert.throws(() => sourceOwnedReplacements({ ...spec, sourceMode: undefined }));
    for (const [field, bad] of Object.entries({ module: 'github.com/sagernet/sing-tun-extra', version: 'HEAD',
      replacementPath: '../sing-tun', replacementTree: null, replacementFileCount: 255, baselineFiles: 252,
      bindingPath: '/tmp/binding', patchSha256: null, other252BaselineFilesByteExact: false, baselineModuleSum: 'h1:bad' })) {
      const modified = structuredClone(spec); modified.sourceOwnedReplacements[0][field] = bad;
      assert.throws(() => sourceOwnedReplacements(modified), undefined, field);
    }
    assert.throws(() => validateSourceManifest({ ...f.source, sourceOwnedReplacements: [] }, spec), /manifest\/spec/);
    const ls = legacy.manifest.desktopSourceBuild;
    assert.equal(sourceOwnedReplacements(ls), undefined);
    const identity = platformSourceIdentity(legacy.receipt, legacy.source, ls, 'linux');
    const facts = { sourceReceiptFingerprint: legacy.receipt.fingerprint, buildTree: ls.platforms.linux.buildTree, overlays: [],
      version: ls.version, goVersion: legacy.source.goVersion, platform: 'linux', transportPins: ls.transportPins,
      patchedModules: ls.platforms.linux.patchedModules, transportModules: ls.platforms.linux.transportModules,
      goos: 'linux', goarch: 'amd64', cgo: '0', tags: expectedTags('linux') };
    assert.equal(identity.platformInputFingerprint, digest(canonical(facts)));
    assert.equal(identity.buildID, `polaris-desktop-source-v1-${digest(canonical(facts))}`);
    assert.throws(() => validateSourceManifest({ ...legacy.source, sourceOwnedReplacements: null }, ls));
    const plan = desktopProducerPlan(f.root, true);
    assert.equal(plan.legacyOrigins, false); assert.equal(plan.matrix.length, 4);
    assert.deepEqual(plan.matrix.map((row) => row.platform), ['linux', 'win', 'mac-x64', 'mac-arm64']);
    assert.equal(desktopProducerPlan(legacy.root, true).matrix.length, 1);
    assert.equal(desktopProducerPlan(legacy.root, false).matrix.length, 4);
    f.source.sourceTagObject = null; freezeFixture(f);
    assert.throws(() => desktopProducerPlan(f.root, true), /tag\/upstream/);
  } finally { rmSync(f.root, { recursive: true, force: true }); rmSync(legacy.root, { recursive: true, force: true }); }
});

test('source-owned native metadata binds one actual root/replacement and clean exact VCS', () => {
  const f = fixture(true);
  try {
    const spec = f.manifest.desktopSourceBuild, raw = metadata('linux', 'transport-path', f.source);
    validateBuildInfo(raw, f.source, 'linux', spec);
    const bad = [raw.replace('github.com/sagernet/sing-box\tv0.0.0-fixture', 'example.com/root\tv0.0.0-fixture'),
      raw.replace('\tdep\texample.com/transport', '\tmod\texample.com/transport'),
      raw.replace('\tdep\tgithub.com/sagernet/sing-tun', '\tmod\tgithub.com/sagernet/sing-tun'),
      raw.replace('./third_party/sing-tun', '/absolute/sing-tun'), raw.replace('(devel)', 'v1.0.0'),
      raw.replace('v0.9.7-0.20261009022811-5c2edb183cc9', 'v0.0.0'),
      raw.replace(f.source.sourceCommit, 'd'.repeat(40)), raw.replace('vcs.modified=false', 'vcs.modified=true'),
      raw.replace('with_gvisor,', ''), raw.replace('vcs.modified=false\n', ''),
      raw + '\tdep\texample.com/extra\tv1.0.0\n\t=>\t./arbitrary\t(devel)\n',
      raw.replace('\tmod\tgithub.com/sagernet/sing-box\tv0.0.0-fixture\n',
        '\tmod\tgithub.com/sagernet/sing-box\tv0.0.0-fixture\n\t=>\t./root\t(devel)\n')];
    for (const value of bad) assert.throws(() => validateBuildInfo(value, f.source, 'linux', spec));
    assert.equal(buildInfoFingerprint(raw), buildInfoFingerprint(raw.replaceAll('\n', '\r\n').replace('transport-path', 'another-path')));
    const capture = (command, args, options) => execFileSync(command, args, { ...options, encoding: 'utf8', stdio: 'pipe' }).trim();
    verifySourceOwnedCheckout(f.repository, f.source, spec, capture);
    const file = join(f.repository, 'third_party/sing-tun/file-0.go'), original = readFileSync(file);
    writeFileSync(file, 'changed bytes'); assert.throws(() => verifySourceOwnedCheckout(f.repository, f.source, spec, capture), /bytes differ/);
    writeFileSync(file, original);
    git(f.repository, 'update-index', '--chmod=+x', 'third_party/sing-tun/file-0.go');
    assert.throws(() => verifySourceOwnedCheckout(f.repository, f.source, spec, capture), /index mode/);
    git(f.repository, 'update-index', '--chmod=-x', 'third_party/sing-tun/file-0.go');
    const mismatched = structuredClone(spec); mismatched.sourceOwnedReplacements[0].bindingSha256 = 'f'.repeat(64);
    const source = { ...f.source, sourceOwnedReplacements: mismatched.sourceOwnedReplacements };
    assert.throws(() => verifySourceOwnedCheckout(f.repository, source, mismatched, capture), /SHA-256/);
  } finally { rmSync(f.root, { recursive: true, force: true }); }
});

test('metadata outside checkout restores exact bytes on success and compile failure', () => {
  const root = mkdtempSync(join(tmpdir(), 'polaris-held-metadata-'));
  try {
    const checkout = join(root, 'checkout'); mkdirSync(checkout);
    const path = join(checkout, '.polaris-source-receipt.json'); const bytes = Buffer.from('exact original receipt\n'); writeFileSync(path, bytes);
    for (const fail of [false, true]) {
      const action = () => { assert.equal(existsSync(path), false); if (fail) throw Error('compiler failed'); return 'done'; };
      if (fail) assert.throws(() => withProviderMetadataOutside(checkout, root, action), /compiler failed/);
      else assert.equal(withProviderMetadataOutside(checkout, root, action), 'done');
      assert.deepEqual(readFileSync(path), bytes);
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('merge queue and tag callers reach source-owned four-producer and ordinary consumer contracts with a fake compiler', () => withOwnedEnv(() => {
  const f = fixture(true);
  try {
    process.env.POLARIS_NATIVE_PAYLOAD_VALIDATION = '0';
    for (const [caller, event, ref] of [
      ['release-risk', 'merge_group', `refs/heads/gh-readonly-queue/main/pr-27-${candidate}`],
      ['package', 'push', 'refs/tags/v1.0.0'],
      ['package', 'workflow_dispatch', 'refs/tags/v1.0.0'],
    ]) {
      Object.assign(process.env, { GITHUB_EVENT_NAME: event, GITHUB_REF: ref,
        GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/${caller}.yml@${ref}` });
      const dir = join(f.root, `ordinary-${caller}-${event}`);
      for (const key of Object.keys(DESKTOP_TARGETS)) {
        const r = runner(f, key);
        const receipt = produceDesktopCore(f.root, f.manifest, key, join(dir, key, coreFilename(key)), candidate, r.run);
        assert.deepEqual(receipt.producerContext, producerContext(candidate));
        assert.equal(receipt.fingerprint, refingerprint(receipt).fingerprint);
      }
      writeBundleInventory(dir, candidate);
      const inventory = consumeDesktopBundle(f.root, f.manifest, dir, candidate, ['linux'], (command, args, options) =>
        command === 'git' ? (args[0] === 'rev-parse' ? candidate : '')
          : runner(f, args[2].split(/[/\\]/).at(-2)).run(command, args, options));
      assert.equal(inventory.schema, 'polaris-desktop-bundle-v1');
      assert.deepEqual(Object.keys(inventory.platforms).sort(), Object.keys(DESKTOP_TARGETS).sort());
    }
  } finally { rmSync(f.root, { recursive: true, force: true }); }
}));

test('all-current v2 producer receipt binds actual workflow context before fingerprint; old attempt cannot be relabeled', () => withOwnedEnv(() => {
  const f = fixture(true);
  try {
    const dir = join(f.root, 'bundle');
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      const r = runner(f, key), receipt = produceDesktopCore(f.root, f.manifest, key, join(dir, key, coreFilename(key)), candidate, r.run);
      assert.deepEqual(receipt.producerContext, producerContext(candidate));
      assert.deepEqual(receipt.sourceOwnedReplacements, f.source.sourceOwnedReplacements);
      assert.equal(receipt.fingerprint, refingerprint(receipt).fingerprint);
      assert.ok(r.calls.find((call) => call.args[0] === 'build').args.includes('-buildvcs=true'));
    }
    const inv = writeValidationBundle(f.root, dir, candidate);
    assert.equal(inv.schema, 'polaris-desktop-validation-bundle-v2');
    assert.equal(inv.mode, 'source-owned-all-current-v1'); assert.equal(inv.originTransport, undefined);
    const file = join(dir, 'linux', `${coreFilename('linux')}.source-receipt.json`), original = readFileSync(file), rec = JSON.parse(original);
    for (const [field, bad] of Object.entries({ runId: '12344', runAttempt: '1', repository: 'other/Polaris', repositoryId: '1',
      candidate: 'd'.repeat(40), workflowHead: 'd'.repeat(40), workflowRef: 'other/workflow', event: 'push', ref: 'refs/heads/main' })) {
      const stale = refingerprint({ ...rec, producerContext: { ...rec.producerContext, [field]: bad } });
      writeFileSync(file, JSON.stringify(stale));
      assert.throws(() => writeValidationBundle(f.root, dir, candidate), /producer context/, field);
      // An assembler's current-native label cannot repair the original stale receipt.
      assert.throws(() => validateBundleOrigin(f.root, inv, candidate, 'linux', stale), /producer context/, field);
    }
    writeFileSync(file, original);
    assert.throws(() => validateBundleOrigin(f.root, { ...inv, mode: undefined }, candidate, 'linux', rec), /historical origins/);
    const missing = structuredClone(inv); delete missing.origins['mac-arm64'];
    assert.throws(() => validateBundleOrigin(f.root, missing, candidate, 'linux', rec), /envelope/);
    const wrongOrigin = structuredClone(inv); wrongOrigin.origins.linux.runAttempt = '1';
    assert.throws(() => validateBundleOrigin(f.root, wrongOrigin, candidate, 'linux', rec), /actual producer/);
    const info = consumeDesktopBundle(f.root, f.manifest, dir, candidate, ['linux'], (command, args, options) =>
      command === 'git' ? (args[0] === 'rev-parse' ? candidate : '') : runner(f, args[2].split(/[/\\]/).at(-2)).run(command, args, options));
    assert.equal(info.mode, inv.mode);
    rmSync(join(dir, 'mac-arm64', coreFilename('mac-arm64')));
    assert.throws(() => writeValidationBundle(f.root, dir, candidate));
  } finally { rmSync(f.root, { recursive: true, force: true }); }
}));

const actualTechnicalMetadata = [{"key":"mac-arm64","raw":"/Users/runner/work/_temp/polaris-dns-native/polaris-box-1.15.0-alpha.11.polaris.candidate-aafc521b-darwin-arm64/sing-box: go1.25.5\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n\tmod\tgithub.com/sagernet/sing-box\tv0.0.0-20261010200656-aafc521b745e\t\n\tdep\tfilippo.io/edwards25519\tv1.2.0\th1:crnVqOiS4jqYleHd9vaKZ+HKtHfllngJIiOpNpoJsjo=\n\tdep\tgithub.com/RyuaNerin/go-krypto\tv1.3.0\th1:smavTzSMAx8iuVlGb4pEwl9MD2qicqMzuXR2QWp2/Pg=\n\tdep\tgithub.com/ajg/form\tv1.5.1\th1:t9c7v8JUKu/XxOGBU0yjNpaMloxGEJhUkqFRq0ibGeU=\n\tdep\tgithub.com/anchore/go-lzo\tv0.1.0\th1:NgAacnzqPeGH49Ky19QKLBZEuFRqtTG9cdaucc3Vncs=\n\tdep\tgithub.com/andybalholm/brotli\tv1.1.0\th1:eLKJA0d02Lf0mVpIDgYnqXcUn0GqVmEFny3VuID1U3M=\n\tdep\tgithub.com/anmitsu/go-shlex\tv0.0.0-20200514113438-38f4b401e2be\th1:9AeTilPcZAjCFIImctFaOjnTIavg87rW78vTPkQqLI8=\n\tdep\tgithub.com/anthropics/anthropic-sdk-go\tv1.26.0\th1:oUTzFaUpAevfuELAP1sjL6CQJ9HHAfT7CoSYSac11PY=\n\tdep\tgithub.com/axiomhq/hyperloglog\tv0.0.0-20240319100328-84253e514e02\th1:bXAPYSbdYbS5VTy92NIUbeDI1qyggi+JYh5op9IFlcQ=\n\tdep\tgithub.com/caddyserver/certmagic\tv0.25.3-0.20260421143802-60d9d8b415d6\th1:LYSB6VgWzKtNrcxElw3c97BP40Oc7bizKxA9K1Vi/5k=\n\tdep\tgithub.com/caddyserver/zerossl\tv0.1.5\th1:dkvOjBAEEtY6LIGAHei7sw2UgqSD6TrWweXpV7lvEvE=\n\tdep\tgithub.com/clipperhouse/uax29/v2\tv2.2.0\th1:ChwIKnQN3kcZteTXMgb1wztSgaU+ZemkgWdohwgs8tY=\n\tdep\tgithub.com/coder/websocket\tv1.8.14\th1:9L0p0iKiNOibykf283eHkKUHHrpG7f65OE3BhhO7v9g=\n\tdep\tgithub.com/coreos/go-oidc/v3\tv3.17.0\th1:hWBGaQfbi0iVviX4ibC7bk8OKT5qNr4klBaCHVNvehc=\n\tdep\tgithub.com/creack/pty\tv1.1.24\th1:bJrF4RRfyJnbTJqzRLHzcGaZK1NeM5kTC9jGgovnR1s=\n\tdep\tgithub.com/cretz/bine\tv0.2.0\th1:8GiDRGlTgz+o8H9DSnsl+5MeBK4HsExxgl6WgzOCuZo=\n\tdep\tgithub.com/database64128/netx-go\tv0.1.1\th1:dT5LG7Gs7zFZBthFBbzWE6K8wAHjSNAaK7wCYZT7NzM=\n\tdep\tgithub.com/database64128/tfo-go/v2\tv2.3.2\th1:UhZMKiMq3swZGUiETkLBDzQnZBPSAeBMClpJGlnJ5Fw=\n\tdep\tgithub.com/dgryski/go-camellia\tv0.0.0-20191119043421-69a8a13fb23d\th1:CPqTNIigGweVPT4CYb+OO2E6XyRKFOmvTHwWRLgCAlE=\n\tdep\tgithub.com/dgryski/go-metro\tv0.0.0-20180109044635-280f6062b5bc\th1:8WFBn63wegobsYAX0YjD+8suexZDga5CctH4CCTx2+8=\n\tdep\tgithub.com/fsnotify/fsnotify\tv1.9.0\th1:2Ml+OJNzbYCTzsxtv8vKSFD9PbJjmhYF14k/jKC7S9k=\n\tdep\tgithub.com/fxamacker/cbor/v2\tv2.9.0\th1:NpKPmjDBgUfBms6tr6JZkTHtfFGcMKsw3eGcmD/sapM=\n\tdep\tgithub.com/gaissmai/bart\tv0.26.1\th1:+w4rnLGNlA2GDVn382Tfe3jOsK5vOr5n4KmigJ9lbTo=\n\tdep\tgithub.com/go-chi/chi/v5\tv5.2.5\th1:Eg4myHZBjyvJmAFjFvWgrqDTXFyOzjj7YIm3L3mu6Ug=\n\tdep\tgithub.com/go-chi/render\tv1.0.3\th1:AsXqd2a1/INaIfUSKq3G5uA8weYx20FOsM7uSoCyyt4=\n\tdep\tgithub.com/go-jose/go-jose/v4\tv4.1.3\th1:CVLmWDhDVRa6Mi/IgCgaopNosCaHz7zrMeF9MlZRkrs=\n\tdep\tgithub.com/go4org/hashtriemap\tv0.0.0-20251130024219-545ba229f689\th1:0psnKZ+N2IP43/SZC8SKx6OpFJwLmQb9m9QyV9BC2f8=\n\tdep\tgithub.com/gobwas/httphead\tv0.1.0\th1:exrUm0f4YX0L7EBwZHuCF4GDp8aJfVeBrlLQrs6NqWU=\n\tdep\tgithub.com/gobwas/pool\tv0.2.1\th1:xfeeEhW7pwmX8nuLVlqbzVc7udMDrwetjEv+TZIz1og=\n\tdep\tgithub.com/gofrs/uuid/v5\tv5.5.1\th1:z1Ce19/JwNidXpy3tOQc3241lnJLKdKyq/xlNvlD4Ng=\n\tdep\tgithub.com/golang/groupcache\tv0.0.0-20241129210726-2c02b8208cf8\th1:f+oWsMOmNPc8JmEHVZIycC7hBoQxHH9pNKQORJNozsQ=\n\tdep\tgithub.com/google/btree\tv1.1.3\th1:CVpQJjYgC4VbzxeGVHfvZrv1ctoYCAI8vbl07Fcxlyg=\n\tdep\tgithub.com/google/certificate-transparency-go\tv1.3.2\th1:9ahSNZF2o7SYMaKaXhAumVEzXB2QaayzII9C8rv7v+A=\n\tdep\tgithub.com/google/gopacket\tv1.1.19\th1:ves8RnFZPGiFnTS0uPQStjwru6uO6h+nlr9j6fL7kF8=\n\tdep\tgithub.com/google/uuid\tv1.6.0\th1:NIvaJDMOsjHA8n1jAhLSgzrAzy1Hgr+hNrb57e+94F0=\n\tdep\tgithub.com/hashicorp/yamux\tv0.1.2\th1:XtB8kyFOyHXYVFnwT5C3+Bdo8gArse7j2AQ0DA0Uey8=\n\tdep\tgithub.com/hdevalence/ed25519consensus\tv0.2.0\th1:37ICyZqdyj0lAZ8P4D1d1id3HqbbG1N3iBb1Tb4rdcU=\n\tdep\tgithub.com/huin/goupnp\tv1.3.0\th1:UvLUlWDNpoUdYzb2TCn+MuTWtcjXKSza2n6CBdQ0xXc=\n\tdep\tgithub.com/insomniacslk/dhcp\tv0.0.0-20260220084031-5adc3eb26f91\th1:u9i04mGE3iliBh0EFuWaKsmcwrLacqGmq1G3XoaM7gY=\n\tdep\tgithub.com/jackpal/go-nat-pmp\tv1.0.2\th1:KzKSgb7qkJvOUTqYl9/Hg/me3pWgBmERKrTGD7BdWus=\n\tdep\tgithub.com/keybase/go-keychain\tv0.0.1\th1:way+bWYa6lDppZoZcgMbYsvC7GxljxrskdNInRtuthU=\n\tdep\tgithub.com/klauspost/compress\tv1.19.1\th1:VsB4HPswih7mmZ8WleSFQ75c/Ui1M4trX5oAsJnhSlk=\n\tdep\tgithub.com/klauspost/cpuid/v2\tv2.3.0\th1:S4CRMLnYUhGeDFDqkGriYKdfoFlDnMtqTiI/sFzhA9Y=\n\tdep\tgithub.com/koron/go-ssdp\tv0.0.4\th1:1IDwrghSKYM7yLf7XCzbByg2sJ/JcNOZRXS2jczTwz0=\n\tdep\tgithub.com/kr/fs\tv0.1.0\th1:Jskdu9ieNAYnjxsi0LbQp1ulIKZV1LAFgK1tWhpZgl8=\n\tdep\tgithub.com/libdns/acmedns\tv0.5.0\th1:5pRtmUj4Lb/QkNJSl1xgOGBUJTWW7RjpNaIhjpDXjPE=\n\tdep\tgithub.com/libdns/alidns\tv1.0.6\th1:/Ii428ty6WHFJmE24rZxq2taq++gh7rf9jhgLfp8PmM=\n\tdep\tgithub.com/libdns/cloudflare\tv0.2.2\th1:XWHv+C1dDcApqazlh08Q6pjytYLgR2a+Y3xrXFu0vsI=\n\tdep\tgithub.com/libdns/libdns\tv1.1.1\th1:wPrHrXILoSHKWJKGd0EiAVmiJbFShguILTg9leS/P/U=\n\tdep\tgithub.com/libp2p/go-nat\tv1.0.1-0.20250821073202-01afc089f138\th1:YohuNPT/1k3VcThCQlBZ43PCPWPfMRS1zcxWBF2SLK8=\n\tdep\tgithub.com/libp2p/go-netroute\tv0.2.1\th1:V8kVrpD8GK0Riv15/7VN6RbUQ3URNZVosw7H2v9tksU=\n\tdep\tgithub.com/logrusorgru/aurora\tv2.0.3+incompatible\th1:tOpm7WcpBTn4fjmVfgpQq0EfczGlG91VSDkswnjF5A8=\n\tdep\tgithub.com/mattn/go-runewidth\tv0.0.27\th1:Feg/Oou5zI/wnpgDF6omIU0OokC9GxLC/WRknhVlIR0=\n\tdep\tgithub.com/metacubex/utls\tv1.8.7\th1:Cp+yWkNTFkSihETgGWq34hlVFds5HpYWVOR1xovUVTs=\n\tdep\tgithub.com/mholt/acmez/v3\tv3.1.6\th1:eGVQNObP0pBN4sxqrXeg7MYqTOWyoiYpQqITVWlrevk=\n\tdep\tgithub.com/miekg/dns\tv1.1.72\th1:vhmr+TF2A3tuoGNkLDFK9zi36F2LS+hKTRW0Uf8kbzI=\n\tdep\tgithub.com/mitchellh/go-ps\tv1.0.0\th1:i6ampVEEF4wQFF+bkYfwYgY+F/uYJDktmvLPf7qIgjc=\n\tdep\tgithub.com/openai/openai-go/v3\tv3.26.0\th1:bRt6H/ozMNt/dDkN4gobnLqaEGrRGBzmbVs0xxJEnQE=\n\tdep\tgithub.com/oschwald/maxminddb-golang\tv1.13.1\th1:G3wwjdN9JmIK2o/ermkHM+98oX5fS+k5MbwsmL4MRQE=\n\tdep\tgithub.com/pierrec/lz4/v4\tv4.1.26\th1:GrpZw1gZttORinvzBdXPUXATeqlJjqUG/D87TKMnhjY=\n\tdep\tgithub.com/pion/dtls/v3\tv3.1.5\th1:9xJtVsHwMYeSjPp5Hh1FTis4DchnQWtnOa5o+6ygqfc=\n\tdep\tgithub.com/pion/logging\tv0.2.4\th1:tTew+7cmQ+Mc1pTBLKH2puKsOvhm32dROumOZ655zB8=\n\tdep\tgithub.com/pion/transport/v4\tv4.0.2\th1:ifYlPqNwsy6aKQ9y8yzxXlHae5431ZrH2avkD/Rn6Tk=\n\tdep\tgithub.com/pires/go-proxyproto\tv0.8.1\th1:9KEixbdJfhrbtjpz/ZwCdWDD2Xem0NZ38qMYaASJgp0=\n\tdep\tgithub.com/pkg/sftp\tv1.13.10\th1:+5FbKNTe5Z9aspU88DPIKJ9z2KZoaGCu6Sr6kKR/5mU=\n\tdep\tgithub.com/quic-go/qpack\tv0.6.0\th1:g7W+BMYynC1LbYLSqRt8PBg5Tgwxn214ZZR34VIOjz8=\n\tdep\tgithub.com/sagernet/bbolt\tv0.0.0-20260915102804-500ee1e84832\th1:KDXMGbzqLmDdKcld0T3wuMuqf1iezZq+DemgtZ/vhtQ=\n\tdep\tgithub.com/sagernet/cors\tv1.2.1\th1:Cv5Z8y9YSD6Gm+qSpNrL3LO4lD3eQVvbFYJSG7JCMHQ=\n\tdep\tgithub.com/sagernet/cronet-go\tv0.0.0-20260929213745-4d18a60dc3a8\th1:yW/KNhSxWxllsPjLx5EUPohHLa4Kvzl2Qt9kHkB0ZIc=\n\tdep\tgithub.com/sagernet/cronet-go/all\tv0.0.0-20260929213745-4d18a60dc3a8\th1:xhh9UrPFncqI8r2mr+bybgZyJICtV4GKOSJuhnEx75g=\n\tdep\tgithub.com/sagernet/cronet-go/lib/darwin_arm64\tv0.0.0-20260929213014-a1cafd93eb1f\th1:SpLjX7QCXhKInRzT0hiYCpt+kOmcCVsrJvUV9RRUy2c=\n\tdep\tgithub.com/sagernet/fswatch\tv0.1.2\th1:/TT7k4mkce1qFPxamLO842WjqBgbTBiXP2mlUjp9PFk=\n\tdep\tgithub.com/sagernet/gliderssh\tv0.3.4-0.20260531100337-2194faca5648\th1:IWVjKBARzVjdmH0VUaeTBOBli1qkwKmTG4XfbkpSS20=\n\tdep\tgithub.com/sagernet/gvisor\tv0.0.0-20260727.0-sing-box-mod.1\th1:IdQ7yTKkB2wv8txwshxUroPlO4npOYAV71xb7xQ7Lys=\n\tdep\tgithub.com/sagernet/quic-go\tv0.61.0-sing-box-mod.9\th1:Lojx77uC63TxZO38Z1OSo6aOoBSXY+ueH0/IKKeTZuI=\n\tdep\tgithub.com/sagernet/sing\tv0.9.7-0.20260929150544-6f21f2425a95\th1:taM2SOI5HK/Ubpi3FO0Bc7bKHsFVdGe4YS/jjXD4kAM=\n\tdep\tgithub.com/sagernet/sing-anytls\tv0.0.0-20260928104022-580984e4d8cb\th1:n4sF2zv0CR1gcb4awh61t11t0FZmcP40x5enB8HARjE=\n\tdep\tgithub.com/sagernet/sing-cloudflared\tv0.1.4-0.20261002084126-c1255ae368f2\th1:+AfbAXGEV8tezIYHeShzwHm7IMUE5kYGVFLo0ly7W24=\n\tdep\tgithub.com/sagernet/sing-mux\tv0.3.10-0.20260929204512-caf09fe32475\th1:aG0DqHEOjax7DyfE1GTAXZotOg3QGVCz3/8rCSua2kg=\n\tdep\tgithub.com/sagernet/sing-openconnect\tv0.1.1-0.20260929151226-29757281a247\th1:Ui4XtxNgQiEquK+ch4dYhthKsVlGQpdCdKIpOuJVyog=\n\tdep\tgithub.com/sagernet/sing-openvpn\tv0.1.1-0.20260929151220-f330676d6d4a\th1:Q16Twlwk9fssoCSUTDo5WqPg0EZHG9JSV9SC3zRy9M0=\n\tdep\tgithub.com/sagernet/sing-quic\tv0.7.2-0.20261002084117-75c3ac4fa12b\th1:2Jx/URJAPt32MEbLmAR5nzH87tO6NQZLRfRFvFqBWuk=\n\tdep\tgithub.com/sagernet/sing-shadowsocks\tv0.2.9-0.20260929204512-65740e0f0e3e\th1:LKgE/uuqp/D2kgxYViOz8vf4slaCUfpFywXjtuwD5gU=\n\tdep\tgithub.com/sagernet/sing-shadowsocks2\tv0.2.2-0.20260929152114-a69d1086332b\th1:VH65QNqSHat2b+N3Rlzo5Qe44gRmQGwzY5atuy3D0A8=\n\tdep\tgithub.com/sagernet/sing-shadowtls\tv0.2.2-0.20260928201441-a9c0127d5c99\th1:l/tdZjaoDp3iqEG4NVrFHtphy1+Qjt9bpWAgNbF8iGg=\n\tdep\tgithub.com/sagernet/sing-snell\tv0.0.0-20260904135315-bc5a12ac736f\th1:tbTCI+iis5sTmua7a6Q7vebcSRURok/LGgnEafnQAsc=\n\tdep\tgithub.com/sagernet/sing-tun\tv0.9.7-0.20261009022811-5c2edb183cc9\n\t=>\t./third_party/sing-tun\t(devel)\t\n\t\n\tdep\tgithub.com/sagernet/sing-usbip\tv0.0.0-20260817040617-28bd42667eca\th1:5wA+IE0Fq1CGVLOgSpm0gKKZ03HzugcJ2JyhmDqeX6A=\n\tdep\tgithub.com/sagernet/sing-vmess\tv0.2.9-0.20260929152519-9b95ab8c9478\th1:q2eQn4nq8oWGxRm9zXesXSlPr9GEgPrOGzf4iPtzG9A=\n\tdep\tgithub.com/sagernet/smux\tv1.5.50-sing-box-mod.1\th1:XkJcivBC9V4wBjiGXIXZ229aZCU1hzcbp6kSkkyQ478=\n\tdep\tgithub.com/sagernet/tailscale\tv1.102.1-sing-box-1.14-mod.5.0.20260925112514-35e61219dedd\th1:PzEFeBqDaSUdoWE77NptloLTd1sNT1JAiFThgGq5rlo=\n\tdep\tgithub.com/sagernet/wireguard-go\tv0.0.8-0.20260929150556-ca3bc60c4ce7\th1:MiffQUqKBKoAHz7nkHqh4EmjJsz2M8FHeUdPUdnNyvs=\n\tdep\tgithub.com/sagernet/ws\tv0.0.0-20231204124109-acfe8907c854\th1:6uUiZcDRnZSAegryaUGwPC/Fj13JSHwiTftrXhMmYOc=\n\tdep\tgithub.com/smallstep/pkcs7\tv0.1.1\th1:x+rPdt2W088V9Vkjho4KtoggyktZJlMduZAtRHm68LU=\n\tdep\tgithub.com/spf13/cobra\tv1.10.2\th1:DMTTonx5m65Ic0GOoRY2c16WCbHxOOw6xxezuLaBpcU=\n\tdep\tgithub.com/spf13/pflag\tv1.0.10\th1:4EBh2KAYBwaONj6b2Ye1GiHfwjqyROoF4RwYO+vPwFk=\n\tdep\tgithub.com/tailscale/certstore\tv0.1.1-0.20260409135935-3638fb84b77d\th1:JcGKBZAL7ePLwOhUdN8qGQZlP5GueEiIZwY7R62pejE=\n\tdep\tgithub.com/tailscale/hujson\tv0.0.0-20260302212456-ecc657c15afd\th1:Rf9uhF1+VJ7ZHqxrG8pJ6YacmHvVCmByDmGbAWCc/gA=\n\tdep\tgithub.com/tailscale/peercred\tv0.0.0-20250107143737-35a0c7bd7edc\th1:24heQPtnFR+yfntqhI3oAu9i27nEojcQ4NuBQOo5ZFA=\n\tdep\tgithub.com/tailscale/web-client-prebuilt\tv0.0.0-20250124233751-d4cd19a26976\th1:UBPHPtv8+nEAy2PD8RyAhOYvau1ek0HDJqLS/Pysi14=\n\tdep\tgithub.com/tidwall/gjson\tv1.18.0\th1:FIDeeyB800efLX89e5a8Y0BNH+LOngJyGrIWxG2FKQY=\n\tdep\tgithub.com/tidwall/match\tv1.1.1\th1:+Ho715JplO36QYgwN9PGYNhgZvoUSc9X2c80KVTi+GA=\n\tdep\tgithub.com/tidwall/pretty\tv1.2.1\th1:qjsOFOWWQl+N3RsoF5/ssm1pHmJJwhjlSbZ51I6wMl4=\n\tdep\tgithub.com/tidwall/sjson\tv1.2.5\th1:kLy8mja+1c9jlljvWTlSazM7cKDRfJuR/bOJhcY5NcY=\n\tdep\tgithub.com/tjfoc/gmsm\tv1.4.1\th1:aMe1GlZb+0bLjn+cKTPEvvn9oUEBlJitaZiiBwsbgho=\n\tdep\tgithub.com/u-root/uio\tv0.0.0-20240224005618-d2acac8f3701\th1:pyC9PaHYZFgEKFdlp3G8RaCKgVpHZnecvArXvPXcFkM=\n\tdep\tgithub.com/vishvananda/netns\tv0.0.5\th1:DfiHV+j8bA32MFM7bfEunvT8IAqQ/NzSJHtcmW5zdEY=\n\tdep\tgithub.com/x448/float16\tv0.8.4\th1:qLwI1I70+NjRFUR3zs1JPUCgaCXSh3SW62uAKT1mSBM=\n\tdep\tgithub.com/youmark/pkcs8\tv0.0.0-20240726163527-a2c0da244d78\th1:ilQV1hzziu+LLM3zUTJ0trRztfwgjqKnBWNtSRkbmwM=\n\tdep\tgithub.com/zeebo/blake3\tv0.2.4\th1:KYQPkhpRtcqh0ssGYcKLG1JYvddkEA8QwCM/yBqhaZI=\n\tdep\tgo.uber.org/multierr\tv1.11.0\th1:blXXJkSxSSfBVBlC76pxqeO+LN3aDfLQo+309xJstO0=\n\tdep\tgo.uber.org/zap\tv1.27.1\th1:08RqriUEv8+ArZRYSTXy1LeBScaMpVSTBhCeaZYfMYc=\n\tdep\tgo.uber.org/zap/exp\tv0.3.0\th1:6JYzdifzYkGmTdRR59oYH+Ng7k49H9qVpWwNSsGJj3U=\n\tdep\tgo4.org/mem\tv0.0.0-20240501181205-ae6ca9944745\th1:Tl++JLUCe4sxGu8cTpDzRLd3tN7US4hOxG5YpKCzkek=\n\tdep\tgo4.org/netipx\tv0.0.0-20231129151722-fdeea329fbba\th1:0b9z3AuHCjxk0x/opv64kcgZLBseWJUpBw5I82+2U4M=\n\tdep\tgolang.org/x/crypto\tv0.54.0\th1:YLIA59K4fiNzHzjnZt2tUJQjQtUWfWbeHBqKtk3eScw=\n\tdep\tgolang.org/x/exp\tv0.0.0-20260410095643-746e56fc9e2f\th1:W3F4c+6OLc6H2lb//N1q4WpJkhzJCK5J6kUi1NTVXfM=\n\tdep\tgolang.org/x/mod\tv0.37.0\th1:vF1DjpVEshcIqoEaauuHebaLk1O1forxjxBaVn884JQ=\n\tdep\tgolang.org/x/net\tv0.57.0\th1:K5+3DljvIuDG9/Jv9rvyMywYNFCQ9RSUY6OOTTkT+tE=\n\tdep\tgolang.org/x/oauth2\tv0.36.0\th1:peZ/1z27fi9hUOFCAZaHyrpWG5lwe0RJEEEeH0ThlIs=\n\tdep\tgolang.org/x/sync\tv0.22.0\th1:SZjpbeLmrCk4xhRSZFNZW5gFUeCeFgjekvI/+gfScek=\n\tdep\tgolang.org/x/sys\tv0.47.0\th1:o7XGOvZQCADBQQ4Y7VNq2dRWQR7JmOUW8Kxx4ZsNgWs=\n\tdep\tgolang.org/x/term\tv0.45.0\th1:NwWyBmoJCbfTHpxrWoZ9C6/VxOf7ic219I8xZZFdrf0=\n\tdep\tgolang.org/x/text\tv0.40.0\th1:Ub2Z6/xjgF1WrYQz2nuITOEegKFtiIy+rieRJ5lHZKs=\n\tdep\tgolang.org/x/time\tv0.15.0\th1:bbrp8t3bGUeFOx08pvsMYRTCVSMk89u4tKbNOZbp88U=\n\tdep\tgolang.zx2c4.com/wireguard/wgctrl\tv0.0.0-20241231184526-a9ab2273dd10\th1:3GDAcqdIg1ozBNLgPy4SLT84nfcBjr6rhGtXYtrkWLU=\n\tdep\tgoogle.golang.org/genproto/googleapis/rpc\tv0.0.0-20251202230838-ff82c1b0f217\th1:gRkg/vSppuSQoDjxyiGfN4Upv/h/DQmIR10ZU8dh4Ww=\n\tdep\tgoogle.golang.org/grpc\tv1.79.1\th1:zGhSi45ODB9/p3VAawt9a+O/MULLl9dpizzNNpq7flY=\n\tdep\tgoogle.golang.org/protobuf\tv1.36.11\th1:fV6ZwhNocDyBLK0dj+fg8ektcVegBBuEolpbTQyBNVE=\n\tdep\tlukechampine.com/blake3\tv1.3.0\th1:sJ3XhFINmHSrYCgl958hscfIa3bw8x4DqMP3u1YvoYE=\n\tdep\tzombiezen.com/go/capnproto2\tv2.18.2+incompatible\th1:v3BD1zbruvffn7zjJUU5Pn8nZAB11bhZSQC4W+YnnKo=\n\tbuild\t-buildmode=exe\n\tbuild\t-compiler=gc\n\tbuild\t-tags=with_gvisor,with_quic,with_dhcp,with_wireguard,with_utls,with_acme,with_clash_api,with_tailscale,with_ccm,with_ocm,with_cloudflared,with_naive_outbound,with_usbip,with_openvpn,with_openconnect,badlinkname,tfogo_checklinkname0\n\tbuild\t-trimpath=true\n\tbuild\tCGO_ENABLED=1\n\tbuild\tGOARCH=arm64\n\tbuild\tGOOS=darwin\n\tbuild\tGOARM64=v8.0\n\tbuild\tvcs=git\n\tbuild\tvcs.revision=aafc521b745e0d1a8f50a6a3f41169433f1c2194\n\tbuild\tvcs.time=2026-10-10T20:06:56Z\n\tbuild\tvcs.modified=false\n","schema":"polaris-dns-technical-native-v1","BuildID":"polaris-dns-technical-v1-0e93b72fd119542de973937266b1c2fb6d743a0f9018160964e7c77cf7cc847e","AppAdmissible":false,"publicationEligible":false,"receiptSha256":"31eb933e0579ed39ed1cc5997ee91ffa681cd34a42b763eb1a269551d987f0af"},{"key":"mac-x64","raw":"/Users/runner/work/_temp/polaris-dns-native/polaris-box-1.15.0-alpha.11.polaris.candidate-aafc521b-darwin-amd64/sing-box: go1.25.5\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n\tmod\tgithub.com/sagernet/sing-box\tv0.0.0-20261010200656-aafc521b745e\t\n\tdep\tfilippo.io/edwards25519\tv1.2.0\th1:crnVqOiS4jqYleHd9vaKZ+HKtHfllngJIiOpNpoJsjo=\n\tdep\tgithub.com/RyuaNerin/go-krypto\tv1.3.0\th1:smavTzSMAx8iuVlGb4pEwl9MD2qicqMzuXR2QWp2/Pg=\n\tdep\tgithub.com/ajg/form\tv1.5.1\th1:t9c7v8JUKu/XxOGBU0yjNpaMloxGEJhUkqFRq0ibGeU=\n\tdep\tgithub.com/anchore/go-lzo\tv0.1.0\th1:NgAacnzqPeGH49Ky19QKLBZEuFRqtTG9cdaucc3Vncs=\n\tdep\tgithub.com/andybalholm/brotli\tv1.1.0\th1:eLKJA0d02Lf0mVpIDgYnqXcUn0GqVmEFny3VuID1U3M=\n\tdep\tgithub.com/anmitsu/go-shlex\tv0.0.0-20200514113438-38f4b401e2be\th1:9AeTilPcZAjCFIImctFaOjnTIavg87rW78vTPkQqLI8=\n\tdep\tgithub.com/anthropics/anthropic-sdk-go\tv1.26.0\th1:oUTzFaUpAevfuELAP1sjL6CQJ9HHAfT7CoSYSac11PY=\n\tdep\tgithub.com/axiomhq/hyperloglog\tv0.0.0-20240319100328-84253e514e02\th1:bXAPYSbdYbS5VTy92NIUbeDI1qyggi+JYh5op9IFlcQ=\n\tdep\tgithub.com/caddyserver/certmagic\tv0.25.3-0.20260421143802-60d9d8b415d6\th1:LYSB6VgWzKtNrcxElw3c97BP40Oc7bizKxA9K1Vi/5k=\n\tdep\tgithub.com/caddyserver/zerossl\tv0.1.5\th1:dkvOjBAEEtY6LIGAHei7sw2UgqSD6TrWweXpV7lvEvE=\n\tdep\tgithub.com/clipperhouse/uax29/v2\tv2.2.0\th1:ChwIKnQN3kcZteTXMgb1wztSgaU+ZemkgWdohwgs8tY=\n\tdep\tgithub.com/coder/websocket\tv1.8.14\th1:9L0p0iKiNOibykf283eHkKUHHrpG7f65OE3BhhO7v9g=\n\tdep\tgithub.com/coreos/go-oidc/v3\tv3.17.0\th1:hWBGaQfbi0iVviX4ibC7bk8OKT5qNr4klBaCHVNvehc=\n\tdep\tgithub.com/creack/pty\tv1.1.24\th1:bJrF4RRfyJnbTJqzRLHzcGaZK1NeM5kTC9jGgovnR1s=\n\tdep\tgithub.com/cretz/bine\tv0.2.0\th1:8GiDRGlTgz+o8H9DSnsl+5MeBK4HsExxgl6WgzOCuZo=\n\tdep\tgithub.com/database64128/netx-go\tv0.1.1\th1:dT5LG7Gs7zFZBthFBbzWE6K8wAHjSNAaK7wCYZT7NzM=\n\tdep\tgithub.com/database64128/tfo-go/v2\tv2.3.2\th1:UhZMKiMq3swZGUiETkLBDzQnZBPSAeBMClpJGlnJ5Fw=\n\tdep\tgithub.com/dgryski/go-camellia\tv0.0.0-20191119043421-69a8a13fb23d\th1:CPqTNIigGweVPT4CYb+OO2E6XyRKFOmvTHwWRLgCAlE=\n\tdep\tgithub.com/dgryski/go-metro\tv0.0.0-20180109044635-280f6062b5bc\th1:8WFBn63wegobsYAX0YjD+8suexZDga5CctH4CCTx2+8=\n\tdep\tgithub.com/fsnotify/fsnotify\tv1.9.0\th1:2Ml+OJNzbYCTzsxtv8vKSFD9PbJjmhYF14k/jKC7S9k=\n\tdep\tgithub.com/fxamacker/cbor/v2\tv2.9.0\th1:NpKPmjDBgUfBms6tr6JZkTHtfFGcMKsw3eGcmD/sapM=\n\tdep\tgithub.com/gaissmai/bart\tv0.26.1\th1:+w4rnLGNlA2GDVn382Tfe3jOsK5vOr5n4KmigJ9lbTo=\n\tdep\tgithub.com/go-chi/chi/v5\tv5.2.5\th1:Eg4myHZBjyvJmAFjFvWgrqDTXFyOzjj7YIm3L3mu6Ug=\n\tdep\tgithub.com/go-chi/render\tv1.0.3\th1:AsXqd2a1/INaIfUSKq3G5uA8weYx20FOsM7uSoCyyt4=\n\tdep\tgithub.com/go-jose/go-jose/v4\tv4.1.3\th1:CVLmWDhDVRa6Mi/IgCgaopNosCaHz7zrMeF9MlZRkrs=\n\tdep\tgithub.com/go4org/hashtriemap\tv0.0.0-20251130024219-545ba229f689\th1:0psnKZ+N2IP43/SZC8SKx6OpFJwLmQb9m9QyV9BC2f8=\n\tdep\tgithub.com/gobwas/httphead\tv0.1.0\th1:exrUm0f4YX0L7EBwZHuCF4GDp8aJfVeBrlLQrs6NqWU=\n\tdep\tgithub.com/gobwas/pool\tv0.2.1\th1:xfeeEhW7pwmX8nuLVlqbzVc7udMDrwetjEv+TZIz1og=\n\tdep\tgithub.com/gofrs/uuid/v5\tv5.5.1\th1:z1Ce19/JwNidXpy3tOQc3241lnJLKdKyq/xlNvlD4Ng=\n\tdep\tgithub.com/golang/groupcache\tv0.0.0-20241129210726-2c02b8208cf8\th1:f+oWsMOmNPc8JmEHVZIycC7hBoQxHH9pNKQORJNozsQ=\n\tdep\tgithub.com/google/btree\tv1.1.3\th1:CVpQJjYgC4VbzxeGVHfvZrv1ctoYCAI8vbl07Fcxlyg=\n\tdep\tgithub.com/google/certificate-transparency-go\tv1.3.2\th1:9ahSNZF2o7SYMaKaXhAumVEzXB2QaayzII9C8rv7v+A=\n\tdep\tgithub.com/google/gopacket\tv1.1.19\th1:ves8RnFZPGiFnTS0uPQStjwru6uO6h+nlr9j6fL7kF8=\n\tdep\tgithub.com/google/uuid\tv1.6.0\th1:NIvaJDMOsjHA8n1jAhLSgzrAzy1Hgr+hNrb57e+94F0=\n\tdep\tgithub.com/hashicorp/yamux\tv0.1.2\th1:XtB8kyFOyHXYVFnwT5C3+Bdo8gArse7j2AQ0DA0Uey8=\n\tdep\tgithub.com/hdevalence/ed25519consensus\tv0.2.0\th1:37ICyZqdyj0lAZ8P4D1d1id3HqbbG1N3iBb1Tb4rdcU=\n\tdep\tgithub.com/huin/goupnp\tv1.3.0\th1:UvLUlWDNpoUdYzb2TCn+MuTWtcjXKSza2n6CBdQ0xXc=\n\tdep\tgithub.com/insomniacslk/dhcp\tv0.0.0-20260220084031-5adc3eb26f91\th1:u9i04mGE3iliBh0EFuWaKsmcwrLacqGmq1G3XoaM7gY=\n\tdep\tgithub.com/jackpal/go-nat-pmp\tv1.0.2\th1:KzKSgb7qkJvOUTqYl9/Hg/me3pWgBmERKrTGD7BdWus=\n\tdep\tgithub.com/keybase/go-keychain\tv0.0.1\th1:way+bWYa6lDppZoZcgMbYsvC7GxljxrskdNInRtuthU=\n\tdep\tgithub.com/klauspost/compress\tv1.19.1\th1:VsB4HPswih7mmZ8WleSFQ75c/Ui1M4trX5oAsJnhSlk=\n\tdep\tgithub.com/klauspost/cpuid/v2\tv2.3.0\th1:S4CRMLnYUhGeDFDqkGriYKdfoFlDnMtqTiI/sFzhA9Y=\n\tdep\tgithub.com/koron/go-ssdp\tv0.0.4\th1:1IDwrghSKYM7yLf7XCzbByg2sJ/JcNOZRXS2jczTwz0=\n\tdep\tgithub.com/kr/fs\tv0.1.0\th1:Jskdu9ieNAYnjxsi0LbQp1ulIKZV1LAFgK1tWhpZgl8=\n\tdep\tgithub.com/libdns/acmedns\tv0.5.0\th1:5pRtmUj4Lb/QkNJSl1xgOGBUJTWW7RjpNaIhjpDXjPE=\n\tdep\tgithub.com/libdns/alidns\tv1.0.6\th1:/Ii428ty6WHFJmE24rZxq2taq++gh7rf9jhgLfp8PmM=\n\tdep\tgithub.com/libdns/cloudflare\tv0.2.2\th1:XWHv+C1dDcApqazlh08Q6pjytYLgR2a+Y3xrXFu0vsI=\n\tdep\tgithub.com/libdns/libdns\tv1.1.1\th1:wPrHrXILoSHKWJKGd0EiAVmiJbFShguILTg9leS/P/U=\n\tdep\tgithub.com/libp2p/go-nat\tv1.0.1-0.20250821073202-01afc089f138\th1:YohuNPT/1k3VcThCQlBZ43PCPWPfMRS1zcxWBF2SLK8=\n\tdep\tgithub.com/libp2p/go-netroute\tv0.2.1\th1:V8kVrpD8GK0Riv15/7VN6RbUQ3URNZVosw7H2v9tksU=\n\tdep\tgithub.com/logrusorgru/aurora\tv2.0.3+incompatible\th1:tOpm7WcpBTn4fjmVfgpQq0EfczGlG91VSDkswnjF5A8=\n\tdep\tgithub.com/mattn/go-runewidth\tv0.0.27\th1:Feg/Oou5zI/wnpgDF6omIU0OokC9GxLC/WRknhVlIR0=\n\tdep\tgithub.com/metacubex/utls\tv1.8.7\th1:Cp+yWkNTFkSihETgGWq34hlVFds5HpYWVOR1xovUVTs=\n\tdep\tgithub.com/mholt/acmez/v3\tv3.1.6\th1:eGVQNObP0pBN4sxqrXeg7MYqTOWyoiYpQqITVWlrevk=\n\tdep\tgithub.com/miekg/dns\tv1.1.72\th1:vhmr+TF2A3tuoGNkLDFK9zi36F2LS+hKTRW0Uf8kbzI=\n\tdep\tgithub.com/mitchellh/go-ps\tv1.0.0\th1:i6ampVEEF4wQFF+bkYfwYgY+F/uYJDktmvLPf7qIgjc=\n\tdep\tgithub.com/openai/openai-go/v3\tv3.26.0\th1:bRt6H/ozMNt/dDkN4gobnLqaEGrRGBzmbVs0xxJEnQE=\n\tdep\tgithub.com/oschwald/maxminddb-golang\tv1.13.1\th1:G3wwjdN9JmIK2o/ermkHM+98oX5fS+k5MbwsmL4MRQE=\n\tdep\tgithub.com/pierrec/lz4/v4\tv4.1.26\th1:GrpZw1gZttORinvzBdXPUXATeqlJjqUG/D87TKMnhjY=\n\tdep\tgithub.com/pion/dtls/v3\tv3.1.5\th1:9xJtVsHwMYeSjPp5Hh1FTis4DchnQWtnOa5o+6ygqfc=\n\tdep\tgithub.com/pion/logging\tv0.2.4\th1:tTew+7cmQ+Mc1pTBLKH2puKsOvhm32dROumOZ655zB8=\n\tdep\tgithub.com/pion/transport/v4\tv4.0.2\th1:ifYlPqNwsy6aKQ9y8yzxXlHae5431ZrH2avkD/Rn6Tk=\n\tdep\tgithub.com/pires/go-proxyproto\tv0.8.1\th1:9KEixbdJfhrbtjpz/ZwCdWDD2Xem0NZ38qMYaASJgp0=\n\tdep\tgithub.com/pkg/sftp\tv1.13.10\th1:+5FbKNTe5Z9aspU88DPIKJ9z2KZoaGCu6Sr6kKR/5mU=\n\tdep\tgithub.com/quic-go/qpack\tv0.6.0\th1:g7W+BMYynC1LbYLSqRt8PBg5Tgwxn214ZZR34VIOjz8=\n\tdep\tgithub.com/sagernet/bbolt\tv0.0.0-20260915102804-500ee1e84832\th1:KDXMGbzqLmDdKcld0T3wuMuqf1iezZq+DemgtZ/vhtQ=\n\tdep\tgithub.com/sagernet/cors\tv1.2.1\th1:Cv5Z8y9YSD6Gm+qSpNrL3LO4lD3eQVvbFYJSG7JCMHQ=\n\tdep\tgithub.com/sagernet/cronet-go\tv0.0.0-20260929213745-4d18a60dc3a8\th1:yW/KNhSxWxllsPjLx5EUPohHLa4Kvzl2Qt9kHkB0ZIc=\n\tdep\tgithub.com/sagernet/cronet-go/all\tv0.0.0-20260929213745-4d18a60dc3a8\th1:xhh9UrPFncqI8r2mr+bybgZyJICtV4GKOSJuhnEx75g=\n\tdep\tgithub.com/sagernet/cronet-go/lib/darwin_amd64\tv0.0.0-20260929213014-a1cafd93eb1f\th1:xenAxIaoocXqtaNECC1iNOqtdzYRYxhXyE+o+m2ExSw=\n\tdep\tgithub.com/sagernet/fswatch\tv0.1.2\th1:/TT7k4mkce1qFPxamLO842WjqBgbTBiXP2mlUjp9PFk=\n\tdep\tgithub.com/sagernet/gliderssh\tv0.3.4-0.20260531100337-2194faca5648\th1:IWVjKBARzVjdmH0VUaeTBOBli1qkwKmTG4XfbkpSS20=\n\tdep\tgithub.com/sagernet/gvisor\tv0.0.0-20260727.0-sing-box-mod.1\th1:IdQ7yTKkB2wv8txwshxUroPlO4npOYAV71xb7xQ7Lys=\n\tdep\tgithub.com/sagernet/quic-go\tv0.61.0-sing-box-mod.9\th1:Lojx77uC63TxZO38Z1OSo6aOoBSXY+ueH0/IKKeTZuI=\n\tdep\tgithub.com/sagernet/sing\tv0.9.7-0.20260929150544-6f21f2425a95\th1:taM2SOI5HK/Ubpi3FO0Bc7bKHsFVdGe4YS/jjXD4kAM=\n\tdep\tgithub.com/sagernet/sing-anytls\tv0.0.0-20260928104022-580984e4d8cb\th1:n4sF2zv0CR1gcb4awh61t11t0FZmcP40x5enB8HARjE=\n\tdep\tgithub.com/sagernet/sing-cloudflared\tv0.1.4-0.20261002084126-c1255ae368f2\th1:+AfbAXGEV8tezIYHeShzwHm7IMUE5kYGVFLo0ly7W24=\n\tdep\tgithub.com/sagernet/sing-mux\tv0.3.10-0.20260929204512-caf09fe32475\th1:aG0DqHEOjax7DyfE1GTAXZotOg3QGVCz3/8rCSua2kg=\n\tdep\tgithub.com/sagernet/sing-openconnect\tv0.1.1-0.20260929151226-29757281a247\th1:Ui4XtxNgQiEquK+ch4dYhthKsVlGQpdCdKIpOuJVyog=\n\tdep\tgithub.com/sagernet/sing-openvpn\tv0.1.1-0.20260929151220-f330676d6d4a\th1:Q16Twlwk9fssoCSUTDo5WqPg0EZHG9JSV9SC3zRy9M0=\n\tdep\tgithub.com/sagernet/sing-quic\tv0.7.2-0.20261002084117-75c3ac4fa12b\th1:2Jx/URJAPt32MEbLmAR5nzH87tO6NQZLRfRFvFqBWuk=\n\tdep\tgithub.com/sagernet/sing-shadowsocks\tv0.2.9-0.20260929204512-65740e0f0e3e\th1:LKgE/uuqp/D2kgxYViOz8vf4slaCUfpFywXjtuwD5gU=\n\tdep\tgithub.com/sagernet/sing-shadowsocks2\tv0.2.2-0.20260929152114-a69d1086332b\th1:VH65QNqSHat2b+N3Rlzo5Qe44gRmQGwzY5atuy3D0A8=\n\tdep\tgithub.com/sagernet/sing-shadowtls\tv0.2.2-0.20260928201441-a9c0127d5c99\th1:l/tdZjaoDp3iqEG4NVrFHtphy1+Qjt9bpWAgNbF8iGg=\n\tdep\tgithub.com/sagernet/sing-snell\tv0.0.0-20260904135315-bc5a12ac736f\th1:tbTCI+iis5sTmua7a6Q7vebcSRURok/LGgnEafnQAsc=\n\tdep\tgithub.com/sagernet/sing-tun\tv0.9.7-0.20261009022811-5c2edb183cc9\n\t=>\t./third_party/sing-tun\t(devel)\t\n\t\n\tdep\tgithub.com/sagernet/sing-usbip\tv0.0.0-20260817040617-28bd42667eca\th1:5wA+IE0Fq1CGVLOgSpm0gKKZ03HzugcJ2JyhmDqeX6A=\n\tdep\tgithub.com/sagernet/sing-vmess\tv0.2.9-0.20260929152519-9b95ab8c9478\th1:q2eQn4nq8oWGxRm9zXesXSlPr9GEgPrOGzf4iPtzG9A=\n\tdep\tgithub.com/sagernet/smux\tv1.5.50-sing-box-mod.1\th1:XkJcivBC9V4wBjiGXIXZ229aZCU1hzcbp6kSkkyQ478=\n\tdep\tgithub.com/sagernet/tailscale\tv1.102.1-sing-box-1.14-mod.5.0.20260925112514-35e61219dedd\th1:PzEFeBqDaSUdoWE77NptloLTd1sNT1JAiFThgGq5rlo=\n\tdep\tgithub.com/sagernet/wireguard-go\tv0.0.8-0.20260929150556-ca3bc60c4ce7\th1:MiffQUqKBKoAHz7nkHqh4EmjJsz2M8FHeUdPUdnNyvs=\n\tdep\tgithub.com/sagernet/ws\tv0.0.0-20231204124109-acfe8907c854\th1:6uUiZcDRnZSAegryaUGwPC/Fj13JSHwiTftrXhMmYOc=\n\tdep\tgithub.com/smallstep/pkcs7\tv0.1.1\th1:x+rPdt2W088V9Vkjho4KtoggyktZJlMduZAtRHm68LU=\n\tdep\tgithub.com/spf13/cobra\tv1.10.2\th1:DMTTonx5m65Ic0GOoRY2c16WCbHxOOw6xxezuLaBpcU=\n\tdep\tgithub.com/spf13/pflag\tv1.0.10\th1:4EBh2KAYBwaONj6b2Ye1GiHfwjqyROoF4RwYO+vPwFk=\n\tdep\tgithub.com/tailscale/certstore\tv0.1.1-0.20260409135935-3638fb84b77d\th1:JcGKBZAL7ePLwOhUdN8qGQZlP5GueEiIZwY7R62pejE=\n\tdep\tgithub.com/tailscale/hujson\tv0.0.0-20260302212456-ecc657c15afd\th1:Rf9uhF1+VJ7ZHqxrG8pJ6YacmHvVCmByDmGbAWCc/gA=\n\tdep\tgithub.com/tailscale/peercred\tv0.0.0-20250107143737-35a0c7bd7edc\th1:24heQPtnFR+yfntqhI3oAu9i27nEojcQ4NuBQOo5ZFA=\n\tdep\tgithub.com/tailscale/web-client-prebuilt\tv0.0.0-20250124233751-d4cd19a26976\th1:UBPHPtv8+nEAy2PD8RyAhOYvau1ek0HDJqLS/Pysi14=\n\tdep\tgithub.com/tidwall/gjson\tv1.18.0\th1:FIDeeyB800efLX89e5a8Y0BNH+LOngJyGrIWxG2FKQY=\n\tdep\tgithub.com/tidwall/match\tv1.1.1\th1:+Ho715JplO36QYgwN9PGYNhgZvoUSc9X2c80KVTi+GA=\n\tdep\tgithub.com/tidwall/pretty\tv1.2.1\th1:qjsOFOWWQl+N3RsoF5/ssm1pHmJJwhjlSbZ51I6wMl4=\n\tdep\tgithub.com/tidwall/sjson\tv1.2.5\th1:kLy8mja+1c9jlljvWTlSazM7cKDRfJuR/bOJhcY5NcY=\n\tdep\tgithub.com/tjfoc/gmsm\tv1.4.1\th1:aMe1GlZb+0bLjn+cKTPEvvn9oUEBlJitaZiiBwsbgho=\n\tdep\tgithub.com/u-root/uio\tv0.0.0-20240224005618-d2acac8f3701\th1:pyC9PaHYZFgEKFdlp3G8RaCKgVpHZnecvArXvPXcFkM=\n\tdep\tgithub.com/vishvananda/netns\tv0.0.5\th1:DfiHV+j8bA32MFM7bfEunvT8IAqQ/NzSJHtcmW5zdEY=\n\tdep\tgithub.com/x448/float16\tv0.8.4\th1:qLwI1I70+NjRFUR3zs1JPUCgaCXSh3SW62uAKT1mSBM=\n\tdep\tgithub.com/youmark/pkcs8\tv0.0.0-20240726163527-a2c0da244d78\th1:ilQV1hzziu+LLM3zUTJ0trRztfwgjqKnBWNtSRkbmwM=\n\tdep\tgithub.com/zeebo/blake3\tv0.2.4\th1:KYQPkhpRtcqh0ssGYcKLG1JYvddkEA8QwCM/yBqhaZI=\n\tdep\tgo.uber.org/multierr\tv1.11.0\th1:blXXJkSxSSfBVBlC76pxqeO+LN3aDfLQo+309xJstO0=\n\tdep\tgo.uber.org/zap\tv1.27.1\th1:08RqriUEv8+ArZRYSTXy1LeBScaMpVSTBhCeaZYfMYc=\n\tdep\tgo.uber.org/zap/exp\tv0.3.0\th1:6JYzdifzYkGmTdRR59oYH+Ng7k49H9qVpWwNSsGJj3U=\n\tdep\tgo4.org/mem\tv0.0.0-20240501181205-ae6ca9944745\th1:Tl++JLUCe4sxGu8cTpDzRLd3tN7US4hOxG5YpKCzkek=\n\tdep\tgo4.org/netipx\tv0.0.0-20231129151722-fdeea329fbba\th1:0b9z3AuHCjxk0x/opv64kcgZLBseWJUpBw5I82+2U4M=\n\tdep\tgolang.org/x/crypto\tv0.54.0\th1:YLIA59K4fiNzHzjnZt2tUJQjQtUWfWbeHBqKtk3eScw=\n\tdep\tgolang.org/x/exp\tv0.0.0-20260410095643-746e56fc9e2f\th1:W3F4c+6OLc6H2lb//N1q4WpJkhzJCK5J6kUi1NTVXfM=\n\tdep\tgolang.org/x/mod\tv0.37.0\th1:vF1DjpVEshcIqoEaauuHebaLk1O1forxjxBaVn884JQ=\n\tdep\tgolang.org/x/net\tv0.57.0\th1:K5+3DljvIuDG9/Jv9rvyMywYNFCQ9RSUY6OOTTkT+tE=\n\tdep\tgolang.org/x/oauth2\tv0.36.0\th1:peZ/1z27fi9hUOFCAZaHyrpWG5lwe0RJEEEeH0ThlIs=\n\tdep\tgolang.org/x/sync\tv0.22.0\th1:SZjpbeLmrCk4xhRSZFNZW5gFUeCeFgjekvI/+gfScek=\n\tdep\tgolang.org/x/sys\tv0.47.0\th1:o7XGOvZQCADBQQ4Y7VNq2dRWQR7JmOUW8Kxx4ZsNgWs=\n\tdep\tgolang.org/x/term\tv0.45.0\th1:NwWyBmoJCbfTHpxrWoZ9C6/VxOf7ic219I8xZZFdrf0=\n\tdep\tgolang.org/x/text\tv0.40.0\th1:Ub2Z6/xjgF1WrYQz2nuITOEegKFtiIy+rieRJ5lHZKs=\n\tdep\tgolang.org/x/time\tv0.15.0\th1:bbrp8t3bGUeFOx08pvsMYRTCVSMk89u4tKbNOZbp88U=\n\tdep\tgolang.zx2c4.com/wireguard/wgctrl\tv0.0.0-20241231184526-a9ab2273dd10\th1:3GDAcqdIg1ozBNLgPy4SLT84nfcBjr6rhGtXYtrkWLU=\n\tdep\tgoogle.golang.org/genproto/googleapis/rpc\tv0.0.0-20251202230838-ff82c1b0f217\th1:gRkg/vSppuSQoDjxyiGfN4Upv/h/DQmIR10ZU8dh4Ww=\n\tdep\tgoogle.golang.org/grpc\tv1.79.1\th1:zGhSi45ODB9/p3VAawt9a+O/MULLl9dpizzNNpq7flY=\n\tdep\tgoogle.golang.org/protobuf\tv1.36.11\th1:fV6ZwhNocDyBLK0dj+fg8ektcVegBBuEolpbTQyBNVE=\n\tdep\tlukechampine.com/blake3\tv1.3.0\th1:sJ3XhFINmHSrYCgl958hscfIa3bw8x4DqMP3u1YvoYE=\n\tdep\tzombiezen.com/go/capnproto2\tv2.18.2+incompatible\th1:v3BD1zbruvffn7zjJUU5Pn8nZAB11bhZSQC4W+YnnKo=\n\tbuild\t-buildmode=exe\n\tbuild\t-compiler=gc\n\tbuild\t-tags=with_gvisor,with_quic,with_dhcp,with_wireguard,with_utls,with_acme,with_clash_api,with_tailscale,with_ccm,with_ocm,with_cloudflared,with_naive_outbound,with_usbip,with_openvpn,with_openconnect,badlinkname,tfogo_checklinkname0\n\tbuild\t-trimpath=true\n\tbuild\tCGO_ENABLED=1\n\tbuild\tGOARCH=amd64\n\tbuild\tGOOS=darwin\n\tbuild\tGOAMD64=v1\n\tbuild\tvcs=git\n\tbuild\tvcs.revision=aafc521b745e0d1a8f50a6a3f41169433f1c2194\n\tbuild\tvcs.time=2026-10-10T20:06:56Z\n\tbuild\tvcs.modified=false\n","schema":"polaris-dns-technical-native-v1","BuildID":"polaris-dns-technical-v1-4036fb8f82c1e7da5bf7260a052f6d81d99f8480495fbe1e87b589cf25ca4204","AppAdmissible":false,"publicationEligible":false,"receiptSha256":"a9d77843abb01c4b385535135e35d71620bc657212b81331ca26ddafdce79cf7"},{"key":"linux","raw":"/home/runner/work/_temp/polaris-dns-native/polaris-box-1.15.0-alpha.11.polaris.candidate-aafc521b-linux-amd64/sing-box: go1.25.5\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n\tmod\tgithub.com/sagernet/sing-box\tv0.0.0-20261010200656-aafc521b745e\t\n\tdep\tfilippo.io/edwards25519\tv1.2.0\th1:crnVqOiS4jqYleHd9vaKZ+HKtHfllngJIiOpNpoJsjo=\n\tdep\tgithub.com/RyuaNerin/go-krypto\tv1.3.0\th1:smavTzSMAx8iuVlGb4pEwl9MD2qicqMzuXR2QWp2/Pg=\n\tdep\tgithub.com/ajg/form\tv1.5.1\th1:t9c7v8JUKu/XxOGBU0yjNpaMloxGEJhUkqFRq0ibGeU=\n\tdep\tgithub.com/anchore/go-lzo\tv0.1.0\th1:NgAacnzqPeGH49Ky19QKLBZEuFRqtTG9cdaucc3Vncs=\n\tdep\tgithub.com/andybalholm/brotli\tv1.1.0\th1:eLKJA0d02Lf0mVpIDgYnqXcUn0GqVmEFny3VuID1U3M=\n\tdep\tgithub.com/anmitsu/go-shlex\tv0.0.0-20200514113438-38f4b401e2be\th1:9AeTilPcZAjCFIImctFaOjnTIavg87rW78vTPkQqLI8=\n\tdep\tgithub.com/anthropics/anthropic-sdk-go\tv1.26.0\th1:oUTzFaUpAevfuELAP1sjL6CQJ9HHAfT7CoSYSac11PY=\n\tdep\tgithub.com/axiomhq/hyperloglog\tv0.0.0-20240319100328-84253e514e02\th1:bXAPYSbdYbS5VTy92NIUbeDI1qyggi+JYh5op9IFlcQ=\n\tdep\tgithub.com/caddyserver/certmagic\tv0.25.3-0.20260421143802-60d9d8b415d6\th1:LYSB6VgWzKtNrcxElw3c97BP40Oc7bizKxA9K1Vi/5k=\n\tdep\tgithub.com/caddyserver/zerossl\tv0.1.5\th1:dkvOjBAEEtY6LIGAHei7sw2UgqSD6TrWweXpV7lvEvE=\n\tdep\tgithub.com/clipperhouse/uax29/v2\tv2.2.0\th1:ChwIKnQN3kcZteTXMgb1wztSgaU+ZemkgWdohwgs8tY=\n\tdep\tgithub.com/coder/websocket\tv1.8.14\th1:9L0p0iKiNOibykf283eHkKUHHrpG7f65OE3BhhO7v9g=\n\tdep\tgithub.com/coreos/go-iptables\tv0.7.1-0.20240112124308-65c67c9f46e6\th1:8h5+bWd7R6AYUslN6c6iuZWTKsKxUFDlpnmilO6R2n0=\n\tdep\tgithub.com/coreos/go-oidc/v3\tv3.17.0\th1:hWBGaQfbi0iVviX4ibC7bk8OKT5qNr4klBaCHVNvehc=\n\tdep\tgithub.com/creack/pty\tv1.1.24\th1:bJrF4RRfyJnbTJqzRLHzcGaZK1NeM5kTC9jGgovnR1s=\n\tdep\tgithub.com/cretz/bine\tv0.2.0\th1:8GiDRGlTgz+o8H9DSnsl+5MeBK4HsExxgl6WgzOCuZo=\n\tdep\tgithub.com/database64128/netx-go\tv0.1.1\th1:dT5LG7Gs7zFZBthFBbzWE6K8wAHjSNAaK7wCYZT7NzM=\n\tdep\tgithub.com/database64128/tfo-go/v2\tv2.3.2\th1:UhZMKiMq3swZGUiETkLBDzQnZBPSAeBMClpJGlnJ5Fw=\n\tdep\tgithub.com/dgryski/go-camellia\tv0.0.0-20191119043421-69a8a13fb23d\th1:CPqTNIigGweVPT4CYb+OO2E6XyRKFOmvTHwWRLgCAlE=\n\tdep\tgithub.com/dgryski/go-metro\tv0.0.0-20180109044635-280f6062b5bc\th1:8WFBn63wegobsYAX0YjD+8suexZDga5CctH4CCTx2+8=\n\tdep\tgithub.com/ebitengine/purego\tv0.10.0\th1:QIw4xfpWT6GWTzaW5XEKy3HXoqrJGx1ijYHzTF0/ISU=\n\tdep\tgithub.com/florianl/go-nfqueue/v2\tv2.1.0\th1:Fywt30TY/evxyDySpXjxQ1jsRW7nQbLpOhELqpr4068=\n\tdep\tgithub.com/fsnotify/fsnotify\tv1.9.0\th1:2Ml+OJNzbYCTzsxtv8vKSFD9PbJjmhYF14k/jKC7S9k=\n\tdep\tgithub.com/fxamacker/cbor/v2\tv2.9.0\th1:NpKPmjDBgUfBms6tr6JZkTHtfFGcMKsw3eGcmD/sapM=\n\tdep\tgithub.com/gaissmai/bart\tv0.26.1\th1:+w4rnLGNlA2GDVn382Tfe3jOsK5vOr5n4KmigJ9lbTo=\n\tdep\tgithub.com/go-chi/chi/v5\tv5.2.5\th1:Eg4myHZBjyvJmAFjFvWgrqDTXFyOzjj7YIm3L3mu6Ug=\n\tdep\tgithub.com/go-chi/render\tv1.0.3\th1:AsXqd2a1/INaIfUSKq3G5uA8weYx20FOsM7uSoCyyt4=\n\tdep\tgithub.com/go-jose/go-jose/v4\tv4.1.3\th1:CVLmWDhDVRa6Mi/IgCgaopNosCaHz7zrMeF9MlZRkrs=\n\tdep\tgithub.com/go4org/hashtriemap\tv0.0.0-20251130024219-545ba229f689\th1:0psnKZ+N2IP43/SZC8SKx6OpFJwLmQb9m9QyV9BC2f8=\n\tdep\tgithub.com/gobwas/httphead\tv0.1.0\th1:exrUm0f4YX0L7EBwZHuCF4GDp8aJfVeBrlLQrs6NqWU=\n\tdep\tgithub.com/gobwas/pool\tv0.2.1\th1:xfeeEhW7pwmX8nuLVlqbzVc7udMDrwetjEv+TZIz1og=\n\tdep\tgithub.com/godbus/dbus/v5\tv5.2.2\th1:TUR3TgtSVDmjiXOgAAyaZbYmIeP3DPkld3jgKGV8mXQ=\n\tdep\tgithub.com/gofrs/uuid/v5\tv5.5.1\th1:z1Ce19/JwNidXpy3tOQc3241lnJLKdKyq/xlNvlD4Ng=\n\tdep\tgithub.com/golang/groupcache\tv0.0.0-20241129210726-2c02b8208cf8\th1:f+oWsMOmNPc8JmEHVZIycC7hBoQxHH9pNKQORJNozsQ=\n\tdep\tgithub.com/google/btree\tv1.1.3\th1:CVpQJjYgC4VbzxeGVHfvZrv1ctoYCAI8vbl07Fcxlyg=\n\tdep\tgithub.com/google/certificate-transparency-go\tv1.3.2\th1:9ahSNZF2o7SYMaKaXhAumVEzXB2QaayzII9C8rv7v+A=\n\tdep\tgithub.com/google/gopacket\tv1.1.19\th1:ves8RnFZPGiFnTS0uPQStjwru6uO6h+nlr9j6fL7kF8=\n\tdep\tgithub.com/google/nftables\tv0.2.1-0.20240414091927-5e242ec57806\th1:wG8RYIyctLhdFk6Vl1yPGtSRtwGpVkWyZww1OCil2MI=\n\tdep\tgithub.com/google/uuid\tv1.6.0\th1:NIvaJDMOsjHA8n1jAhLSgzrAzy1Hgr+hNrb57e+94F0=\n\tdep\tgithub.com/hashicorp/yamux\tv0.1.2\th1:XtB8kyFOyHXYVFnwT5C3+Bdo8gArse7j2AQ0DA0Uey8=\n\tdep\tgithub.com/hdevalence/ed25519consensus\tv0.2.0\th1:37ICyZqdyj0lAZ8P4D1d1id3HqbbG1N3iBb1Tb4rdcU=\n\tdep\tgithub.com/huin/goupnp\tv1.3.0\th1:UvLUlWDNpoUdYzb2TCn+MuTWtcjXKSza2n6CBdQ0xXc=\n\tdep\tgithub.com/insomniacslk/dhcp\tv0.0.0-20260220084031-5adc3eb26f91\th1:u9i04mGE3iliBh0EFuWaKsmcwrLacqGmq1G3XoaM7gY=\n\tdep\tgithub.com/jackpal/go-nat-pmp\tv1.0.2\th1:KzKSgb7qkJvOUTqYl9/Hg/me3pWgBmERKrTGD7BdWus=\n\tdep\tgithub.com/jsimonetti/rtnetlink\tv1.4.1\th1:JfD4jthWBqZMEffc5RjgmlzpYttAVw1sdnmiNaPO3hE=\n\tdep\tgithub.com/klauspost/compress\tv1.19.1\th1:VsB4HPswih7mmZ8WleSFQ75c/Ui1M4trX5oAsJnhSlk=\n\tdep\tgithub.com/klauspost/cpuid/v2\tv2.3.0\th1:S4CRMLnYUhGeDFDqkGriYKdfoFlDnMtqTiI/sFzhA9Y=\n\tdep\tgithub.com/koron/go-ssdp\tv0.0.4\th1:1IDwrghSKYM7yLf7XCzbByg2sJ/JcNOZRXS2jczTwz0=\n\tdep\tgithub.com/kr/fs\tv0.1.0\th1:Jskdu9ieNAYnjxsi0LbQp1ulIKZV1LAFgK1tWhpZgl8=\n\tdep\tgithub.com/libdns/acmedns\tv0.5.0\th1:5pRtmUj4Lb/QkNJSl1xgOGBUJTWW7RjpNaIhjpDXjPE=\n\tdep\tgithub.com/libdns/alidns\tv1.0.6\th1:/Ii428ty6WHFJmE24rZxq2taq++gh7rf9jhgLfp8PmM=\n\tdep\tgithub.com/libdns/cloudflare\tv0.2.2\th1:XWHv+C1dDcApqazlh08Q6pjytYLgR2a+Y3xrXFu0vsI=\n\tdep\tgithub.com/libdns/libdns\tv1.1.1\th1:wPrHrXILoSHKWJKGd0EiAVmiJbFShguILTg9leS/P/U=\n\tdep\tgithub.com/libp2p/go-nat\tv1.0.1-0.20250821073202-01afc089f138\th1:YohuNPT/1k3VcThCQlBZ43PCPWPfMRS1zcxWBF2SLK8=\n\tdep\tgithub.com/libp2p/go-netroute\tv0.2.1\th1:V8kVrpD8GK0Riv15/7VN6RbUQ3URNZVosw7H2v9tksU=\n\tdep\tgithub.com/logrusorgru/aurora\tv2.0.3+incompatible\th1:tOpm7WcpBTn4fjmVfgpQq0EfczGlG91VSDkswnjF5A8=\n\tdep\tgithub.com/mattn/go-runewidth\tv0.0.27\th1:Feg/Oou5zI/wnpgDF6omIU0OokC9GxLC/WRknhVlIR0=\n\tdep\tgithub.com/mdlayher/netlink\tv1.11.2\th1:HKh2jqe+omdSWcQ88nrT7INE61B0NXfiSPFdgL4YbNI=\n\tdep\tgithub.com/mdlayher/socket\tv0.6.0\th1:ScZPaAGyO1icQnbFrhPM8mnXyMu9qukC1K4ZoM2IQKU=\n\tdep\tgithub.com/metacubex/utls\tv1.8.7\th1:Cp+yWkNTFkSihETgGWq34hlVFds5HpYWVOR1xovUVTs=\n\tdep\tgithub.com/mholt/acmez/v3\tv3.1.6\th1:eGVQNObP0pBN4sxqrXeg7MYqTOWyoiYpQqITVWlrevk=\n\tdep\tgithub.com/miekg/dns\tv1.1.72\th1:vhmr+TF2A3tuoGNkLDFK9zi36F2LS+hKTRW0Uf8kbzI=\n\tdep\tgithub.com/mitchellh/go-ps\tv1.0.0\th1:i6ampVEEF4wQFF+bkYfwYgY+F/uYJDktmvLPf7qIgjc=\n\tdep\tgithub.com/openai/openai-go/v3\tv3.26.0\th1:bRt6H/ozMNt/dDkN4gobnLqaEGrRGBzmbVs0xxJEnQE=\n\tdep\tgithub.com/oschwald/maxminddb-golang\tv1.13.1\th1:G3wwjdN9JmIK2o/ermkHM+98oX5fS+k5MbwsmL4MRQE=\n\tdep\tgithub.com/pierrec/lz4/v4\tv4.1.26\th1:GrpZw1gZttORinvzBdXPUXATeqlJjqUG/D87TKMnhjY=\n\tdep\tgithub.com/pion/dtls/v3\tv3.1.5\th1:9xJtVsHwMYeSjPp5Hh1FTis4DchnQWtnOa5o+6ygqfc=\n\tdep\tgithub.com/pion/logging\tv0.2.4\th1:tTew+7cmQ+Mc1pTBLKH2puKsOvhm32dROumOZ655zB8=\n\tdep\tgithub.com/pion/transport/v4\tv4.0.2\th1:ifYlPqNwsy6aKQ9y8yzxXlHae5431ZrH2avkD/Rn6Tk=\n\tdep\tgithub.com/pires/go-proxyproto\tv0.8.1\th1:9KEixbdJfhrbtjpz/ZwCdWDD2Xem0NZ38qMYaASJgp0=\n\tdep\tgithub.com/pkg/sftp\tv1.13.10\th1:+5FbKNTe5Z9aspU88DPIKJ9z2KZoaGCu6Sr6kKR/5mU=\n\tdep\tgithub.com/quic-go/qpack\tv0.6.0\th1:g7W+BMYynC1LbYLSqRt8PBg5Tgwxn214ZZR34VIOjz8=\n\tdep\tgithub.com/safchain/ethtool\tv0.3.0\th1:gimQJpsI6sc1yIqP/y8GYgiXn/NjgvpM0RNoWLVVmP0=\n\tdep\tgithub.com/sagernet/bbolt\tv0.0.0-20260915102804-500ee1e84832\th1:KDXMGbzqLmDdKcld0T3wuMuqf1iezZq+DemgtZ/vhtQ=\n\tdep\tgithub.com/sagernet/cors\tv1.2.1\th1:Cv5Z8y9YSD6Gm+qSpNrL3LO4lD3eQVvbFYJSG7JCMHQ=\n\tdep\tgithub.com/sagernet/cronet-go\tv0.0.0-20260929213745-4d18a60dc3a8\th1:yW/KNhSxWxllsPjLx5EUPohHLa4Kvzl2Qt9kHkB0ZIc=\n\tdep\tgithub.com/sagernet/cronet-go/all\tv0.0.0-20260929213745-4d18a60dc3a8\th1:xhh9UrPFncqI8r2mr+bybgZyJICtV4GKOSJuhnEx75g=\n\tdep\tgithub.com/sagernet/cronet-go/lib/linux_amd64\tv0.0.0-20260929213014-a1cafd93eb1f\th1:dAbYhebHYrOBOvJux8hcuj4yTEIT+UqZmuTBKsLs6cQ=\n\tdep\tgithub.com/sagernet/fswatch\tv0.1.2\th1:/TT7k4mkce1qFPxamLO842WjqBgbTBiXP2mlUjp9PFk=\n\tdep\tgithub.com/sagernet/gliderssh\tv0.3.4-0.20260531100337-2194faca5648\th1:IWVjKBARzVjdmH0VUaeTBOBli1qkwKmTG4XfbkpSS20=\n\tdep\tgithub.com/sagernet/gvisor\tv0.0.0-20260727.0-sing-box-mod.1\th1:IdQ7yTKkB2wv8txwshxUroPlO4npOYAV71xb7xQ7Lys=\n\tdep\tgithub.com/sagernet/netlink\tv0.0.0-20260814022025-64455d367bbf\th1:b/3rm+KvxzuYRcnMdLq/hmcqX6/2B8k24l6SDHL2Piw=\n\tdep\tgithub.com/sagernet/nftables\tv0.3.0-mod.4\th1:vnOtcDYeSXv2e5RoRuGH0lrpttQFJ8iC4ICS2nhlDSo=\n\tdep\tgithub.com/sagernet/quic-go\tv0.61.0-sing-box-mod.9\th1:Lojx77uC63TxZO38Z1OSo6aOoBSXY+ueH0/IKKeTZuI=\n\tdep\tgithub.com/sagernet/sing\tv0.9.7-0.20260929150544-6f21f2425a95\th1:taM2SOI5HK/Ubpi3FO0Bc7bKHsFVdGe4YS/jjXD4kAM=\n\tdep\tgithub.com/sagernet/sing-anytls\tv0.0.0-20260928104022-580984e4d8cb\th1:n4sF2zv0CR1gcb4awh61t11t0FZmcP40x5enB8HARjE=\n\tdep\tgithub.com/sagernet/sing-cloudflared\tv0.1.4-0.20261002084126-c1255ae368f2\th1:+AfbAXGEV8tezIYHeShzwHm7IMUE5kYGVFLo0ly7W24=\n\tdep\tgithub.com/sagernet/sing-mux\tv0.3.10-0.20260929204512-caf09fe32475\th1:aG0DqHEOjax7DyfE1GTAXZotOg3QGVCz3/8rCSua2kg=\n\tdep\tgithub.com/sagernet/sing-openconnect\tv0.1.1-0.20260929151226-29757281a247\th1:Ui4XtxNgQiEquK+ch4dYhthKsVlGQpdCdKIpOuJVyog=\n\tdep\tgithub.com/sagernet/sing-openvpn\tv0.1.1-0.20260929151220-f330676d6d4a\th1:Q16Twlwk9fssoCSUTDo5WqPg0EZHG9JSV9SC3zRy9M0=\n\tdep\tgithub.com/sagernet/sing-quic\tv0.7.2-0.20261002084117-75c3ac4fa12b\th1:2Jx/URJAPt32MEbLmAR5nzH87tO6NQZLRfRFvFqBWuk=\n\tdep\tgithub.com/sagernet/sing-shadowsocks\tv0.2.9-0.20260929204512-65740e0f0e3e\th1:LKgE/uuqp/D2kgxYViOz8vf4slaCUfpFywXjtuwD5gU=\n\tdep\tgithub.com/sagernet/sing-shadowsocks2\tv0.2.2-0.20260929152114-a69d1086332b\th1:VH65QNqSHat2b+N3Rlzo5Qe44gRmQGwzY5atuy3D0A8=\n\tdep\tgithub.com/sagernet/sing-shadowtls\tv0.2.2-0.20260928201441-a9c0127d5c99\th1:l/tdZjaoDp3iqEG4NVrFHtphy1+Qjt9bpWAgNbF8iGg=\n\tdep\tgithub.com/sagernet/sing-snell\tv0.0.0-20260904135315-bc5a12ac736f\th1:tbTCI+iis5sTmua7a6Q7vebcSRURok/LGgnEafnQAsc=\n\tdep\tgithub.com/sagernet/sing-tun\tv0.9.7-0.20261009022811-5c2edb183cc9\n\t=>\t./third_party/sing-tun\t(devel)\t\n\t\n\tdep\tgithub.com/sagernet/sing-usbip\tv0.0.0-20260817040617-28bd42667eca\th1:5wA+IE0Fq1CGVLOgSpm0gKKZ03HzugcJ2JyhmDqeX6A=\n\tdep\tgithub.com/sagernet/sing-vmess\tv0.2.9-0.20260929152519-9b95ab8c9478\th1:q2eQn4nq8oWGxRm9zXesXSlPr9GEgPrOGzf4iPtzG9A=\n\tdep\tgithub.com/sagernet/smux\tv1.5.50-sing-box-mod.1\th1:XkJcivBC9V4wBjiGXIXZ229aZCU1hzcbp6kSkkyQ478=\n\tdep\tgithub.com/sagernet/tailscale\tv1.102.1-sing-box-1.14-mod.5.0.20260925112514-35e61219dedd\th1:PzEFeBqDaSUdoWE77NptloLTd1sNT1JAiFThgGq5rlo=\n\tdep\tgithub.com/sagernet/wireguard-go\tv0.0.8-0.20260929150556-ca3bc60c4ce7\th1:MiffQUqKBKoAHz7nkHqh4EmjJsz2M8FHeUdPUdnNyvs=\n\tdep\tgithub.com/sagernet/ws\tv0.0.0-20231204124109-acfe8907c854\th1:6uUiZcDRnZSAegryaUGwPC/Fj13JSHwiTftrXhMmYOc=\n\tdep\tgithub.com/smallstep/pkcs7\tv0.1.1\th1:x+rPdt2W088V9Vkjho4KtoggyktZJlMduZAtRHm68LU=\n\tdep\tgithub.com/spf13/cobra\tv1.10.2\th1:DMTTonx5m65Ic0GOoRY2c16WCbHxOOw6xxezuLaBpcU=\n\tdep\tgithub.com/spf13/pflag\tv1.0.10\th1:4EBh2KAYBwaONj6b2Ye1GiHfwjqyROoF4RwYO+vPwFk=\n\tdep\tgithub.com/tailscale/hujson\tv0.0.0-20260302212456-ecc657c15afd\th1:Rf9uhF1+VJ7ZHqxrG8pJ6YacmHvVCmByDmGbAWCc/gA=\n\tdep\tgithub.com/tailscale/netlink\tv1.1.1-0.20240822203006-4d49adab4de7\th1:uFsXVBE9Qr4ZoF094vE6iYTLDl0qCiKzYXlL6UeWObU=\n\tdep\tgithub.com/tailscale/peercred\tv0.0.0-20250107143737-35a0c7bd7edc\th1:24heQPtnFR+yfntqhI3oAu9i27nEojcQ4NuBQOo5ZFA=\n\tdep\tgithub.com/tailscale/web-client-prebuilt\tv0.0.0-20250124233751-d4cd19a26976\th1:UBPHPtv8+nEAy2PD8RyAhOYvau1ek0HDJqLS/Pysi14=\n\tdep\tgithub.com/tidwall/gjson\tv1.18.0\th1:FIDeeyB800efLX89e5a8Y0BNH+LOngJyGrIWxG2FKQY=\n\tdep\tgithub.com/tidwall/match\tv1.1.1\th1:+Ho715JplO36QYgwN9PGYNhgZvoUSc9X2c80KVTi+GA=\n\tdep\tgithub.com/tidwall/pretty\tv1.2.1\th1:qjsOFOWWQl+N3RsoF5/ssm1pHmJJwhjlSbZ51I6wMl4=\n\tdep\tgithub.com/tidwall/sjson\tv1.2.5\th1:kLy8mja+1c9jlljvWTlSazM7cKDRfJuR/bOJhcY5NcY=\n\tdep\tgithub.com/tjfoc/gmsm\tv1.4.1\th1:aMe1GlZb+0bLjn+cKTPEvvn9oUEBlJitaZiiBwsbgho=\n\tdep\tgithub.com/u-root/uio\tv0.0.0-20240224005618-d2acac8f3701\th1:pyC9PaHYZFgEKFdlp3G8RaCKgVpHZnecvArXvPXcFkM=\n\tdep\tgithub.com/vishvananda/netns\tv0.0.5\th1:DfiHV+j8bA32MFM7bfEunvT8IAqQ/NzSJHtcmW5zdEY=\n\tdep\tgithub.com/x448/float16\tv0.8.4\th1:qLwI1I70+NjRFUR3zs1JPUCgaCXSh3SW62uAKT1mSBM=\n\tdep\tgithub.com/youmark/pkcs8\tv0.0.0-20240726163527-a2c0da244d78\th1:ilQV1hzziu+LLM3zUTJ0trRztfwgjqKnBWNtSRkbmwM=\n\tdep\tgithub.com/zeebo/blake3\tv0.2.4\th1:KYQPkhpRtcqh0ssGYcKLG1JYvddkEA8QwCM/yBqhaZI=\n\tdep\tgo.uber.org/multierr\tv1.11.0\th1:blXXJkSxSSfBVBlC76pxqeO+LN3aDfLQo+309xJstO0=\n\tdep\tgo.uber.org/zap\tv1.27.1\th1:08RqriUEv8+ArZRYSTXy1LeBScaMpVSTBhCeaZYfMYc=\n\tdep\tgo.uber.org/zap/exp\tv0.3.0\th1:6JYzdifzYkGmTdRR59oYH+Ng7k49H9qVpWwNSsGJj3U=\n\tdep\tgo4.org/mem\tv0.0.0-20240501181205-ae6ca9944745\th1:Tl++JLUCe4sxGu8cTpDzRLd3tN7US4hOxG5YpKCzkek=\n\tdep\tgo4.org/netipx\tv0.0.0-20231129151722-fdeea329fbba\th1:0b9z3AuHCjxk0x/opv64kcgZLBseWJUpBw5I82+2U4M=\n\tdep\tgolang.org/x/crypto\tv0.54.0\th1:YLIA59K4fiNzHzjnZt2tUJQjQtUWfWbeHBqKtk3eScw=\n\tdep\tgolang.org/x/exp\tv0.0.0-20260410095643-746e56fc9e2f\th1:W3F4c+6OLc6H2lb//N1q4WpJkhzJCK5J6kUi1NTVXfM=\n\tdep\tgolang.org/x/mod\tv0.37.0\th1:vF1DjpVEshcIqoEaauuHebaLk1O1forxjxBaVn884JQ=\n\tdep\tgolang.org/x/net\tv0.57.0\th1:K5+3DljvIuDG9/Jv9rvyMywYNFCQ9RSUY6OOTTkT+tE=\n\tdep\tgolang.org/x/oauth2\tv0.36.0\th1:peZ/1z27fi9hUOFCAZaHyrpWG5lwe0RJEEEeH0ThlIs=\n\tdep\tgolang.org/x/sync\tv0.22.0\th1:SZjpbeLmrCk4xhRSZFNZW5gFUeCeFgjekvI/+gfScek=\n\tdep\tgolang.org/x/sys\tv0.47.0\th1:o7XGOvZQCADBQQ4Y7VNq2dRWQR7JmOUW8Kxx4ZsNgWs=\n\tdep\tgolang.org/x/term\tv0.45.0\th1:NwWyBmoJCbfTHpxrWoZ9C6/VxOf7ic219I8xZZFdrf0=\n\tdep\tgolang.org/x/text\tv0.40.0\th1:Ub2Z6/xjgF1WrYQz2nuITOEegKFtiIy+rieRJ5lHZKs=\n\tdep\tgolang.org/x/time\tv0.15.0\th1:bbrp8t3bGUeFOx08pvsMYRTCVSMk89u4tKbNOZbp88U=\n\tdep\tgolang.zx2c4.com/wireguard/wgctrl\tv0.0.0-20241231184526-a9ab2273dd10\th1:3GDAcqdIg1ozBNLgPy4SLT84nfcBjr6rhGtXYtrkWLU=\n\tdep\tgoogle.golang.org/genproto/googleapis/rpc\tv0.0.0-20251202230838-ff82c1b0f217\th1:gRkg/vSppuSQoDjxyiGfN4Upv/h/DQmIR10ZU8dh4Ww=\n\tdep\tgoogle.golang.org/grpc\tv1.79.1\th1:zGhSi45ODB9/p3VAawt9a+O/MULLl9dpizzNNpq7flY=\n\tdep\tgoogle.golang.org/protobuf\tv1.36.11\th1:fV6ZwhNocDyBLK0dj+fg8ektcVegBBuEolpbTQyBNVE=\n\tdep\tlukechampine.com/blake3\tv1.3.0\th1:sJ3XhFINmHSrYCgl958hscfIa3bw8x4DqMP3u1YvoYE=\n\tdep\tzombiezen.com/go/capnproto2\tv2.18.2+incompatible\th1:v3BD1zbruvffn7zjJUU5Pn8nZAB11bhZSQC4W+YnnKo=\n\tbuild\t-buildmode=exe\n\tbuild\t-compiler=gc\n\tbuild\t-tags=with_gvisor,with_quic,with_dhcp,with_wireguard,with_utls,with_acme,with_clash_api,with_tailscale,with_ccm,with_ocm,with_cloudflared,with_naive_outbound,with_usbip,with_openvpn,with_openconnect,badlinkname,tfogo_checklinkname0,with_purego\n\tbuild\t-trimpath=true\n\tbuild\tCGO_ENABLED=0\n\tbuild\tGOARCH=amd64\n\tbuild\tGOOS=linux\n\tbuild\tGOAMD64=v1\n\tbuild\tvcs=git\n\tbuild\tvcs.revision=aafc521b745e0d1a8f50a6a3f41169433f1c2194\n\tbuild\tvcs.time=2026-10-10T20:06:56Z\n\tbuild\tvcs.modified=false\n","schema":"polaris-dns-technical-native-v1","BuildID":"polaris-dns-technical-v1-40740da714acef3da127a116c245b820f268e45c706f44077055e888e7db30f3","AppAdmissible":false,"publicationEligible":false,"receiptSha256":"3fe119b0fe707526a6f6a926b1bfacc90dac9900bae102509fc604742bbd181f"},{"key":"win","raw":"D:\\a\\_temp\\polaris-dns-native\\polaris-box-1.15.0-alpha.11.polaris.candidate-aafc521b-windows-amd64\\sing-box.exe: go1.25.5\r\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\r\n\tmod\tgithub.com/sagernet/sing-box\tv0.0.0-20261010200656-aafc521b745e\t\r\n\tdep\tfilippo.io/edwards25519\tv1.2.0\th1:crnVqOiS4jqYleHd9vaKZ+HKtHfllngJIiOpNpoJsjo=\r\n\tdep\tgithub.com/RyuaNerin/go-krypto\tv1.3.0\th1:smavTzSMAx8iuVlGb4pEwl9MD2qicqMzuXR2QWp2/Pg=\r\n\tdep\tgithub.com/ajg/form\tv1.5.1\th1:t9c7v8JUKu/XxOGBU0yjNpaMloxGEJhUkqFRq0ibGeU=\r\n\tdep\tgithub.com/alexbrainman/sspi\tv0.0.0-20231016080023-1a75b4708caa\th1:LHTHcTQiSGT7VVbI0o4wBRNQIgn917usHWOd6VAffYI=\r\n\tdep\tgithub.com/anchore/go-lzo\tv0.1.0\th1:NgAacnzqPeGH49Ky19QKLBZEuFRqtTG9cdaucc3Vncs=\r\n\tdep\tgithub.com/andybalholm/brotli\tv1.1.0\th1:eLKJA0d02Lf0mVpIDgYnqXcUn0GqVmEFny3VuID1U3M=\r\n\tdep\tgithub.com/anmitsu/go-shlex\tv0.0.0-20200514113438-38f4b401e2be\th1:9AeTilPcZAjCFIImctFaOjnTIavg87rW78vTPkQqLI8=\r\n\tdep\tgithub.com/anthropics/anthropic-sdk-go\tv1.26.0\th1:oUTzFaUpAevfuELAP1sjL6CQJ9HHAfT7CoSYSac11PY=\r\n\tdep\tgithub.com/axiomhq/hyperloglog\tv0.0.0-20240319100328-84253e514e02\th1:bXAPYSbdYbS5VTy92NIUbeDI1qyggi+JYh5op9IFlcQ=\r\n\tdep\tgithub.com/caddyserver/certmagic\tv0.25.3-0.20260421143802-60d9d8b415d6\th1:LYSB6VgWzKtNrcxElw3c97BP40Oc7bizKxA9K1Vi/5k=\r\n\tdep\tgithub.com/caddyserver/zerossl\tv0.1.5\th1:dkvOjBAEEtY6LIGAHei7sw2UgqSD6TrWweXpV7lvEvE=\r\n\tdep\tgithub.com/clipperhouse/uax29/v2\tv2.2.0\th1:ChwIKnQN3kcZteTXMgb1wztSgaU+ZemkgWdohwgs8tY=\r\n\tdep\tgithub.com/coder/websocket\tv1.8.14\th1:9L0p0iKiNOibykf283eHkKUHHrpG7f65OE3BhhO7v9g=\r\n\tdep\tgithub.com/coreos/go-oidc/v3\tv3.17.0\th1:hWBGaQfbi0iVviX4ibC7bk8OKT5qNr4klBaCHVNvehc=\r\n\tdep\tgithub.com/cretz/bine\tv0.2.0\th1:8GiDRGlTgz+o8H9DSnsl+5MeBK4HsExxgl6WgzOCuZo=\r\n\tdep\tgithub.com/database64128/netx-go\tv0.1.1\th1:dT5LG7Gs7zFZBthFBbzWE6K8wAHjSNAaK7wCYZT7NzM=\r\n\tdep\tgithub.com/database64128/tfo-go/v2\tv2.3.2\th1:UhZMKiMq3swZGUiETkLBDzQnZBPSAeBMClpJGlnJ5Fw=\r\n\tdep\tgithub.com/dblohm7/wingoes\tv0.0.0-20240119213807-a09d6be7affa\th1:h8TfIT1xc8FWbwwpmHn1J5i43Y0uZP97GqasGCzSRJk=\r\n\tdep\tgithub.com/dgryski/go-camellia\tv0.0.0-20191119043421-69a8a13fb23d\th1:CPqTNIigGweVPT4CYb+OO2E6XyRKFOmvTHwWRLgCAlE=\r\n\tdep\tgithub.com/dgryski/go-metro\tv0.0.0-20180109044635-280f6062b5bc\th1:8WFBn63wegobsYAX0YjD+8suexZDga5CctH4CCTx2+8=\r\n\tdep\tgithub.com/ebitengine/purego\tv0.10.0\th1:QIw4xfpWT6GWTzaW5XEKy3HXoqrJGx1ijYHzTF0/ISU=\r\n\tdep\tgithub.com/fsnotify/fsnotify\tv1.9.0\th1:2Ml+OJNzbYCTzsxtv8vKSFD9PbJjmhYF14k/jKC7S9k=\r\n\tdep\tgithub.com/fxamacker/cbor/v2\tv2.9.0\th1:NpKPmjDBgUfBms6tr6JZkTHtfFGcMKsw3eGcmD/sapM=\r\n\tdep\tgithub.com/gaissmai/bart\tv0.26.1\th1:+w4rnLGNlA2GDVn382Tfe3jOsK5vOr5n4KmigJ9lbTo=\r\n\tdep\tgithub.com/go-chi/chi/v5\tv5.2.5\th1:Eg4myHZBjyvJmAFjFvWgrqDTXFyOzjj7YIm3L3mu6Ug=\r\n\tdep\tgithub.com/go-chi/render\tv1.0.3\th1:AsXqd2a1/INaIfUSKq3G5uA8weYx20FOsM7uSoCyyt4=\r\n\tdep\tgithub.com/go-jose/go-jose/v4\tv4.1.3\th1:CVLmWDhDVRa6Mi/IgCgaopNosCaHz7zrMeF9MlZRkrs=\r\n\tdep\tgithub.com/go-ole/go-ole\tv1.3.0\th1:Dt6ye7+vXGIKZ7Xtk4s6/xVdGDQynvom7xCFEdWr6uE=\r\n\tdep\tgithub.com/go4org/hashtriemap\tv0.0.0-20251130024219-545ba229f689\th1:0psnKZ+N2IP43/SZC8SKx6OpFJwLmQb9m9QyV9BC2f8=\r\n\tdep\tgithub.com/gobwas/httphead\tv0.1.0\th1:exrUm0f4YX0L7EBwZHuCF4GDp8aJfVeBrlLQrs6NqWU=\r\n\tdep\tgithub.com/gobwas/pool\tv0.2.1\th1:xfeeEhW7pwmX8nuLVlqbzVc7udMDrwetjEv+TZIz1og=\r\n\tdep\tgithub.com/gofrs/uuid/v5\tv5.5.1\th1:z1Ce19/JwNidXpy3tOQc3241lnJLKdKyq/xlNvlD4Ng=\r\n\tdep\tgithub.com/golang/groupcache\tv0.0.0-20241129210726-2c02b8208cf8\th1:f+oWsMOmNPc8JmEHVZIycC7hBoQxHH9pNKQORJNozsQ=\r\n\tdep\tgithub.com/google/btree\tv1.1.3\th1:CVpQJjYgC4VbzxeGVHfvZrv1ctoYCAI8vbl07Fcxlyg=\r\n\tdep\tgithub.com/google/certificate-transparency-go\tv1.3.2\th1:9ahSNZF2o7SYMaKaXhAumVEzXB2QaayzII9C8rv7v+A=\r\n\tdep\tgithub.com/google/gopacket\tv1.1.19\th1:ves8RnFZPGiFnTS0uPQStjwru6uO6h+nlr9j6fL7kF8=\r\n\tdep\tgithub.com/google/uuid\tv1.6.0\th1:NIvaJDMOsjHA8n1jAhLSgzrAzy1Hgr+hNrb57e+94F0=\r\n\tdep\tgithub.com/hashicorp/yamux\tv0.1.2\th1:XtB8kyFOyHXYVFnwT5C3+Bdo8gArse7j2AQ0DA0Uey8=\r\n\tdep\tgithub.com/hdevalence/ed25519consensus\tv0.2.0\th1:37ICyZqdyj0lAZ8P4D1d1id3HqbbG1N3iBb1Tb4rdcU=\r\n\tdep\tgithub.com/huin/goupnp\tv1.3.0\th1:UvLUlWDNpoUdYzb2TCn+MuTWtcjXKSza2n6CBdQ0xXc=\r\n\tdep\tgithub.com/inconshreveable/mousetrap\tv1.1.0\th1:wN+x4NVGpMsO7ErUn/mUI3vEoE6Jt13X2s0bqwp9tc8=\r\n\tdep\tgithub.com/insomniacslk/dhcp\tv0.0.0-20260220084031-5adc3eb26f91\th1:u9i04mGE3iliBh0EFuWaKsmcwrLacqGmq1G3XoaM7gY=\r\n\tdep\tgithub.com/jackpal/go-nat-pmp\tv1.0.2\th1:KzKSgb7qkJvOUTqYl9/Hg/me3pWgBmERKrTGD7BdWus=\r\n\tdep\tgithub.com/klauspost/compress\tv1.19.1\th1:VsB4HPswih7mmZ8WleSFQ75c/Ui1M4trX5oAsJnhSlk=\r\n\tdep\tgithub.com/klauspost/cpuid/v2\tv2.3.0\th1:S4CRMLnYUhGeDFDqkGriYKdfoFlDnMtqTiI/sFzhA9Y=\r\n\tdep\tgithub.com/koron/go-ssdp\tv0.0.4\th1:1IDwrghSKYM7yLf7XCzbByg2sJ/JcNOZRXS2jczTwz0=\r\n\tdep\tgithub.com/kr/fs\tv0.1.0\th1:Jskdu9ieNAYnjxsi0LbQp1ulIKZV1LAFgK1tWhpZgl8=\r\n\tdep\tgithub.com/libdns/acmedns\tv0.5.0\th1:5pRtmUj4Lb/QkNJSl1xgOGBUJTWW7RjpNaIhjpDXjPE=\r\n\tdep\tgithub.com/libdns/alidns\tv1.0.6\th1:/Ii428ty6WHFJmE24rZxq2taq++gh7rf9jhgLfp8PmM=\r\n\tdep\tgithub.com/libdns/cloudflare\tv0.2.2\th1:XWHv+C1dDcApqazlh08Q6pjytYLgR2a+Y3xrXFu0vsI=\r\n\tdep\tgithub.com/libdns/libdns\tv1.1.1\th1:wPrHrXILoSHKWJKGd0EiAVmiJbFShguILTg9leS/P/U=\r\n\tdep\tgithub.com/libp2p/go-nat\tv1.0.1-0.20250821073202-01afc089f138\th1:YohuNPT/1k3VcThCQlBZ43PCPWPfMRS1zcxWBF2SLK8=\r\n\tdep\tgithub.com/libp2p/go-netroute\tv0.2.1\th1:V8kVrpD8GK0Riv15/7VN6RbUQ3URNZVosw7H2v9tksU=\r\n\tdep\tgithub.com/logrusorgru/aurora\tv2.0.3+incompatible\th1:tOpm7WcpBTn4fjmVfgpQq0EfczGlG91VSDkswnjF5A8=\r\n\tdep\tgithub.com/mattn/go-runewidth\tv0.0.27\th1:Feg/Oou5zI/wnpgDF6omIU0OokC9GxLC/WRknhVlIR0=\r\n\tdep\tgithub.com/metacubex/utls\tv1.8.7\th1:Cp+yWkNTFkSihETgGWq34hlVFds5HpYWVOR1xovUVTs=\r\n\tdep\tgithub.com/mholt/acmez/v3\tv3.1.6\th1:eGVQNObP0pBN4sxqrXeg7MYqTOWyoiYpQqITVWlrevk=\r\n\tdep\tgithub.com/miekg/dns\tv1.1.72\th1:vhmr+TF2A3tuoGNkLDFK9zi36F2LS+hKTRW0Uf8kbzI=\r\n\tdep\tgithub.com/mitchellh/go-ps\tv1.0.0\th1:i6ampVEEF4wQFF+bkYfwYgY+F/uYJDktmvLPf7qIgjc=\r\n\tdep\tgithub.com/openai/openai-go/v3\tv3.26.0\th1:bRt6H/ozMNt/dDkN4gobnLqaEGrRGBzmbVs0xxJEnQE=\r\n\tdep\tgithub.com/oschwald/maxminddb-golang\tv1.13.1\th1:G3wwjdN9JmIK2o/ermkHM+98oX5fS+k5MbwsmL4MRQE=\r\n\tdep\tgithub.com/pierrec/lz4/v4\tv4.1.26\th1:GrpZw1gZttORinvzBdXPUXATeqlJjqUG/D87TKMnhjY=\r\n\tdep\tgithub.com/pion/dtls/v3\tv3.1.5\th1:9xJtVsHwMYeSjPp5Hh1FTis4DchnQWtnOa5o+6ygqfc=\r\n\tdep\tgithub.com/pion/logging\tv0.2.4\th1:tTew+7cmQ+Mc1pTBLKH2puKsOvhm32dROumOZ655zB8=\r\n\tdep\tgithub.com/pion/transport/v4\tv4.0.2\th1:ifYlPqNwsy6aKQ9y8yzxXlHae5431ZrH2avkD/Rn6Tk=\r\n\tdep\tgithub.com/pires/go-proxyproto\tv0.8.1\th1:9KEixbdJfhrbtjpz/ZwCdWDD2Xem0NZ38qMYaASJgp0=\r\n\tdep\tgithub.com/pkg/sftp\tv1.13.10\th1:+5FbKNTe5Z9aspU88DPIKJ9z2KZoaGCu6Sr6kKR/5mU=\r\n\tdep\tgithub.com/quic-go/qpack\tv0.6.0\th1:g7W+BMYynC1LbYLSqRt8PBg5Tgwxn214ZZR34VIOjz8=\r\n\tdep\tgithub.com/sagernet/bbolt\tv0.0.0-20260915102804-500ee1e84832\th1:KDXMGbzqLmDdKcld0T3wuMuqf1iezZq+DemgtZ/vhtQ=\r\n\tdep\tgithub.com/sagernet/cors\tv1.2.1\th1:Cv5Z8y9YSD6Gm+qSpNrL3LO4lD3eQVvbFYJSG7JCMHQ=\r\n\tdep\tgithub.com/sagernet/cronet-go\tv0.0.0-20260929213745-4d18a60dc3a8\th1:yW/KNhSxWxllsPjLx5EUPohHLa4Kvzl2Qt9kHkB0ZIc=\r\n\tdep\tgithub.com/sagernet/cronet-go/all\tv0.0.0-20260929213745-4d18a60dc3a8\th1:xhh9UrPFncqI8r2mr+bybgZyJICtV4GKOSJuhnEx75g=\r\n\tdep\tgithub.com/sagernet/cronet-go/lib/windows_amd64\tv0.0.0-20260929213014-a1cafd93eb1f\th1:0Z4FyS1oD2Cl/Shp10UYHki8x/2VxCRGbYJi09pWGPU=\r\n\tdep\tgithub.com/sagernet/fswatch\tv0.1.2\th1:/TT7k4mkce1qFPxamLO842WjqBgbTBiXP2mlUjp9PFk=\r\n\tdep\tgithub.com/sagernet/gliderssh\tv0.3.4-0.20260531100337-2194faca5648\th1:IWVjKBARzVjdmH0VUaeTBOBli1qkwKmTG4XfbkpSS20=\r\n\tdep\tgithub.com/sagernet/gvisor\tv0.0.0-20260727.0-sing-box-mod.1\th1:IdQ7yTKkB2wv8txwshxUroPlO4npOYAV71xb7xQ7Lys=\r\n\tdep\tgithub.com/sagernet/quic-go\tv0.61.0-sing-box-mod.9\th1:Lojx77uC63TxZO38Z1OSo6aOoBSXY+ueH0/IKKeTZuI=\r\n\tdep\tgithub.com/sagernet/sing\tv0.9.7-0.20260929150544-6f21f2425a95\th1:taM2SOI5HK/Ubpi3FO0Bc7bKHsFVdGe4YS/jjXD4kAM=\r\n\tdep\tgithub.com/sagernet/sing-anytls\tv0.0.0-20260928104022-580984e4d8cb\th1:n4sF2zv0CR1gcb4awh61t11t0FZmcP40x5enB8HARjE=\r\n\tdep\tgithub.com/sagernet/sing-cloudflared\tv0.1.4-0.20261002084126-c1255ae368f2\th1:+AfbAXGEV8tezIYHeShzwHm7IMUE5kYGVFLo0ly7W24=\r\n\tdep\tgithub.com/sagernet/sing-mux\tv0.3.10-0.20260929204512-caf09fe32475\th1:aG0DqHEOjax7DyfE1GTAXZotOg3QGVCz3/8rCSua2kg=\r\n\tdep\tgithub.com/sagernet/sing-openconnect\tv0.1.1-0.20260929151226-29757281a247\th1:Ui4XtxNgQiEquK+ch4dYhthKsVlGQpdCdKIpOuJVyog=\r\n\tdep\tgithub.com/sagernet/sing-openvpn\tv0.1.1-0.20260929151220-f330676d6d4a\th1:Q16Twlwk9fssoCSUTDo5WqPg0EZHG9JSV9SC3zRy9M0=\r\n\tdep\tgithub.com/sagernet/sing-quic\tv0.7.2-0.20261002084117-75c3ac4fa12b\th1:2Jx/URJAPt32MEbLmAR5nzH87tO6NQZLRfRFvFqBWuk=\r\n\tdep\tgithub.com/sagernet/sing-shadowsocks\tv0.2.9-0.20260929204512-65740e0f0e3e\th1:LKgE/uuqp/D2kgxYViOz8vf4slaCUfpFywXjtuwD5gU=\r\n\tdep\tgithub.com/sagernet/sing-shadowsocks2\tv0.2.2-0.20260929152114-a69d1086332b\th1:VH65QNqSHat2b+N3Rlzo5Qe44gRmQGwzY5atuy3D0A8=\r\n\tdep\tgithub.com/sagernet/sing-shadowtls\tv0.2.2-0.20260928201441-a9c0127d5c99\th1:l/tdZjaoDp3iqEG4NVrFHtphy1+Qjt9bpWAgNbF8iGg=\r\n\tdep\tgithub.com/sagernet/sing-snell\tv0.0.0-20260904135315-bc5a12ac736f\th1:tbTCI+iis5sTmua7a6Q7vebcSRURok/LGgnEafnQAsc=\r\n\tdep\tgithub.com/sagernet/sing-tun\tv0.9.7-0.20261009022811-5c2edb183cc9\r\n\t=>\t./third_party/sing-tun\t(devel)\t\r\n\t\r\n\tdep\tgithub.com/sagernet/sing-usbip\tv0.0.0-20260817040617-28bd42667eca\th1:5wA+IE0Fq1CGVLOgSpm0gKKZ03HzugcJ2JyhmDqeX6A=\r\n\tdep\tgithub.com/sagernet/sing-vmess\tv0.2.9-0.20260929152519-9b95ab8c9478\th1:q2eQn4nq8oWGxRm9zXesXSlPr9GEgPrOGzf4iPtzG9A=\r\n\tdep\tgithub.com/sagernet/smux\tv1.5.50-sing-box-mod.1\th1:XkJcivBC9V4wBjiGXIXZ229aZCU1hzcbp6kSkkyQ478=\r\n\tdep\tgithub.com/sagernet/tailscale\tv1.102.1-sing-box-1.14-mod.5.0.20260925112514-35e61219dedd\th1:PzEFeBqDaSUdoWE77NptloLTd1sNT1JAiFThgGq5rlo=\r\n\tdep\tgithub.com/sagernet/wireguard-go\tv0.0.8-0.20260929150556-ca3bc60c4ce7\th1:MiffQUqKBKoAHz7nkHqh4EmjJsz2M8FHeUdPUdnNyvs=\r\n\tdep\tgithub.com/sagernet/ws\tv0.0.0-20231204124109-acfe8907c854\th1:6uUiZcDRnZSAegryaUGwPC/Fj13JSHwiTftrXhMmYOc=\r\n\tdep\tgithub.com/smallstep/pkcs7\tv0.1.1\th1:x+rPdt2W088V9Vkjho4KtoggyktZJlMduZAtRHm68LU=\r\n\tdep\tgithub.com/spf13/cobra\tv1.10.2\th1:DMTTonx5m65Ic0GOoRY2c16WCbHxOOw6xxezuLaBpcU=\r\n\tdep\tgithub.com/spf13/pflag\tv1.0.10\th1:4EBh2KAYBwaONj6b2Ye1GiHfwjqyROoF4RwYO+vPwFk=\r\n\tdep\tgithub.com/tailscale/certstore\tv0.1.1-0.20260409135935-3638fb84b77d\th1:JcGKBZAL7ePLwOhUdN8qGQZlP5GueEiIZwY7R62pejE=\r\n\tdep\tgithub.com/tailscale/go-winio\tv0.0.0-20231025203758-c4f33415bf55\th1:Gzfnfk2TWrk8Jj4P4c1a3CtQyMaTVCznlkLZI++hok4=\r\n\tdep\tgithub.com/tailscale/hujson\tv0.0.0-20260302212456-ecc657c15afd\th1:Rf9uhF1+VJ7ZHqxrG8pJ6YacmHvVCmByDmGbAWCc/gA=\r\n\tdep\tgithub.com/tailscale/web-client-prebuilt\tv0.0.0-20250124233751-d4cd19a26976\th1:UBPHPtv8+nEAy2PD8RyAhOYvau1ek0HDJqLS/Pysi14=\r\n\tdep\tgithub.com/tidwall/gjson\tv1.18.0\th1:FIDeeyB800efLX89e5a8Y0BNH+LOngJyGrIWxG2FKQY=\r\n\tdep\tgithub.com/tidwall/match\tv1.1.1\th1:+Ho715JplO36QYgwN9PGYNhgZvoUSc9X2c80KVTi+GA=\r\n\tdep\tgithub.com/tidwall/pretty\tv1.2.1\th1:qjsOFOWWQl+N3RsoF5/ssm1pHmJJwhjlSbZ51I6wMl4=\r\n\tdep\tgithub.com/tidwall/sjson\tv1.2.5\th1:kLy8mja+1c9jlljvWTlSazM7cKDRfJuR/bOJhcY5NcY=\r\n\tdep\tgithub.com/tjfoc/gmsm\tv1.4.1\th1:aMe1GlZb+0bLjn+cKTPEvvn9oUEBlJitaZiiBwsbgho=\r\n\tdep\tgithub.com/u-root/uio\tv0.0.0-20240224005618-d2acac8f3701\th1:pyC9PaHYZFgEKFdlp3G8RaCKgVpHZnecvArXvPXcFkM=\r\n\tdep\tgithub.com/vishvananda/netns\tv0.0.5\th1:DfiHV+j8bA32MFM7bfEunvT8IAqQ/NzSJHtcmW5zdEY=\r\n\tdep\tgithub.com/x448/float16\tv0.8.4\th1:qLwI1I70+NjRFUR3zs1JPUCgaCXSh3SW62uAKT1mSBM=\r\n\tdep\tgithub.com/youmark/pkcs8\tv0.0.0-20240726163527-a2c0da244d78\th1:ilQV1hzziu+LLM3zUTJ0trRztfwgjqKnBWNtSRkbmwM=\r\n\tdep\tgithub.com/zeebo/blake3\tv0.2.4\th1:KYQPkhpRtcqh0ssGYcKLG1JYvddkEA8QwCM/yBqhaZI=\r\n\tdep\tgo.uber.org/multierr\tv1.11.0\th1:blXXJkSxSSfBVBlC76pxqeO+LN3aDfLQo+309xJstO0=\r\n\tdep\tgo.uber.org/zap\tv1.27.1\th1:08RqriUEv8+ArZRYSTXy1LeBScaMpVSTBhCeaZYfMYc=\r\n\tdep\tgo.uber.org/zap/exp\tv0.3.0\th1:6JYzdifzYkGmTdRR59oYH+Ng7k49H9qVpWwNSsGJj3U=\r\n\tdep\tgo4.org/mem\tv0.0.0-20240501181205-ae6ca9944745\th1:Tl++JLUCe4sxGu8cTpDzRLd3tN7US4hOxG5YpKCzkek=\r\n\tdep\tgo4.org/netipx\tv0.0.0-20231129151722-fdeea329fbba\th1:0b9z3AuHCjxk0x/opv64kcgZLBseWJUpBw5I82+2U4M=\r\n\tdep\tgolang.org/x/crypto\tv0.54.0\th1:YLIA59K4fiNzHzjnZt2tUJQjQtUWfWbeHBqKtk3eScw=\r\n\tdep\tgolang.org/x/exp\tv0.0.0-20260410095643-746e56fc9e2f\th1:W3F4c+6OLc6H2lb//N1q4WpJkhzJCK5J6kUi1NTVXfM=\r\n\tdep\tgolang.org/x/mod\tv0.37.0\th1:vF1DjpVEshcIqoEaauuHebaLk1O1forxjxBaVn884JQ=\r\n\tdep\tgolang.org/x/net\tv0.57.0\th1:K5+3DljvIuDG9/Jv9rvyMywYNFCQ9RSUY6OOTTkT+tE=\r\n\tdep\tgolang.org/x/oauth2\tv0.36.0\th1:peZ/1z27fi9hUOFCAZaHyrpWG5lwe0RJEEEeH0ThlIs=\r\n\tdep\tgolang.org/x/sync\tv0.22.0\th1:SZjpbeLmrCk4xhRSZFNZW5gFUeCeFgjekvI/+gfScek=\r\n\tdep\tgolang.org/x/sys\tv0.47.0\th1:o7XGOvZQCADBQQ4Y7VNq2dRWQR7JmOUW8Kxx4ZsNgWs=\r\n\tdep\tgolang.org/x/term\tv0.45.0\th1:NwWyBmoJCbfTHpxrWoZ9C6/VxOf7ic219I8xZZFdrf0=\r\n\tdep\tgolang.org/x/text\tv0.40.0\th1:Ub2Z6/xjgF1WrYQz2nuITOEegKFtiIy+rieRJ5lHZKs=\r\n\tdep\tgolang.org/x/time\tv0.15.0\th1:bbrp8t3bGUeFOx08pvsMYRTCVSMk89u4tKbNOZbp88U=\r\n\tdep\tgolang.zx2c4.com/wintun\tv0.0.0-20230126152724-0fa3db229ce2\th1:B82qJJgjvYKsXS9jeunTOisW56dUokqW/FOteYJJ/yg=\r\n\tdep\tgolang.zx2c4.com/wireguard/wgctrl\tv0.0.0-20241231184526-a9ab2273dd10\th1:3GDAcqdIg1ozBNLgPy4SLT84nfcBjr6rhGtXYtrkWLU=\r\n\tdep\tgolang.zx2c4.com/wireguard/windows\tv0.5.3\th1:On6j2Rpn3OEMXqBq00QEDC7bWSZrPIHKIus8eIuExIE=\r\n\tdep\tgoogle.golang.org/genproto/googleapis/rpc\tv0.0.0-20251202230838-ff82c1b0f217\th1:gRkg/vSppuSQoDjxyiGfN4Upv/h/DQmIR10ZU8dh4Ww=\r\n\tdep\tgoogle.golang.org/grpc\tv1.79.1\th1:zGhSi45ODB9/p3VAawt9a+O/MULLl9dpizzNNpq7flY=\r\n\tdep\tgoogle.golang.org/protobuf\tv1.36.11\th1:fV6ZwhNocDyBLK0dj+fg8ektcVegBBuEolpbTQyBNVE=\r\n\tdep\tlukechampine.com/blake3\tv1.3.0\th1:sJ3XhFINmHSrYCgl958hscfIa3bw8x4DqMP3u1YvoYE=\r\n\tdep\tzombiezen.com/go/capnproto2\tv2.18.2+incompatible\th1:v3BD1zbruvffn7zjJUU5Pn8nZAB11bhZSQC4W+YnnKo=\r\n\tbuild\t-buildmode=exe\r\n\tbuild\t-compiler=gc\r\n\tbuild\t-tags=with_gvisor,with_quic,with_dhcp,with_wireguard,with_utls,with_acme,with_clash_api,with_tailscale,with_ccm,with_ocm,with_cloudflared,with_naive_outbound,with_purego,with_usbip,with_openvpn,with_openconnect,badlinkname,tfogo_checklinkname0\r\n\tbuild\t-trimpath=true\r\n\tbuild\tCGO_ENABLED=0\r\n\tbuild\tGOARCH=amd64\r\n\tbuild\tGOOS=windows\r\n\tbuild\tGOAMD64=v1\r\n\tbuild\tvcs=git\r\n\tbuild\tvcs.revision=aafc521b745e0d1a8f50a6a3f41169433f1c2194\r\n\tbuild\tvcs.time=2026-10-10T20:06:56Z\r\n\tbuild\tvcs.modified=false\r\n","schema":"polaris-dns-technical-native-v1","BuildID":"polaris-dns-technical-v1-1d26cd15720921b8e90fac5f4a32449b81b939e75dd64c0e94d6a9293edb112d","AppAdmissible":false,"publicationEligible":false,"receiptSha256":"00f418b42957db3e0728ab37fe8c0fa3d8197cd94c8c1d7298473e0682b921b4"}];
test('four original technical native metadata fixtures stay production-inadmissible', () => {
  const source = json(join(repo, 'scripts/desktop-core/source-manifest.json'));
  const spec = json(join(repo, 'src-tauri/core-manifest.json')).desktopSourceBuild;
  for (const row of actualTechnicalMetadata) {
    assert.equal(row.AppAdmissible, false); assert.equal(row.publicationEligible, false);
    assert.match(row.BuildID, /^polaris-dns-technical-v1-/);
    // Matching module/VCS facts do not turn a technical artifact into an App receipt.
    if (sourceOwnedReplacements(spec)) {
      assert.doesNotThrow(() => validateBuildInfo(row.raw, source, row.key, spec));
      const identity = platformSourceIdentity({ fingerprint: spec.sourceReceiptFingerprint }, source, spec, row.key, null);
      assert.notEqual(row.BuildID, identity.buildID);
    } else assert.throws(() => validateBuildInfo(row.raw, source, row.key, spec), /Undeclared embedded module replacement/);
    assert.throws(() => validateSourceReceipt({ schema: row.schema }, source, spec), /Unsupported source receipt schema/);
  }
});
