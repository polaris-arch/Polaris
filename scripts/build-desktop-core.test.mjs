// Stub-runner cases prove consumer logic only. The opt-in final case uses a
// tiny real local Go build; neither case is a sing-box/native-platform verdict.
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { buildDesktopCore, produceDesktopCore } from './desktop-core/build-core.mjs';
import { consumeDesktopBundle, coreFilename, verifyPackagedSource, writeBundleInventory } from './desktop-core/bundle.mjs';
import { buildInfoFingerprint, canonical, DESKTOP_TARGETS, digest, expectedTags, frozenSourceVersion, platformSourceIdentity,
  validateBuildInfo, validateSourcePins, validateSourceReceipt } from './desktop-core/source-graph.mjs';
import { classifyImpact, androidRegistrationOf } from './classify-ci-impact.mjs';

const repo = join(dirname(fileURLToPath(import.meta.url)), '..');
const candidate = 'c'.repeat(40);
const tree = 'd'.repeat(40);
const windowsTree = 'e'.repeat(40);
const TUN_MODULE = 'github.com/sagernet/sing-tun';
const NFT_MODULE = 'github.com/sagernet/nftables';
const PATCHED_MODULES = ['example.com/tiny', TUN_MODULE, NFT_MODULE];
const TRANSPORT_MODULES = ['example.com/transport', 'example.com/linux-transport'];
const platformPolicies = (key) => ({
  patchedModules: { requiredLinked: PATCHED_MODULES.filter((module) => key === 'linux' || module !== NFT_MODULE),
    allowedAbsent: key === 'linux' ? [] : [NFT_MODULE] },
  transportModules: { requiredLinked: TRANSPORT_MODULES.filter((module) => key === 'linux' || module !== 'example.com/linux-transport'),
    confirmedAbsent: key === 'linux' ? [] : ['example.com/linux-transport'] },
});
const write = (path, bytes) => { mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, bytes); };
const signed = (facts) => ({ ...facts, fingerprint: digest(canonical(facts)) });
function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'polaris-graph-logic-'));
  const patch = 'synthetic core patch';
  const depPatch = 'synthetic dependency patch';
  const overlay = 'synthetic Windows overlay';
  const provider = 'synthetic provider; never executed';
  const source = { sourceCommit: 'a'.repeat(40), goVersion: '1.25.5',
    patches: [{ file: 'core.patch', sha256: digest(patch) }], dependencyPatches: PATCHED_MODULES.map((module, index) => {
      const name = ['tiny', 'sing-tun', 'nftables'][index];
      return { name, module, upstreamVersion: 'v1.0.0',
        upstreamCommit: 'b'.repeat(40), sourceURL: `https://github.com/example/${name}`,
        patchFile: `${name}.patch`, patchSha256: digest(depPatch), patchedTree: 'f'.repeat(40) };
    }) };
  const manifestBytes = JSON.stringify(source);
  const graph = source.dependencyPatches.map((dep) => ({ Path: dep.module, Version: dep.upstreamVersion,
    Replace: { Path: `./polaris-dependencies/${dep.name}` } }));
  const receipt = signed({ schema: 'polaris-core-source-v1', sourceCommit: source.sourceCommit, sourceURL: 'https://github.com/SagerNet/sing-box',
    graphScope: 'declared-patched-modules', sourceGraphState: 'dependencies-patched', moduleGraphQueries: [...PATCHED_MODULES].sort(),
    mainGoModSha256: digest('synthetic full main go.mod'), mainGoSumSha256: digest('synthetic full main go.sum'),
    upstreamTree: '1'.repeat(40), patchedSourceTree: '2'.repeat(40), buildTree: tree,
    sourceManifestSha256: digest(manifestBytes), provisionerSha256: digest(provider), patches: source.patches,
    dependencies: source.dependencyPatches.map((dep) => ({ ...dep, upstreamTree: '3'.repeat(40), replacement: `./polaris-dependencies/${dep.name}` })),
    moduleGraph: graph, moduleGraphSha256: digest(canonical(graph)) });
  const manifest = { bundledCoreVersion: '1.0.0', sourceBuild: {
    sourceManifestSha256: receipt.sourceManifestSha256, provisionerSha256: receipt.provisionerSha256,
    sourceReceiptFingerprint: receipt.fingerprint, moduleGraphSha256: receipt.moduleGraphSha256,
    patchedSourceTree: receipt.patchedSourceTree, buildTree: tree, version: '1.0.0.polaris.1',
    dependencyModules: PATCHED_MODULES, transportPins: { 'example.com/transport': 'v0.47.0', 'example.com/linux-transport': 'v1.2.0' },
    platforms: Object.fromEntries(Object.keys(DESKTOP_TARGETS)
      .map((key) => [key, { buildTree: key === 'win' ? windowsTree : tree, ...platformPolicies(key), binarySha256: null }])) },
  windowsBuild: { sourceCommit: source.sourceCommit, goVersion: source.goVersion, version: '1.0.0.polaris.1', patchSha256: digest(overlay) } };
  write(join(root, 'scripts/libbox-patches/source-manifest.json'), manifestBytes);
  write(join(root, 'scripts/libbox-patches/core.patch'), patch);
  for (const dep of source.dependencyPatches) write(join(root, 'scripts/libbox-patches', dep.patchFile), depPatch);
  write(join(root, 'scripts/core-source-provision.py'), provider);
  write(join(root, 'scripts/core-patches/windows-dns-refresh.patch'), overlay);
  return { root, manifest, source, receipt, dispose: () => rmSync(root, { recursive: true, force: true }) };
}
function metadata(key, path = '/synthetic/core', { nft = key === 'linux', linuxTransport = key === 'linux' } = {}) {
  const target = DESKTOP_TARGETS[key];
  return `${path}: go1.25.5\n\tpath\texample.com/probe\n\tmod\texample.com/probe\t(devel)\t\n`
    + '\tdep\texample.com/tiny\tv1.0.0\t\n\t=>\t./polaris-dependencies/tiny\t(devel)\t\n\t\n'
    + '\tdep\tgithub.com/sagernet/sing-tun\tv1.0.0\t\n\t=>\t./polaris-dependencies/sing-tun\t(devel)\t\n\t\n'
    + (nft ? '\tdep\tgithub.com/sagernet/nftables\tv1.0.0\t\n\t=>\t./polaris-dependencies/nftables\t(devel)\t\n\t\n' : '')
    + '\tdep\texample.com/transport\tv0.47.0\th1:synthetic\n'
    + (linuxTransport ? '\tdep\texample.com/linux-transport\tv1.2.0\th1:synthetic-linux\n' : '')
    + `\tbuild\tGOOS=${target.goos}\n\tbuild\tGOARCH=${target.goarch}\n\tbuild\tCGO_ENABLED=${target.cgo}\n`
    + `\tbuild\t-tags=${expectedTags(key).join(',')}\n`;
}
function stub(f, key, change = {}) {
  const calls = [];
  const fetched = new Map();
  const run = (command, args, options) => {
    calls.push({ command, args, options });
    if (command === 'git') {
      if (args[0] === 'rev-parse') return change.rootHead ?? candidate;
      if (args[0] === 'status') return change.status ?? '';
      if (args[0] === 'ls-files') return change.untracked ?? '';
      if (args[0] === '-C' && args[2] === 'fetch') {
        const [commit, ref] = args.at(-1).split(':');
        assert.match(commit, /^[a-f0-9]{40}$/);
        assert.equal(ref, 'refs/heads/polaris-source');
        fetched.set(args[1], commit);
        return '';
      }
      if (args[0] === '-C' && args[2] === 'rev-parse') {
        const commit = fetched.get(args[1]);
        assert.ok(commit, 'source object must be fetched before inspection');
        if (args[3] === `${commit}^{commit}`) return change.fetchedCommit ?? commit;
        assert.equal(args[3], 'refs/heads/polaris-source');
        return change.fetchedRef ?? commit;
      }
      if (args[0] === 'add') {
        assert.equal(existsSync(join(options.cwd, '.polaris-source-receipt.json')), false);
        assert.deepEqual(JSON.parse(readFileSync(join(dirname(options.cwd), 'source-receipt.staging.json'))),
          change.receipt ?? f.receipt);
        return '';
      }
      if (args[0] === 'write-tree') return change.tree ?? (key === 'win' ? windowsTree : tree);
      if (['init', 'apply', 'diff'].includes(args[0])) return '';
      throw new Error(`Unrecognized stub git command: ${args}`);
    }
    if (args[0]?.endsWith('core-source-provision.py')) {
      const checkout = args[args.indexOf('--checkout') + 1];
      write(join(checkout, '.polaris-source-receipt.json'), JSON.stringify(change.receipt ?? f.receipt));
      write(join(checkout, 'release/DEFAULT_BUILD_TAGS'), expectedTags('mac-arm64').join(','));
      write(join(checkout, 'release/DEFAULT_BUILD_TAGS_WINDOWS'), expectedTags('win').join(','));
      write(join(checkout, 'release/LDFLAGS'), '-checklinkname=0');
      write(join(checkout, 'go.mod'), 'synthetic full main go.mod');
      write(join(checkout, 'go.sum'), 'synthetic full main go.sum');
      return JSON.stringify(change.receipt ?? f.receipt);
    }
    if (args[0] === 'env') return '/synthetic-go';
    if (args[0] === 'version' && args.length === 1) return 'go version go1.25.5 linux/amd64';
    if (args[0] === 'version' && args[1] === '-m') return change.metadata ?? metadata(key, args[2]);
    if (args[0] === 'tool' && args[1] === 'buildid') return change.buildID ?? platformSourceIdentity(f.receipt, f.source, f.manifest.sourceBuild, key, f.manifest.windowsBuild.patchSha256).buildID;
    if (args[0] === 'build') { write(args[args.indexOf('-o') + 1], `synthetic binary ${key}`); return ''; }
    if (args[0] === 'test') return '';
    throw new Error(`Unrecognized stub command: ${command} ${args}`);
  };
  return { run, calls };
}
function produceAll(f) {
  const directory = join(f.root, 'bundle');
  for (const key of Object.keys(DESKTOP_TARGETS)) {
    produceDesktopCore(f.root, f.manifest, key, join(directory, key, coreFilename(key)), candidate, stub(f, key).run);
  }
  writeBundleInventory(directory, candidate);
  return directory;
}
const inspectStub = (command, args) => command === 'git'
  ? (args[0] === 'rev-parse' ? candidate : '') : args[0] === 'tool'
    ? JSON.parse(readFileSync(`${args[2]}.source-receipt.json`)).buildID
    : metadata(args[2].split(/[/\\]/).at(-2), args[2]);

