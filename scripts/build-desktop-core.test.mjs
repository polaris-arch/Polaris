// Stub-runner cases prove consumer logic only. The opt-in final case uses a
// tiny real local Go build; neither case is a sing-box/native-platform verdict.
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
// Keep FK-A cases inside the existing scripts/*.test.mjs CI collection.
import './desktop-core/fork-source.test.mjs';
import { buildDesktopCore, produceDesktopCore } from './desktop-core/build-core.mjs';
import { consumeDesktopBundle, coreFilename, verifyPackagedSource, writeBundleInventory } from './desktop-core/bundle.mjs';
import { buildInfoFingerprint, canonical, DESKTOP_TARGETS, digest, expectedTags, frozenSourceVersion, platformSourceIdentity,
  validateBuildInfo, validateSourcePins, validateSourceReceipt } from './desktop-core/source-graph.mjs';
import { classifyImpact, androidRegistrationOf } from './classify-ci-impact.mjs';

const repo = join(dirname(fileURLToPath(import.meta.url)), '..');
const candidate = 'c'.repeat(40);
const tree = 'd'.repeat(40);
const windowsTree = 'e'.repeat(40);
const PATCHED = [['tiny', 'example.com/tiny'], ['patched', 'example.com/patched'], ['second', 'example.com/second']];
const PATCHED_MODULES = PATCHED.map(([, module]) => module);
const [, FIRST_MODULE, SECOND_MODULE] = PATCHED_MODULES;
const TRANSPORT_MODULES = ['example.com/transport', 'example.com/linux-transport'];
const EMPTY_GRAPH_SHA256 = '4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945';
const platformPolicies = (key, modules = PATCHED_MODULES) => ({
  patchedModules: { requiredLinked: [...modules], allowedAbsent: [] },
  transportModules: { requiredLinked: TRANSPORT_MODULES.filter((module) => key === 'linux' || module !== 'example.com/linux-transport'),
    confirmedAbsent: key === 'linux' ? [] : ['example.com/linux-transport'] },
});
const write = (path, bytes) => { mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, bytes); };
const signed = (facts) => ({ ...facts, fingerprint: digest(canonical(facts)) });
function fixture(patched = PATCHED) {
  const modules = patched.map(([, module]) => module);
  const root = mkdtempSync(join(tmpdir(), 'polaris-graph-logic-'));
  const patch = 'synthetic core patch';
  const depPatch = 'synthetic dependency patch';
  const overlay = 'synthetic Windows overlay';
  const provider = 'synthetic provider; never executed';
  const source = { sourceCommit: 'a'.repeat(40), goVersion: '1.25.5',
    patches: [{ file: 'core.patch', sha256: digest(patch) }], dependencyPatches: patched.map(([name, module]) => {
      return { name, module, upstreamVersion: 'v1.0.0',
        upstreamCommit: 'b'.repeat(40), sourceURL: `https://github.com/example/${name}`,
        patchFile: `${name}.patch`, patchSha256: digest(depPatch), patchedTree: 'f'.repeat(40) };
    }) };
  const manifestBytes = JSON.stringify(source);
  const graph = source.dependencyPatches.map((dep) => ({ Path: dep.module, Version: dep.upstreamVersion,
    Replace: { Path: `./polaris-dependencies/${dep.name}` } }));
  const receipt = signed({ schema: 'polaris-core-source-v1', sourceCommit: source.sourceCommit, sourceURL: 'https://github.com/SagerNet/sing-box',
    graphScope: patched.length ? 'declared-patched-modules' : 'core-source-only',
    sourceGraphState: patched.length ? 'dependencies-patched' : 'source-only', moduleGraphQueries: [...modules].sort(),
    mainGoModSha256: digest('synthetic full main go.mod'), mainGoSumSha256: digest('synthetic full main go.sum'),
    upstreamTree: '1'.repeat(40), patchedSourceTree: '2'.repeat(40), buildTree: tree,
    sourceManifestSha256: digest(manifestBytes), provisionerSha256: digest(provider), patches: source.patches,
    dependencies: source.dependencyPatches.map((dep) => ({ ...dep, upstreamTree: '3'.repeat(40), replacement: `./polaris-dependencies/${dep.name}` })),
    moduleGraph: graph, moduleGraphSha256: digest(canonical(graph)) });
  const manifest = { bundledCoreVersion: '1.0.0', sourceBuild: {
    sourceManifestSha256: receipt.sourceManifestSha256, provisionerSha256: receipt.provisionerSha256,
    sourceReceiptFingerprint: receipt.fingerprint, moduleGraphSha256: receipt.moduleGraphSha256,
    patchedSourceTree: receipt.patchedSourceTree, buildTree: tree, version: '1.0.0.polaris.1',
    dependencyModules: modules, transportPins: { 'example.com/transport': 'v0.47.0', 'example.com/linux-transport': 'v1.2.0' },
    platforms: Object.fromEntries(Object.keys(DESKTOP_TARGETS)
      .map((key) => [key, { buildTree: key === 'win' ? windowsTree : tree, ...platformPolicies(key, modules), binarySha256: null }])) },
  windowsBuild: { sourceCommit: source.sourceCommit, goVersion: source.goVersion, version: '1.0.0.polaris.1', patchSha256: digest(overlay) } };
  write(join(root, 'scripts/libbox-patches/source-manifest.json'), manifestBytes);
  write(join(root, 'scripts/libbox-patches/core.patch'), patch);
  for (const dep of source.dependencyPatches) write(join(root, 'scripts/libbox-patches', dep.patchFile), depPatch);
  write(join(root, 'scripts/core-source-provision.py'), provider);
  write(join(root, 'scripts/core-patches/windows-dns-refresh.patch'), overlay);
  return { root, manifest, source, receipt, patched, dispose: () => rmSync(root, { recursive: true, force: true }) };
}
const patchedRow = (name, module) => `\tdep\t${module}\tv1.0.0\t\n\t=>\t./polaris-dependencies/${name}\t(devel)\t\n\t\n`;
function metadata(key, path = '/synthetic/core', { patched = PATCHED, linuxTransport = key === 'linux' } = {}) {
  const target = DESKTOP_TARGETS[key];
  return `${path}: go1.25.5\n\tpath\texample.com/probe\n\tmod\texample.com/probe\t(devel)\t\n`
    + patched.map(([name, module]) => patchedRow(name, module)).join('')
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
    if (command === 'codesign') {
      const state = change.signature ?? 'signed';
      return { status: state === 'signed' ? 0 : 1, stdout: '', stderr: state === 'signed'
        ? (args[0] === '--display' ? `CDHash=${change.cdHash ?? 'a'.repeat(40)}\n` : '')
        : state === 'unsigned' ? 'core: code object is not signed at all' : 'core: invalid signature' };
    }
    if (command === 'file') return `Mach-O 64-bit executable ${key === 'mac-x64' ? 'x86_64' : 'arm64'}`;
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
    if (args[0] === 'version' && args[1] === '-m') return change.metadata ?? metadata(key, args[2], { patched: f.patched });
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
const inspector = (patched) => (command, args) => command === 'codesign'
  ? { status: 0, stdout: '', stderr: args[0] === '--display' ? `CDHash=${'a'.repeat(40)}\n` : '' } : command === 'git'
  ? (args[0] === 'rev-parse' ? candidate : '') : args[0] === 'tool'
    ? JSON.parse(readFileSync(`${args[2]}.source-receipt.json`)).buildID
    : metadata(args[2].split(/[/\\]/).at(-2), args[2], { patched });
const inspectStub = inspector(PATCHED);

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
  // An empty dependency declaration is a frozen graph of its own: the real
  // producer and consumer entrypoints pass and no module repository is supplied.
  const empty = fixture([]);
  try {
    assert.deepEqual(empty.source.dependencyPatches, []);
    assert.equal(empty.receipt.moduleGraphSha256, EMPTY_GRAPH_SHA256);
    assert.equal(frozenSourceVersion(empty.manifest), empty.manifest.sourceBuild.version);
    const directory = join(empty.root, 'bundle');
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      const s = stub(empty, key);
      const receipt = produceDesktopCore(empty.root, empty.manifest, key, join(directory, key, coreFilename(key)), candidate, s.run);
      assert.equal(Object.values(receipt.linkedModules).some((module) => module.replacement), false);
      const provision = s.calls.filter(({ args }) => args[0]?.endsWith('core-source-provision.py'));
      assert.equal(provision.length, 1);
      assert.equal(provision[0].args.includes('--module-source'), false);
      assert.equal(s.calls.filter(({ command, args }) => command === 'git' && args[2] === 'fetch').length, 1);
    }
    writeBundleInventory(directory, candidate);
    consumeDesktopBundle(empty.root, empty.manifest, directory, candidate, ['linux'], inspector([]));
    assert.equal(readFileSync(join(empty.root, 'resources/linux/sing-box'), 'utf8'), 'synthetic binary linux');
  } finally { empty.dispose(); }
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

function rejectProducerAndConsumerMetadata(f, key, raw, error) {
  const dest = join(f.root, 'rejected/core');
  assert.throws(() => produceDesktopCore(f.root, f.manifest, key, dest, candidate,
    stub(f, key, { metadata: raw }).run), error);
  assert.equal(existsSync(dest), false);
  const directory = produceAll(f);
  const inspect = inspector(f.patched);
  assert.throws(() => consumeDesktopBundle(f.root, f.manifest, directory, candidate, ['linux'],
    (command, args, options) => command !== 'git' && args[0] === 'version'
      && args[2].split(/[/\\]/).at(-2) === key ? raw : inspect(command, args, options)), error);
  assert.equal(existsSync(join(f.root, 'resources')), false);
}

test('patched module omissions, stock rows and wrong replacement identities reject both entrypoints', () => {
  const f = fixture();
  try {
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      const present = metadata(key);
      for (const [name, module] of PATCHED) {
        assert.ok(present.includes(patchedRow(name, module)));
        rejectProducerAndConsumerMetadata(f, key, present.replace(patchedRow(name, module), ''), /patched dependency/);
      }
      for (const bad of [present.replace('\t=>\t./polaris-dependencies/second\t(devel)\t\n\t\n', ''),
        present.replace(`${SECOND_MODULE}\tv1.0.0`, `${SECOND_MODULE}\tv2.0.0`),
        present.replace('./polaris-dependencies/second', './polaris-dependencies/stock-second')]) {
        assert.notEqual(bad, present);
        rejectProducerAndConsumerMetadata(f, key, bad, /patched dependency/);
      }
    }
  } finally { f.dispose(); }
});