test('frozen production inputs still require exact pins; force and old outputs cannot bypass', () => {
  const manifest = JSON.parse(readFileSync(join(repo, 'src-tauri/core-manifest.json')));
  const f = fixture();
  try {
    for (const key of Object.keys(DESKTOP_TARGETS)) validateSourcePins(manifest, key, false);
    const missingPin = structuredClone(manifest);
    missingPin.sourceBuild.sourceReceiptFingerprint = null;
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      const dest = join(f.root, 'resources', key, coreFilename(key));
      write(dest, 'old cached core');
      let calls = 0;
      const run = () => calls++;
      assert.throws(() => produceDesktopCore(f.root, missingPin, key, dest, candidate, run), /not frozen/);
      assert.throws(() => buildDesktopCore(f.root, missingPin, key, dest, true, run, { candidate }), /not frozen/);
      assert.equal(calls, 0);
      assert.equal(readFileSync(dest, 'utf8'), 'old cached core');
    }
    // Run the real CLI against invalid fixture inputs, never the now-frozen
    // product checkout: --force must reject before any source/tool execution.
    write(join(f.root, 'src-tauri/core-manifest.json'), JSON.stringify(missingPin));
    for (const path of ['scripts/fetch-core.mjs', 'scripts/desktop-core/build-core.mjs',
      'scripts/desktop-core/bundle.mjs', 'scripts/desktop-core/source-graph.mjs']) {
      write(join(f.root, path), readFileSync(join(repo, path)));
    }
    const cli = spawnSync(process.execPath, ['scripts/fetch-core.mjs', '--force', '--platform=linux'],
      { cwd: f.root, encoding: 'utf8' });
    assert.equal(cli.status, 1);
    assert.match(cli.stderr, /not frozen/);
    assert.equal(readFileSync(join(f.root, 'resources/linux/sing-box'), 'utf8'), 'old cached core');
  } finally { f.dispose(); }
});