test('every platform module policy is complete, disjoint and never absent', () => {
  const f = fixture();
  try {
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      validateSourcePins(f.manifest, key, false);
      for (const mutate of [
        (p) => { p.patchedModules = null; }, (p) => { p.transportModules = null; },
        (p) => { p.patchedModules.requiredLinked.push(FIRST_MODULE); },
        (p) => { p.patchedModules.allowedAbsent.push(FIRST_MODULE); },
        (p) => { p.patchedModules.requiredLinked = p.patchedModules.requiredLinked.filter((m) => m !== FIRST_MODULE); },
        (p) => { p.patchedModules.requiredLinked = p.patchedModules.requiredLinked.filter((m) => m !== FIRST_MODULE);
          p.patchedModules.allowedAbsent.push(FIRST_MODULE); },
        // A complete, disjoint partition is still refused once anything is absent.
        (p) => { p.patchedModules.requiredLinked = p.patchedModules.requiredLinked.filter((m) => m !== SECOND_MODULE);
          p.patchedModules.allowedAbsent.push(SECOND_MODULE); },
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
    spec.platforms.win.transportModules = { requiredLinked: [...TRANSPORT_MODULES], confirmedAbsent: [] };
    validateSourcePins(f.manifest, 'win', false);
    const changed = platformSourceIdentity(f.receipt, f.source, spec, 'win', f.manifest.windowsBuild.patchSha256);
    assert.equal(changed.sourceFingerprint, identity.sourceFingerprint);
    assert.notEqual(changed.platformInputFingerprint, identity.platformInputFingerprint);
    assert.notEqual(changed.buildID, identity.buildID);
    assert.throws(() => consumeDesktopBundle(f.root, f.manifest, directory, candidate, ['linux'], inspectStub), /Platform source fingerprint/);
    assert.equal(existsSync(join(f.root, 'resources')), false);
    assert.throws(() => produceDesktopCore(f.root, f.manifest, 'win', join(f.root, 'rejected/core'), candidate,
      stub(f, 'win', { metadata: metadata('win', undefined, { linuxTransport: true }), buildID: identity.buildID }).run), /buildID/);
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
    // The scope pair follows whether dependencies are declared; both mismatches reject.
    const empty = fixture([]);
    try {
      validateSourceReceipt(empty.receipt, empty.source, empty.manifest.sourceBuild);
      for (const [receipt, source, manifest, field, value] of [
        [f.receipt, f.source, f.manifest, 'graphScope', 'core-source-only'],
        [f.receipt, f.source, f.manifest, 'sourceGraphState', 'source-only'],
        [empty.receipt, empty.source, empty.manifest, 'graphScope', 'declared-patched-modules'],
        [empty.receipt, empty.source, empty.manifest, 'sourceGraphState', 'dependencies-patched']]) {
        const { fingerprint: _stale, ...rest } = receipt;
        const mismatched = signed({ ...rest, [field]: value });
        assert.throws(() => validateSourceReceipt(mismatched, source, { ...manifest.sourceBuild,
          sourceReceiptFingerprint: mismatched.fingerprint }), /module graph scope/);
      }
    } finally { empty.dispose(); }
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
  // With no declared dependency, any linked replacement row is refused by both entrypoints.
  const empty = fixture([]);
  try {
    for (const key of Object.keys(DESKTOP_TARGETS)) {
      validateBuildInfo(metadata(key, undefined, { patched: [] }), empty.source, key, empty.manifest.sourceBuild);
      rejectProducerAndConsumerMetadata(empty, key, metadata(key, undefined, { patched: [['sing-tun', 'github.com/sagernet/sing-tun']] }),
        /Undeclared embedded module replacement/);
    }
  } finally { empty.dispose(); }
  // A declared graph also refuses a replacement on a module outside the declaration.
  const declared = fixture();
  try {
    rejectProducerAndConsumerMetadata(declared, 'linux', metadata('linux', undefined, { patched: [...PATCHED, ['extra', 'example.com/extra']] }),
      /Undeclared embedded module replacement/);
  } finally { declared.dispose(); }
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
    assert.throws(() => writeBundleInventory(directory, candidate), /fingerprint/);
    const { fingerprint: _old, ...facts } = receipt;
    write(receiptFile, JSON.stringify(signed({ ...facts, schema: 'Unknown' })));
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

test('real Git provider preserves dependency index modes and LF manifests without running Go', () => {
  const result = JSON.parse(execFileSync(process.platform === 'win32' ? 'python' : 'python3', ['-c', String.raw`
import copy, importlib.util, json, os, subprocess, sys, tempfile
from pathlib import Path
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('fixture_provider', sys.argv[1])
provider = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provider)
def git(*args, cwd):
    return subprocess.check_output(['git', *args], cwd=cwd, stderr=subprocess.PIPE).decode().strip()
def repository(path, files):
    path.mkdir()
    git('init', '-q', cwd=path)
    git('config', 'core.autocrlf', 'false', cwd=path)
    git('config', 'core.filemode', 'true', cwd=path)
    for name, (content, mode) in files.items():
        (path / name).write_text(content)
        (path / name).chmod(mode)
    git('add', '-A', cwd=path)
    git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@invalid', 'commit', '-qm', 'fixture', cwd=path)
    return git('rev-parse', 'HEAD', cwd=path)
with tempfile.TemporaryDirectory(prefix='polaris-provider-index-') as directory:
    root = Path(directory)
    main = root / 'main'
    commit = repository(main, {'go.mod': ('module example.com/core\n\ngo 1.25.5\n', 0o644), 'go.sum': ('fixture sum\n', 0o644)})
    dependency = root / 'dependency'
    upstream = repository(dependency, {'go.mod': ('module example.com/dependency\n\ngo 1.25.5\n', 0o644),
        'input.go': ('package input\nconst value = 1\n', 0o644), 'keep.sh': ('kept executable\n', 0o755),
        'mode.sh': ('new executable\n', 0o644), 'plain.sh': ('new plain\n', 0o755), 'remove.sh': ('deleted\n', 0o755)})
    (dependency / 'input.go').write_text('package input\nconst value = 2\n')
    (dependency / 'mode.sh').chmod(0o755)
    (dependency / 'plain.sh').chmod(0o644)
    (dependency / 'remove.sh').unlink()
    (dependency / 'added.sh').write_text('added executable\n')
    (dependency / 'added.sh').chmod(0o755)
    git('add', '-A', cwd=dependency)
    expected_tree = git('write-tree', cwd=dependency)
    patch = root / 'dependency.patch'
    patch.write_bytes(subprocess.check_output(['git', 'diff', '--cached', '--binary', upstream], cwd=dependency))
    git('reset', '--hard', upstream, cwd=dependency)
    base = {'sourceCommit': commit, 'goVersion': '1.25.5', 'patches': [], 'dependencyPatches': []}
    dep = {'name': 'dependency', 'module': 'example.com/dependency', 'sourceURL': 'https://github.com/fixture/dependency',
        'upstreamCommit': upstream, 'upstreamVersion': 'v1.0.0', 'patchFile': patch.name,
        'patchSha256': provider.digest(patch.read_bytes()), 'patchedTree': expected_tree}
    second = root / 'second'
    second_commit = repository(second, {'go.mod': ('module example.com/dependency-two\n\ngo 1.25.5\n', 0o644),
        'input.go': ('package input\nconst value = 1\n', 0o644)})
    (second / 'input.go').write_text('package input\nconst value = 2\n')
    git('add', '-A', cwd=second)
    second_tree = git('write-tree', cwd=second)
    second_patch = root / 'second.patch'
    second_patch.write_bytes(subprocess.check_output(['git', 'diff', '--cached', '--binary', second_commit], cwd=second))
    git('reset', '--hard', second_commit, cwd=second)
    dep2 = {'name': 'second', 'module': 'example.com/dependency-two', 'sourceURL': 'https://github.com/fixture/second',
        'upstreamCommit': second_commit, 'upstreamVersion': 'v1.0.0', 'patchFile': second_patch.name,
        'patchSha256': provider.digest(second_patch.read_bytes()), 'patchedTree': second_tree}
    dependencies = [dep, dep2]
    go = root / 'go-stub'
    go.write_text('never executed')
    original_run, original_export = provider.run, provider.export_module
    def replay(name, windows=False, declaration=None, corrupt=False):
        manifest = copy.deepcopy(declaration if declaration is not None else base | {'dependencyPatches': dependencies})
        manifest_path = root / 'manifest.json'
        manifest_path.write_text(json.dumps(manifest))
        checkout = root / name
        # This is a private configuration file, never the user's global config.
        config = root / 'fixture.gitconfig'
        config.write_text('[core]\n\tautocrlf = ' + ('true' if windows else 'false') + '\n')
        os.environ['GIT_CONFIG_GLOBAL'] = str(config)
        # CI mirrors have their own Windows configuration: disabling conversion
        # in the build checkout alone must not hide archive conversion here.
        for mirror in (dependency, second):
            git('config', 'core.autocrlf', 'true' if windows else 'false', cwd=mirror)
        def run(args, **kwargs):
            if args[0] == str(go):
                if args[1:] == ['version']:
                    return 'go version go1.25.5 linux/amd64\n'
                if args[1:3] == ['mod', 'edit']:
                    edited = next(item for item in dependencies if args[3].startswith('-replace=' + item['module'] + '='))
                    target = Path(kwargs['cwd']) / 'go.mod'
                    target.write_text(target.read_text() + '\nreplace ' + edited['module'] + ' => ./polaris-dependencies/' + edited['name'] + '\n')
                    return ''
                if args[1:3] == ['list', '-m']:
                    return '\n'.join(json.dumps({'Path': item['module'], 'Version': item['upstreamVersion'],
                        'Replace': {'Path': './polaris-dependencies/' + item['name'], 'Dir': str(checkout / 'polaris-dependencies' / item['name'])}})
                        for item in manifest['dependencyPatches'])
                raise AssertionError('unexpected Go stub request: ' + repr(args))
            assert args[0] == 'git', 'only Git and explicit in-process Go stubs are allowed'
            try:
                value = original_run(args, **kwargs)
            except subprocess.CalledProcessError as error:
                if name not in ('wrong-upstream', 'corrupt-input'):
                    print((error.stdout + error.stderr).decode(), file=sys.stderr)
                raise
            if windows and args[1] == 'clone':
                git('config', 'core.filemode', 'false', cwd=checkout)
            return value
        def export(repository, revision, target):
            original_export(repository, revision, target)
            if windows:
                for file in target.rglob('*'):
                    if file.is_file(): file.chmod(0o644)
            if corrupt: (target / 'input.go').write_text('corrupt input\n')
        provider.run, provider.export_module = run, export
        return provider.provision(manifest_path, main, checkout, {dep['module']: dependency, dep2['module']: second}, go)
    linux_plain = replay('linux-plain', declaration=base)
    windows_plain = replay('windows-plain', True, declaration=base)
    linux = replay('linux-dependency')
    try:
        windows = replay('windows-dependency', True)
        windows_error = None
    except (RuntimeError, subprocess.CalledProcessError) as error:
        windows, windows_error = None, str(error)
    rejected = {}
    for name, declaration, corrupt, expected in [
        ('wrong-patch', base | {'dependencyPatches': [dep | {'patchSha256': '0' * 64}]}, False, 'pinned patch hash'),
        ('wrong-tree', base | {'dependencyPatches': [dep | {'patchedTree': '0' * 40}]}, False, 'patched tree differs'),
        ('wrong-upstream', base | {'dependencyPatches': [dep | {'upstreamCommit': '0' * 40}]}, False, None),
        ('corrupt-input', None, True, None),
    ]:
        try:
            replay(name, True, declaration, corrupt)
            raise AssertionError(name + ' unexpectedly accepted')
        except (RuntimeError, subprocess.CalledProcessError) as error:
            if expected: assert expected in str(error), str(error)
            if name in ('wrong-patch', 'wrong-upstream'): assert not (root / name).exists()
            rejected[name] = True
    occupied = root / 'occupied'
    occupied.mkdir()
    (occupied / 'sentinel').write_text('retain existing checkout')
    try:
        replay('occupied')
        raise AssertionError('occupied checkout accepted')
    except RuntimeError as error:
        assert 'fresh and empty' in str(error)
        assert (occupied / 'sentinel').read_text() == 'retain existing checkout'
        rejected['occupied'] = True
    git('update-index', '--chmod=-x', 'polaris-dependencies/dependency/keep.sh', cwd=root / 'linux-dependency')
    try:
        provider.verify_checkout(root / 'linux-dependency', linux)
        raise AssertionError('tampered index mode accepted')
    except RuntimeError as error:
        assert 'staged build source tree differs' in str(error)
        rejected['tampered-index-mode'] = True
    assert git('status', '--porcelain', cwd=main) == ''
    assert git('status', '--porcelain', cwd=dependency) == ''
    assert git('status', '--porcelain', cwd=second) == ''
    cli_manifest = root / 'invalid-cli-manifest.json'
    cli_manifest.write_text(json.dumps(base | {'sourceCommit': '0' * 40}))
    cli = subprocess.run([sys.executable, str(Path(sys.argv[1])), '--manifest', str(cli_manifest),
        '--source', str(main), '--checkout', str(root / 'invalid-cli-checkout')], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert cli.returncode != 0
    assert not (root / 'invalid-cli-checkout').exists()
    cli_error = cli.stderr.decode()
    print(json.dumps({'lfManifestEqual': linux_plain == windows_plain,
        'sourceOnly': {key: linux_plain[key] for key in ('moduleGraph', 'moduleGraphSha256', 'moduleGraphQueries', 'dependencies', 'graphScope', 'sourceGraphState')},
        'sourceOnlyRawSums': [linux_plain['mainGoSumSha256'], windows_plain['mainGoSumSha256']],
        'fullReceiptEqual': linux == windows, 'windowsError': windows_error, 'expectedTree': expected_tree,
        'linuxTree': linux['dependencies'][0]['patchedTree'], 'rejected': rejected,
        'cliNonzeroDiagnostics': {'nonzero': cli.returncode != 0,
            'stdoutPreserved': 'command stdout:\n' + '0' * 40 + '^{commit}' in cli_error,
            'stderrPreserved': 'command stderr:\nfatal:' in cli_error}}))
`, join(repo, 'scripts/core-source-provision.py')], {
    encoding: 'utf8', env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1' },
  }));
  assert.equal(result.lfManifestEqual, true, JSON.stringify(result));
  assert.deepEqual(result.sourceOnly, { moduleGraph: [], moduleGraphSha256: EMPTY_GRAPH_SHA256, moduleGraphQueries: [],
    dependencies: [], graphScope: 'core-source-only', sourceGraphState: 'source-only' });
  assert.equal(digest(canonical([])), EMPTY_GRAPH_SHA256);
  assert.equal(result.fullReceiptEqual, true, JSON.stringify(result));
  assert.equal(result.windowsError, null);
  assert.equal(result.linuxTree, result.expectedTree);
  assert.deepEqual(result.rejected, { 'wrong-patch': true, 'wrong-tree': true, 'wrong-upstream': true,
    'corrupt-input': true, occupied: true, 'tampered-index-mode': true });
  assert.deepEqual(result.cliNonzeroDiagnostics, { nonzero: true, stdoutPreserved: true, stderrPreserved: true });
});

test('shared source provider and desktop-only producers retain explicit platform impact', () => {
  assert.equal(digest(readFileSync(join(repo, 'scripts/core-source-provision.py'))),
    'a2d1379d13b6c1efe28bb07d9f470111d0f5e83eb17439c76da180cac546f63d');
  const provider = 'scripts/core-source-provision.py';
  const sharedImpact = classifyImpact([provider]);
  assert.equal(sharedImpact.kernel, true, provider);
  assert.equal(sharedImpact.platforms.length, 4, provider);
  assert.equal(sharedImpact.android, true, provider);
  assert.deepEqual(androidRegistrationOf(provider), { key: provider, table: 'ANDROID_IMPACT_SCOPES' });
  for (const path of ['scripts/build-desktop-core.test.mjs', 'scripts/desktop-core/source-graph.mjs',
    'scripts/desktop-core/build-core.mjs', 'scripts/desktop-core/bundle.mjs', 'scripts/desktop-core/fork-source.test.mjs']) {
    const impact = classifyImpact([path]);
    assert.equal(impact.kernel, true, path);
    assert.equal(impact.platforms.length, 4, path);
    assert.equal(impact.android, false, path);
    assert.equal(androidRegistrationOf(path)?.table, 'NO_ANDROID_IMPACT_SCOPES', path);
  }
  assert.match(readFileSync(join(repo, 'scripts/gate-node-test.sh'), 'utf8'), /scripts\/build-desktop-core\.test\.mjs/);
});

test('real tiny local Go cross-platform replacements enter every target BuildInfo', {
  skip: process.env.POLARIS_REAL_GO_BUILDINFO_TEST !== '1' ? 'explicit light Go fixture opt-in; default tests use stubs' : false,
}, () => {
  const work = mkdtempSync(join(tmpdir(), 'polaris-real-buildinfo-'));
  try {
    const source = { goVersion: JSON.parse(readFileSync(join(repo, 'scripts/libbox-patches/source-manifest.json'))).goVersion,
      dependencyPatches: PATCHED.map(([name, module]) => ({ module, name, upstreamVersion: 'v1.0.0' })) };
    write(join(work, 'go.mod'), 'module example.com/probe\ngo 1.23\n' + source.dependencyPatches.map((dep) =>
      `require ${dep.module} v1.0.0\nreplace ${dep.module} => ./polaris-dependencies/${dep.name}\n`).join(''));
    write(join(work, 'main.go'), 'package main\nimport ("example.com/tiny"; first "example.com/patched"; second "example.com/second")\nfunc main(){ println(tiny.Value()+first.Value()+second.Value()) }\n');
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
        transportModules: { requiredLinked: [...PATCHED_MODULES], confirmedAbsent: [] } }])) };
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
      for (const dep of source.dependencyPatches) {
        assert.equal(facts.modules.get(dep.module)?.version, 'v1.0.0');
        assert.equal(facts.modules.get(dep.module)?.replacement, `./polaris-dependencies/${dep.name}`);
      }
      assert.equal(digest(readFileSync(join(work, 'go.mod'))), goModSha256);
    }
  } finally { rmSync(work, { recursive: true, force: true }); }
});

test('Mac producer observes final signature bytes before hashing, and rejects missing/bad signatures', () => {
  const f = fixture();
  try {
    for (const key of ['mac-x64', 'mac-arm64']) {
      const s = stub(f, key);
      const run = (command, args, options) => {
        // Synthetic signing-byte boundary only, not a real codesign verdict.
        if (command === 'codesign' && args[0] === '--verify') write(args.at(-1), `final signed bytes ${key}`);
        return s.run(command, args, options);
      };
      const dest = join(f.root, 'observed', key, 'sing-box');
      const receipt = produceDesktopCore(f.root, f.manifest, key, dest, candidate, run);
      assert.equal(receipt.binarySha256, digest(`final signed bytes ${key}`));
      assert.deepEqual(receipt.macCodeSignature, { state: 'signed', cdHash: 'a'.repeat(40) });
      assert.equal(digest(readFileSync(dest)), receipt.binarySha256);
      for (const state of ['invalid', ...(key === 'mac-arm64' ? ['unsigned'] : [])]) {
        const blocked = join(f.root, 'blocked', key, state);
        assert.throws(() => produceDesktopCore(f.root, f.manifest, key, blocked, candidate,
          stub(f, key, { signature: state }).run), /signature invalid or missing/);
        assert.equal(existsSync(blocked), false);
      }
      for (const cdHash of ['', 'not-a-cdhash', 'a'.repeat(64)]) {
        assert.throws(() => produceDesktopCore(f.root, f.manifest, key, dest, candidate,
          stub(f, key, { cdHash }).run), /unique actual CDHash/);
      }
    }
    const unsigned = produceDesktopCore(f.root, f.manifest, 'mac-x64', join(f.root, 'intel/core'), candidate,
      stub(f, 'mac-x64', { signature: 'unsigned' }).run);
    assert.deepEqual(unsigned.macCodeSignature, { state: 'unsigned', cdHash: null });
  } finally { f.dispose(); }
});