test('first source producer needs input pins but no unknown output hash', () => {
  const f = fixture();
  try {
    assert.throws(() => validateSourcePins(f.manifest, 'linux'), /not frozen/);
    validateSourcePins(f.manifest, 'linux', false);
    assert.equal(frozenSourceVersion(f.manifest), f.manifest.sourceBuild.version);
    for (const field of ['sourceManifestSha256', 'provisionerSha256', 'sourceReceiptFingerprint', 'moduleGraphSha256',
      'patchedSourceTree', 'buildTree', 'version', 'dependencyModules', 'transportPins', 'platforms']) {
      assert.equal(frozenSourceVersion({ ...f.manifest, sourceBuild: { ...f.manifest.sourceBuild, [field]: null } }), undefined);
    }
    for (const [field, value] of [['version', '1.0.0.polaris.0'], ['version', '1.0.1.polaris.2'],
      ['sourceManifestSha256', 'A'.repeat(64)], ['sourceManifestSha256', ['a'.repeat(64)]], ['dependencyModules', ['example.com/tiny', 'example.com/tiny']],
      ['transportPins', { 'example.com/tiny': 1 }],
      ['dependencyModules', ['../module+invalid']]]) {
      assert.equal(frozenSourceVersion({ ...f.manifest, sourceBuild: { ...f.manifest.sourceBuild, [field]: value } }), undefined);
    }
    const s = stub(f, 'linux');
    const receipt = produceDesktopCore(f.root, f.manifest, 'linux', join(f.root, 'out/core'), candidate, s.run);
    assert.equal(receipt.binarySha256, digest('synthetic binary linux'));
    assert.equal(receipt.candidate, candidate);
    assert.ok(s.calls.some(({ args }) => args.includes('--module-source') && args.includes('--go')));
  } finally { f.dispose(); }
});