test('Mac signature state/CDHash must match actual consumer observations and immutable transport bytes', () => {
  for (const mutation of ['state', 'cdHash', 'fingerprint', 'bytes']) {
    const f = fixture();
    try {
      const directory = produceAll(f);
      // Explicit mock host exercises the actual Mac consumer branch on Linux.
      consumeDesktopBundle(f.root, f.manifest, directory, candidate, ['mac-arm64'], inspectStub, 'darwin');
      const packaged = join(f.root, 'resources/mac-arm64/sing-box');
      verifyPackagedSource(f.root, f.manifest, 'mac-arm64', packaged, inspectStub, 'darwin');
      const wrongActualSignature = (command, args) => command === 'codesign' && args[0] === '--display'
        ? { status: 0, stderr: `CDHash=${'b'.repeat(40)}\n` } : inspectStub(command, args);
      assert.throws(() => verifyPackagedSource(f.root, f.manifest, 'mac-arm64', packaged,
        wrongActualSignature, 'darwin'), /Actual Mac signature/);
      const file = join(directory, 'mac-x64', 'sing-box.source-receipt.json');
      const receipt = JSON.parse(readFileSync(file));
      if (mutation === 'bytes') write(join(directory, 'mac-x64', 'sing-box'), 'modified after signature');
      else {
        if (mutation === 'state') receipt.macCodeSignature = { state: 'unsigned', cdHash: null };
        if (mutation === 'cdHash') receipt.macCodeSignature.cdHash = 'b'.repeat(40);
        if (mutation !== 'fingerprint') {
          const { fingerprint: _old, ...facts } = receipt;
          receipt.fingerprint = digest(canonical(facts));
        } else receipt.macCodeSignature.cdHash = 'b'.repeat(40);
        write(file, JSON.stringify(receipt));
      }
      if (mutation === 'fingerprint' || mutation === 'bytes') assert.throws(() => writeBundleInventory(directory, candidate), /fingerprint|SHA-256/);
      else {
        writeBundleInventory(directory, candidate);
        assert.throws(() => consumeDesktopBundle(f.root, f.manifest, directory, candidate, ['linux'], inspectStub, 'darwin'), /Actual Mac signature/);
      }
    } finally { f.dispose(); }
  }
});