test('all four targets retain the existing platform feature and CGO faces', () => {
  const f = fixture();
  try {
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      const s = stub(f, key);
      const receipt = produceDesktopCore(f.root, f.manifest, key, join(f.root, key, coreFilename(key)), candidate, s.run);
      const build = s.calls.find(({ args }) => args[0] === 'build');
      assert.deepEqual(build.args[build.args.indexOf('-tags') + 1].split(','), expectedTags(key));
      assert.equal(build.options.env.CGO_ENABLED, DESKTOP_TARGETS[key].cgo);
      assert.equal(build.options.env.GOOS, DESKTOP_TARGETS[key].goos);
      assert.equal(build.options.env.GOARCH, DESKTOP_TARGETS[key].goarch);
      assert.equal(receipt.buildTree, key === 'win' ? windowsTree : tree);
      assert.equal(receipt.sourceFingerprint, f.receipt.fingerprint);
      assert.equal(receipt.buildID, `polaris-desktop-source-v1-${receipt.platformInputFingerprint}`);
      assert.equal(s.calls.filter(({ args }) => args[0] === 'apply').length, key === 'win' ? 2 : 0);
      assert.equal(s.calls.filter(({ args }) => args[0] === 'test').length, key === 'win' ? 1 : 0);
      assert.ok(s.calls.filter(({ args }) => args[0] === 'add').every(({ args }) => args.includes(':(exclude).polaris-source-receipt.json')));
      const fetches = s.calls.filter(({ command, args }) => command === 'git' && args[2] === 'fetch');
      assert.deepEqual(fetches.map(({ args }) => args.slice(4)),
        [['https://github.com/SagerNet/sing-box.git', `${f.source.sourceCommit}:refs/heads/polaris-source`],
          ...f.source.dependencyPatches.map((dep) => [dep.sourceURL, `${dep.upstreamCommit}:refs/heads/polaris-source`])]);
      for (const { args } of fetches) {
        const commit = args.at(-1).split(':')[0];
        assert.deepEqual(s.calls.filter(({ command, args: query }) => command === 'git'
          && query[1] === args[1] && query[2] === 'rev-parse').map(({ args: query }) => query[3]),
        [`${commit}^{commit}`, 'refs/heads/polaris-source']);
      }
    }
  } finally { f.dispose(); }
});

test('non-Linux NFT absence and strictly patched presence pass the real producer and consumer entrypoints', () => {
  const f = fixture();
  try {
    const directory = produceAll(f);
    for (const key of ['win', 'mac-x64', 'mac-arm64']) {
      const receipt = JSON.parse(readFileSync(join(directory, key, `${coreFilename(key)}.source-receipt.json`)));
      assert.equal(Object.hasOwn(receipt.linkedModules, NFT_MODULE), false);
      assert.equal(Object.hasOwn(receipt.linkedModules, TUN_MODULE), true);
      const present = metadata(key, undefined, { nft: true });
      produceDesktopCore(f.root, f.manifest, key, join(directory, key, coreFilename(key)), candidate,
        stub(f, key, { metadata: present }).run);
    }
    writeBundleInventory(directory, candidate);
    consumeDesktopBundle(f.root, f.manifest, directory, candidate, ['win'], (command, args, options) =>
      command !== 'git' && args[0] === 'version'
        ? metadata(args[2].split(/[/\\]/).at(-2), args[2], { nft: true }) : inspectStub(command, args, options));
    assert.equal(existsSync(join(f.root, 'resources/win/sing-box.exe')), true);
    assert.equal(f.manifest.sourceBuild.dependencyModules.length, 3);
    assert.equal(f.receipt.moduleGraph.length, 3);
  } finally { f.dispose(); }
});

function rejectProducerAndConsumerMetadata(f, key, raw, error) {
  const dest = join(f.root, 'rejected/core');
  assert.throws(() => produceDesktopCore(f.root, f.manifest, key, dest, candidate,
    stub(f, key, { metadata: raw }).run), error);
  assert.equal(existsSync(dest), false);
  const directory = produceAll(f);
  assert.throws(() => consumeDesktopBundle(f.root, f.manifest, directory, candidate, ['linux'],
    (command, args, options) => command !== 'git' && args[0] === 'version'
      && args[2].split(/[/\\]/).at(-2) === key ? raw : inspectStub(command, args, options)), error);
  assert.equal(existsSync(join(f.root, 'resources')), false);
}

test('patched module omissions, stock rows and wrong replacement identities reject both entrypoints', () => {
  const f = fixture();
  const omit = (raw, module, name) => raw.replace(
    `\tdep\t${module}\tv1.0.0\t\n\t=>\t./polaris-dependencies/${name}\t(devel)\t\n\t\n`, '');
  try {
    rejectProducerAndConsumerMetadata(f, 'linux', metadata('linux', undefined, { nft: false }), /patched dependency/);
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      rejectProducerAndConsumerMetadata(f, key, omit(metadata(key), TUN_MODULE, 'sing-tun'), /patched dependency/);
      rejectProducerAndConsumerMetadata(f, key, omit(metadata(key), 'example.com/tiny', 'tiny'), /patched dependency/);
      const present = metadata(key, undefined, { nft: true });
      for (const bad of [present.replace('\t=>\t./polaris-dependencies/nftables\t(devel)\t\n\t\n', ''),
        present.replace(`${NFT_MODULE}\tv1.0.0`, `${NFT_MODULE}\tv2.0.0`),
        present.replace('./polaris-dependencies/nftables', './polaris-dependencies/stock-nftables')]) {
        rejectProducerAndConsumerMetadata(f, key, bad, /patched dependency/);
      }
    }
  } finally { f.dispose(); }
});