test('producer refuses a byte change after receipt hash and before publishing staged output', () => {
  const f = fixture();
  try {
    const s = stub(f, 'mac-arm64');
    const run = (command, args, options) => {
      if (command === 'git' && args[0] === 'add') {
        const build = s.calls.find((x) => x.args[0] === 'build');
        if (build) write(build.args[build.args.indexOf('-o') + 1], 'modified after final digest');
      }
      return s.run(command, args, options);
    };
    const dest = join(f.root, 'must-not-publish/core');
    assert.throws(() => produceDesktopCore(f.root, f.manifest, 'mac-arm64', dest, candidate, run), /SHA-256/);
    assert.equal(existsSync(dest), false);
  } finally { f.dispose(); }
});

test('consumer rejects a selected artifact changed after all inspections but before staging', () => {
  const f = fixture();
  try {
    const directory = produceAll(f);
    const run = (command, args) => {
      const answer = inspectStub(command, args);
      if (args[0] === 'tool' && args[1] === 'buildid' && args[2].split(/[/\\]/).at(-2) === 'mac-arm64') {
        write(join(directory, 'mac-x64/sing-box'), 'changed after inspection');
      }
      return answer;
    };
    assert.throws(() => consumeDesktopBundle(f.root, f.manifest, directory, candidate,
      ['mac-x64'], run, 'darwin'), /SHA-256/);
    assert.equal(existsSync(join(f.root, 'resources/mac-x64/sing-box')), false);
  } finally { f.dispose(); }
});