test('every platform module policy is complete, disjoint and limited to the reviewed NFT omission', () => {
  const f = fixture();
  try {
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      for (const mutate of [
        (p) => { p.patchedModules = null; }, (p) => { p.transportModules = null; },
        (p) => { p.patchedModules.requiredLinked.push(TUN_MODULE); },
        (p) => { p.patchedModules.allowedAbsent.push(TUN_MODULE); },
        (p) => { p.patchedModules.requiredLinked = p.patchedModules.requiredLinked.filter((m) => m !== TUN_MODULE); },
        (p) => { p.patchedModules.requiredLinked = p.patchedModules.requiredLinked.filter((m) => m !== TUN_MODULE);
          p.patchedModules.allowedAbsent.push(TUN_MODULE); },
        (p) => { p.patchedModules.requiredLinked = p.patchedModules.requiredLinked.filter((m) => m !== 'example.com/tiny');
          p.patchedModules.allowedAbsent.push('example.com/tiny'); },
        (p) => { p.patchedModules.allowedAbsent.push('unknown/module'); },
        (p) => { p.patchedModules.allowUnknownMissing = true; },
        (p) => { p.transportModules.requiredLinked.push('example.com/transport'); },
        (p) => { p.transportModules.confirmedAbsent.push('example.com/transport'); },
        (p) => { p.transportModules.requiredLinked = []; },
        (p) => { p.transportModules.confirmedAbsent.push('unknown/transport'); },
        (p) => { p.transportModules.allowUnknownMissing = true; },
      ]) {
        const manifest = structuredClone(f.manifest);
        mutate(manifest.sourceBuild.platforms[key]);
        assert.throws(() => validateSourcePins(manifest, key, false), /not frozen/);
        assert.equal(frozenSourceVersion(manifest), undefined);
        let calls = 0;
        assert.throws(() => produceDesktopCore(f.root, manifest, key, '/must-not-write', candidate, () => calls++), /not frozen/);
        assert.equal(calls, 0);
        assert.throws(() => consumeDesktopBundle(f.root, manifest, '/must-not-read', candidate, ['linux'], () => calls++), /not frozen/);
        assert.equal(calls, 0);
      }
    }
    const manifest = structuredClone(f.manifest);
    manifest.sourceBuild.platforms.linux.patchedModules = {
      requiredLinked: PATCHED_MODULES.filter((m) => m !== NFT_MODULE), allowedAbsent: [NFT_MODULE] };
    assert.throws(() => validateSourcePins(manifest, 'linux', false), /not frozen/);
  } finally { f.dispose(); }
});

test('transport presence is fixed per platform with exact versions and no stock replacements', () => {
  const f = fixture();
  try {
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      const raw = metadata(key);
      const required = raw.replace('\tdep\texample.com/transport\tv0.47.0\th1:synthetic\n', '');
      rejectProducerAndConsumerMetadata(f, key, required, /transport module version/);
      const badVersion = raw.replace('v0.47.0', 'v0.46.0');
      rejectProducerAndConsumerMetadata(f, key, badVersion, /transport module version/);
      const replacement = raw.replace('h1:synthetic\n', 'h1:synthetic\n\t=>\t/cache/transport\t(devel)\t\n\t\n');
      rejectProducerAndConsumerMetadata(f, key, replacement, /transport replacement/);
      if (key === 'linux') {
        rejectProducerAndConsumerMetadata(f, key, metadata(key, undefined, { linuxTransport: false }), /transport module version/);
      } else {
        validateBuildInfo(raw, f.source, key, f.manifest.sourceBuild);
        rejectProducerAndConsumerMetadata(f, key, metadata(key, undefined, { linuxTransport: true }), /Unexpected embedded transport/);
      }
    }
  } finally { f.dispose(); }
});

test('changing a valid platform policy changes only platform identity and rejects old BuildIDs and receipts', () => {
  const f = fixture();
  try {
    const directory = produceAll(f);
    const spec = f.manifest.sourceBuild;
    const identity = platformSourceIdentity(f.receipt, f.source, spec, 'win', f.manifest.windowsBuild.patchSha256);
    spec.platforms.win.patchedModules = { requiredLinked: [...PATCHED_MODULES], allowedAbsent: [] };
    validateSourcePins(f.manifest, 'win', false);
    const changed = platformSourceIdentity(f.receipt, f.source, spec, 'win', f.manifest.windowsBuild.patchSha256);
    assert.equal(changed.sourceFingerprint, identity.sourceFingerprint);
    assert.notEqual(changed.platformInputFingerprint, identity.platformInputFingerprint);
    assert.notEqual(changed.buildID, identity.buildID);
    assert.throws(() => consumeDesktopBundle(f.root, f.manifest, directory, candidate, ['linux'], inspectStub), /Platform source fingerprint/);
    assert.equal(existsSync(join(f.root, 'resources')), false);
    assert.throws(() => produceDesktopCore(f.root, f.manifest, 'win', join(f.root, 'rejected/core'), candidate,
      stub(f, 'win', { metadata: metadata('win', undefined, { nft: true }), buildID: identity.buildID }).run), /buildID/);
    const transportIdentity = platformSourceIdentity(f.receipt, f.source, spec, 'win', f.manifest.windowsBuild.patchSha256);
    spec.platforms.win.transportModules = { requiredLinked: [...TRANSPORT_MODULES], confirmedAbsent: [] };
    const changedTransport = platformSourceIdentity(f.receipt, f.source, spec, 'win', f.manifest.windowsBuild.patchSha256);
    assert.notEqual(changedTransport.platformInputFingerprint, transportIdentity.platformInputFingerprint);
    assert.equal(changedTransport.sourceFingerprint, transportIdentity.sourceFingerprint);
  } finally { f.dispose(); }
});

test('missing candidate and changed input bytes fail before source execution', () => {
  const f = fixture();
  try {
    let calls = 0;
    assert.throws(() => produceDesktopCore(f.root, f.manifest, 'linux', '/must-not-write', undefined, () => calls++), /candidate/);
    for (const change of [{ rootHead: '9'.repeat(40) }, { status: ' M source.go' }]) {
      const s = stub(f, 'linux', change);
      const dest = join(f.root, 'rejected/core');
      assert.throws(() => produceDesktopCore(f.root, f.manifest, 'linux', dest, candidate, s.run), /candidate checkout/);
      assert.ok(s.calls.every(({ command }) => command === 'git'));
      assert.equal(existsSync(dest), false);
    }
    write(join(f.root, 'scripts/libbox-patches/tiny.patch'), 'changed');
    assert.throws(() => produceDesktopCore(f.root, f.manifest, 'linux', '/must-not-write', candidate, () => calls++), /SHA-256/);
    assert.equal(calls, 0);
  } finally { f.dispose(); }
});

test('wrong provider receipt and Windows overlay graph never publish', () => {
  const f = fixture();
  try {
    const dest = join(f.root, 'out/core');
    for (const [change, error] of [[{ fetchedCommit: '9'.repeat(40) }, /source commit/],
      [{ fetchedRef: '9'.repeat(40) }, /source ref/]]) {
      const s = stub(f, 'linux', change);
      assert.throws(() => produceDesktopCore(f.root, f.manifest, 'linux', dest, candidate, s.run), error);
      assert.equal(s.calls.some(({ args }) => args[0]?.endsWith('core-source-provision.py')), false);
      assert.equal(existsSync(dest), false);
    }
    assert.throws(() => produceDesktopCore(f.root, f.manifest, 'linux', dest, candidate, stub(f, 'linux', { buildID: 'stock' }).run), /buildID/);
    assert.throws(() => produceDesktopCore(f.root, f.manifest, 'linux', dest, candidate, stub(f, 'linux', { untracked: 'ignored/unreviewed.go' }).run), /Untracked compiler/);
    const receipt = { ...f.receipt, buildTree: '9'.repeat(40) };
    assert.throws(() => produceDesktopCore(f.root, f.manifest, 'linux', dest, candidate, stub(f, 'linux', { receipt }).run), /fingerprint/);
    assert.throws(() => produceDesktopCore(f.root, f.manifest, 'win', dest, candidate, stub(f, 'win', { tree }).run), /overlay tree/);
    assert.equal(existsSync(dest), false);
  } finally { f.dispose(); }
});

test('receipt verification checks identity beyond a self-consistent fingerprint', () => {
  const f = fixture();
  try {
    const { fingerprint: _old, ...facts } = f.receipt;
    const wrong = signed({ ...facts, dependencies: facts.dependencies.map((dep, index) => index === 0
      ? { ...dep, replacement: '/stock/module/cache' } : dep) });
    assert.throws(() => validateSourceReceipt(wrong, f.source, { ...f.manifest.sourceBuild, sourceReceiptFingerprint: wrong.fingerprint }), /dependency binding/);
    assert.throws(() => validateSourceReceipt({ ...f.receipt, schema: 'unknown' }, f.source, f.manifest.sourceBuild), /Unsupported/);
    assert.throws(() => validateSourceReceipt({ ...f.receipt, graphScope: 'all' }, f.source, f.manifest.sourceBuild), /scope/);
    assert.throws(() => validateSourceReceipt({ ...f.receipt, moduleGraphQueries: ['stock/module'] }, f.source, f.manifest.sourceBuild), /scope/);
    const wrongURL = signed({ ...facts, sourceURL: 'https://github.com/example/stock' });
    assert.throws(() => validateSourceReceipt(wrongURL, f.source, { ...f.manifest.sourceBuild,
      sourceReceiptFingerprint: wrongURL.fingerprint }), /input binding/);
  } finally { f.dispose(); }
});

test('actual embedded stock replacement or feature loss rejects producer output', () => {
  const f = fixture();
  try {
    const dest = join(f.root, 'out/core');
    for (const bad of [metadata('linux').replace('./polaris-dependencies/tiny', '/cache/tiny'),
      metadata('linux').replace('with_tailscale,', ''), metadata('linux').replace('CGO_ENABLED=0', 'CGO_ENABLED=1'),
      metadata('linux').replace('v0.47.0', 'v0.46.0'),
      metadata('linux').replace('h1:synthetic\n', 'h1:synthetic\n\t=>\t/cache/transport\t(devel)\t\n\t\n')]) {
      assert.throws(() => produceDesktopCore(f.root, f.manifest, 'linux', dest, candidate, stub(f, 'linux', { metadata: bad }).run), /[Ee]mbedded/);
      assert.equal(existsSync(dest), false);
    }
  } finally { f.dispose(); }
});

test('complete synthetic bundle is consistent after transport, never native proof', () => {
  const f = fixture();
  try {
    const directory = produceAll(f);
    consumeDesktopBundle(f.root, f.manifest, directory, candidate, ['linux'], inspectStub);
    assert.equal(readFileSync(join(f.root, 'resources/linux/sing-box'), 'utf8'), 'synthetic binary linux');
    assert.equal(existsSync(join(f.root, 'resources/linux/sing-box.source-receipt.json')), false);
    verifyPackagedSource(f.root, f.manifest, 'linux', join(f.root, 'resources/linux/sing-box'));
    write(join(f.root, 'resources/linux/sing-box'), 'packaged corruption');
    assert.throws(() => verifyPackagedSource(f.root, f.manifest, 'linux', join(f.root, 'resources/linux/sing-box')), /SHA-256/);
    assert.equal(existsSync(join(f.root, 'resources/win/sing-box.exe')), false);
    assert.equal(buildInfoFingerprint(metadata('linux', '/producer/path')), buildInfoFingerprint(metadata('linux', '/consumer/path')));
  } finally { f.dispose(); }
});

test('missing unselected platform or wrong candidate rejects before any resource write', () => {
  const f = fixture();
  try {
    const directory = produceAll(f);
    assert.throws(() => consumeDesktopBundle(f.root, f.manifest, directory, '9'.repeat(40), ['linux'], inspectStub), /checkout/);
    rmSync(join(directory, 'win/sing-box.exe'));
    assert.throws(() => consumeDesktopBundle(f.root, f.manifest, directory, candidate, ['linux'], inspectStub));
    assert.equal(existsSync(join(f.root, 'resources')), false);
  } finally { f.dispose(); }
});

test('bundle byte mismatch and unknown receipt fail closed', () => {
  const f = fixture();
  try {
    const directory = produceAll(f);
    const path = join(directory, 'linux/sing-box');
    write(path, 'tampered');
    assert.throws(() => consumeDesktopBundle(f.root, f.manifest, directory, candidate, ['linux'], inspectStub), /SHA-256/);
    write(path, 'synthetic binary linux');
    const receiptFile = join(directory, 'linux/sing-box.source-receipt.json');
    const receipt = JSON.parse(readFileSync(receiptFile));
    write(receiptFile, JSON.stringify({ ...receipt, schema: 'Unknown' }));
    writeBundleInventory(directory, candidate);
    assert.throws(() => consumeDesktopBundle(f.root, f.manifest, directory, candidate, ['linux'], inspectStub), /receipt binding/);
    assert.equal(existsSync(join(f.root, 'resources')), false);
  } finally { f.dispose(); }
});

test('malformed or duplicate native metadata cannot be treated as a match', () => {
  const f = fixture();
  try {
    for (const raw of ['', 'not Go', metadata('linux') + '\tbuild\tGOOS=linux\n',
      metadata('linux').replace('\tdep\texample.com/tiny\tv1.0.0', '\t=>\tbad\t(devel)')]) {
      assert.throws(() => validateBuildInfo(raw, f.source, 'linux', f.manifest.sourceBuild));
    }
  } finally { f.dispose(); }
});

test('shared source provider and desktop-only producers retain explicit platform impact', () => {
  assert.equal(digest(readFileSync(join(repo, 'scripts/core-source-provision.py'))),
    'ef0238183e3076ed3cfa51df824cacd9a74bafa69f40298fa7aae7254e90a24d');
  const provider = 'scripts/core-source-provision.py';
  const sharedImpact = classifyImpact([provider]);
  assert.equal(sharedImpact.kernel, true, provider);
  assert.equal(sharedImpact.platforms.length, 4, provider);
  assert.equal(sharedImpact.android, true, provider);
  assert.deepEqual(androidRegistrationOf(provider), { key: provider, table: 'ANDROID_IMPACT_SCOPES' });
  for (const path of ['scripts/build-desktop-core.test.mjs', 'scripts/desktop-core/source-graph.mjs',
    'scripts/desktop-core/build-core.mjs', 'scripts/desktop-core/bundle.mjs']) {
    const impact = classifyImpact([path]);
    assert.equal(impact.kernel, true, path);
    assert.equal(impact.platforms.length, 4, path);
    assert.equal(impact.android, false, path);
    assert.equal(androidRegistrationOf(path)?.table, 'NO_ANDROID_IMPACT_SCOPES', path);
  }
  assert.match(readFileSync(join(repo, 'scripts/gate-node-test.sh'), 'utf8'), /scripts\/build-desktop-core\.test\.mjs/);
});

test('real tiny local Go cross-platform replacements preserve the graph while omitting Linux-only linked rows', {
  skip: process.env.POLARIS_REAL_GO_BUILDINFO_TEST !== '1' ? 'explicit light Go fixture opt-in; default tests use stubs' : false,
}, () => {
  const work = mkdtempSync(join(tmpdir(), 'polaris-real-buildinfo-'));
  try {
    const source = { goVersion: JSON.parse(readFileSync(join(repo, 'scripts/libbox-patches/source-manifest.json'))).goVersion,
      dependencyPatches: PATCHED_MODULES.map((module, index) => ({ module, name: ['tiny', 'sing-tun', 'nftables'][index], upstreamVersion: 'v1.0.0' })) };
    write(join(work, 'go.mod'), 'module example.com/probe\ngo 1.23\n' + source.dependencyPatches.map((dep) =>
      `require ${dep.module} v1.0.0\nreplace ${dep.module} => ./polaris-dependencies/${dep.name}\n`).join(''));
    write(join(work, 'main.go'), 'package main\nimport ("example.com/tiny"; tun "github.com/sagernet/sing-tun")\nfunc main(){ println(tiny.Value()+tun.Value()) }\n');
    write(join(work, 'main_linux.go'), 'package main\nimport nft "github.com/sagernet/nftables"\nfunc init(){ println(nft.Value()) }\n');
    for (const dep of source.dependencyPatches) {
      write(join(work, 'polaris-dependencies', dep.name, 'go.mod'), `module ${dep.module}\ngo 1.23\n`);
      write(join(work, 'polaris-dependencies', dep.name, 'value.go'), 'package tiny\nfunc Value() int { return 7 }\n');
    }
    const options = { cwd: work, encoding: 'utf8', timeout: 60000,
      env: { ...process.env, GOTOOLCHAIN: 'local', GOWORK: 'off', GOFLAGS: '', GOEXPERIMENT: '', GOPROXY: 'off', GOSUMDB: 'off' } };
    const go = process.env.POLARIS_BUILDINFO_GO ?? 'go';
    const spec = { dependencyModules: PATCHED_MODULES, transportPins: Object.fromEntries(PATCHED_MODULES.map((module) => [module, 'v1.0.0'])),
      platforms: Object.fromEntries(Object.keys(DESKTOP_TARGETS).map((key) => [key, {
        patchedModules: platformPolicies(key).patchedModules,
        transportModules: { requiredLinked: PATCHED_MODULES.filter((module) => key === 'linux' || module !== NFT_MODULE),
          confirmedAbsent: key === 'linux' ? [] : [NFT_MODULE] } }])) };
    const goModSha256 = digest(readFileSync(join(work, 'go.mod')));
    for (const [key, target] of Object.entries(DESKTOP_TARGETS)) {
      const binary = join(work, key, coreFilename(key));
      mkdirSync(dirname(binary), { recursive: true });
      const targetOptions = { ...options, env: { ...options.env, GOOS: target.goos, GOARCH: target.goarch, CGO_ENABLED: target.cgo } };
      const buildID = `polaris-desktop-source-v1-${digest(canonical({ key, policy: spec.platforms[key] }))}`;
      execFileSync(go, ['build', '-mod=readonly', '-trimpath', '-buildvcs=false', '-tags', expectedTags(key).join(','),
        `-ldflags=-buildid=${buildID}`, '-o', binary, '.'], targetOptions);
      assert.equal(execFileSync(go, ['tool', 'buildid', binary], targetOptions).trim(), buildID);
      const raw = execFileSync(go, ['version', '-m', binary], targetOptions);
      const facts = validateBuildInfo(raw, source, key, spec);
      for (const dep of source.dependencyPatches.filter((dep) => key === 'linux' || dep.module !== NFT_MODULE)) {
        assert.equal(facts.modules.get(dep.module)?.version, 'v1.0.0');
        assert.equal(facts.modules.get(dep.module)?.replacement, `./polaris-dependencies/${dep.name}`);
      }
      assert.equal(facts.modules.has(NFT_MODULE), key === 'linux');
      assert.equal(digest(readFileSync(join(work, 'go.mod'))), goModSha256);
    }
  } finally { rmSync(work, { recursive: true, force: true }); }
});
