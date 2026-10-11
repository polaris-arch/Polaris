// Synthetic bytes and mocked public HTTP/Go metadata. No Git init, HTTP, core or C API execution.
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync,
  rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { digest } from './source-graph.mjs';
import { ADAPTER_FILES, consumeRelease, hasDesktopRelease, loadReleaseContract, portableCanonical,
  RELEASE_TARGETS, recoverRelease, releasePlatforms, validateReleaseContract, validateReleaseNotice, verifyPackagedRelease,
  verifyReleaseProvenance } from './release-consumer.mjs';
import { inspectNativePayload } from '../native-payload-input.mjs';
import { releaseGateContext, verifyNativeReleaseGate } from './release-native-gates.mjs';
import { validationContext } from './validation-origin.mjs';
import { verifyPackagedSource } from './bundle.mjs';
import { assertSourceFirstRelease } from '../assert-pc-runtime-release.mjs';
import { cronetFixture } from '../lib/cronet-payload.test.mjs';

const repo = join(dirname(fileURLToPath(import.meta.url)), '../..');
const PYTHON = process.platform === 'win32' ? 'python' : 'python3';
const candidate = 'c'.repeat(40), commit = '1'.repeat(40), tree = '2'.repeat(40), tagObject = '3'.repeat(40);
const version = '1.15.0-alpha.12.polaris.1', tag = `v${version}`;
const write = (p, b) => { mkdirSync(dirname(p), { recursive: true }); writeFileSync(p, b); };
const encode = (v) => Buffer.from(JSON.stringify(v));
const identity = (schema, facts) => ({ schema, facts, fingerprint: digest(portableCanonical(facts)) });
const providerNames = ['scripts/polaris_production_release.py', 'scripts/polaris_native_candidate.py',
  'scripts/polaris_source_fingerprint.py', 'scripts/polaris_dns_identity.py', 'scripts/polaris_go_environment.py',
  'scripts/polaris_native_distribution.py', 'release/polaris-production-release.json', 'release/polaris-candidate-inputs.json'];
function coreFixture(key) {
  const t = RELEASE_TARGETS[key], b = Buffer.alloc(256);
  if (t.os === 'linux') { b.set([127, 69, 76, 70, 2, 1]); b.writeUInt16LE(62, 18); }
  else if (t.os === 'windows') { b.write('MZ'); b.writeUInt32LE(0x80, 60); b.write('PE\0\0', 0x80); b.writeUInt16LE(0x8664, 0x84); }
  else { b.writeUInt32LE(0xfeedfacf); b.writeUInt32LE(t.arch === 'amd64' ? 0x1000007 : 0x100000c, 4); }
  b.write(`inert-${key}`, 200); return b;
}
function metadata(row) {
  const g = row.buildGraph;
  return `/synthetic/sing-box: go1.25.5\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n\tmod\tgithub.com/sagernet/sing-box\t(devel)\n`
    + `\tdep\t${g.localReplacement.module}\t${g.localReplacement.version}\n\t=>\t./third_party/sing-tun\t(devel)\n`
    + [...g.normalModules, ...g.prebuiltAcquisitionModules].map((d) => `\tdep\t${d.module}\t${d.version}\t${d.h1}\n`).join('')
    + Object.entries(g.settings).map(([k, v]) => `\tbuild\t${k}=${v}\n`).join('');
}
function fixture(options = {}) {
  const root = mkdtempSync(join(tmpdir(), 'polaris-release-fixture-'));
  const transport = new Map(), calls = [];
  const contract = { schema: 'polaris-box-release-consumer-contract-v1', providerContract: 'polaris-box-release-v1',
    repository: 'polaris-arch/polaris-box', releaseId: 17, sourceCommit: commit, sourceTree: tree,
    version, tag, tagObject, platforms: {} };
  const asset = (name, bytes, id) => ({ name, bytes: bytes.length, sha256: digest(bytes), assetId: id,
    releaseId: 17, url: `https://github.com/polaris-arch/polaris-box/releases/download/${tag}/${name}` });
  const raw = { schema: 'fork-source-fingerprint-v1', sourceCommit: commit, sourceTree: tree, sourceOverlay: 0,
    goVersion: 'go1.25.5', goModSha256: 'a'.repeat(64), goSumSha256: 'b'.repeat(64),
    replacement: { module: 'github.com/sagernet/sing-tun', bindingSha256: 'f'.repeat(64) }, observedHost: 'synthetic only' };
  const { schema, observedHost, ...sourceFacts } = raw;
  const portableSource = identity('polaris-production-source-v1', sourceFacts);
  const provider = identity('polaris-production-provider-v1', { sourceCommit: commit, providerContract: contract.providerContract,
    fileSha256: Object.fromEntries(providerNames.map((n) => [n, 'd'.repeat(64)])) });
  for (const [key, t] of Object.entries(RELEASE_TARGETS)) {
    const rootName = `polaris-box-${version}-${t.os}-${t.arch}`;
    const tags = ['with_naive_outbound', 'with_gvisor', ...(t.sidecar ? ['with_purego'] : [])].sort().join(',');
    const platform = identity('polaris-production-platform-v1', { sourceFingerprint: portableSource.fingerprint,
      os: t.os, arch: t.arch, CGO_ENABLED: t.cgo, tags: tags.split(','), version, tag, goVersion: raw.goVersion, sourceOverlay: 0 });
    platform.buildID = `polaris-production-v1-${platform.fingerprint}`;
    const flags = `-checklinkname=0 -s -w -X github.com/sagernet/sing-box/constant.Version=${version} -buildid=${platform.buildID}`;
    const provenance = { schema: 'polaris-production-native-receipt-v1', target: `${t.os}/${t.arch}`, version, tag,
      sourceCommit: commit, sourceTree: tree, sourceFingerprint: raw, portableSourceIdentity: portableSource,
      portablePlatformIdentity: platform, sourceIdentityFingerprint: portableSource.fingerprint,
      platformInputFingerprint: platform.fingerprint, buildID: platform.buildID,
      providerContract: contract.providerContract, providerFingerprint: provider.fingerprint, providerIdentity: provider,
      signaturePolicy: 'preserve measured original bytes; ad-hoc/not-signed observations do not assert trust',
      signatureObservation: { core: t.os === 'darwin' ? { format: 'Mach-O', signatureTrustValidated: false,
        codeSignatureInspection: options.unsignedIntel && key === 'mac-x64' ? 'sing-box: code object is not signed at all\n'
          : `Executable=/synthetic/sing-box\nSignature=adhoc\nCDHash=${'4'.repeat(40)}\n` }
        : { synthetic: true }, cronet: {}, trustValidated: false }, tags, ldflags: flags,
      buildCommand: ['go', 'build', '-buildvcs=true', '-mod=readonly', '-p', '1', '-trimpath', '-tags', tags,
        '-ldflags', flags, '-o', `/synthetic/${rootName}/${t.core}`, './cmd/sing-box'],
      buildGraph: { settings: { GOOS: t.os, GOARCH: t.arch, CGO_ENABLED: t.cgo, 'vcs.revision': commit,
        'vcs.modified': 'false', '-trimpath': 'true', '-tags': tags },
      normalModules: [{ module: 'example.com/inert', version: 'v1.0.0', h1: 'h1:AA=' }],
      prebuiltAcquisitionModules: t.sidecar ? [{ module: `github.com/sagernet/cronet-go/lib/${t.os}_amd64`, version: 'v1.0.0', h1: 'h1:BB=' }] : [],
      localReplacement: { module: 'github.com/sagernet/sing-tun', version: 'v0.9.7', h1: '' } },
      consumerAcceptance: 'pending', actualVersionOutput: `sing-box version ${version}\n`, fixtureOnly: true };
    const coreBytes = coreFixture(key), sidecarBytes = t.sidecar ? cronetFixture(key) : null;
    const originalArchive = { name: rootName + t.suffix, bytes: 1, sha256: 'e'.repeat(64), members: {
      [`${rootName}/${t.core}`]: digest(coreBytes), ...(t.sidecar ? { [`${rootName}/${t.sidecar}`]: digest(sidecarBytes) } : {}) } };
    const finalPackageCronetSmoke = { os: t.os, arch: t.arch, go: raw.goVersion, gvisorCompiled: true,
      linkage: t.sidecar ? 'shared-purego' : 'static-cgo', version: '154.0.8037.49',
      ...(t.sidecar ? { library: `/synthetic/${rootName}/${t.sidecar}`, sha256: digest(sidecarBytes) } : {}) };
    const files = { [t.core]: coreBytes, LICENSE: Buffer.from('synthetic license'), NOTICE: Buffer.from('synthetic notice'),
      'licenses/NOTICE': Buffer.from('synthetic notice'), 'provenance.json': encode({ ...provenance, archive: originalArchive }),
      'source/SOURCE-INDEX.json': encode({ fixtureOnly: true }) };
    files.SHA256SUMS = Buffer.from(Object.keys(files).sort().map((n) => `${digest(files[n])}  ${n}\n`).join(''));
    if (t.sidecar) { files[t.sidecar] = sidecarBytes; files.SHA256SUMS = Buffer.from(Object.keys(files).filter((n) => n !== 'SHA256SUMS').sort().map((n) => `${digest(files[n])}  ${n}\n`).join('')); }
    const archiveFile = join(root, 'inputs', rootName + t.suffix);
    write(join(root, 'members.json'), encode(Object.fromEntries(Object.entries(files).map(([n, b]) => [rootName + '/' + n, b.toString('base64')]))));
    mkdirSync(dirname(archiveFile), { recursive: true });
    execFileSync(PYTHON, ['-B', '-c', `import base64,io,json,sys,tarfile,zipfile
members=json.load(open(sys.argv[1])); path=sys.argv[2]
if path.endswith('.zip'):
 with zipfile.ZipFile(path,'w',zipfile.ZIP_DEFLATED) as z:
  for name,value in members.items():
   info=zipfile.ZipInfo(name);info.external_attr=(0o100755 if name.endswith('sing-box.exe') else 0o100644)<<16;z.writestr(info,base64.b64decode(value))
else:
 with tarfile.open(path,'w:gz') as z:
  for name,value in members.items():
   b=base64.b64decode(value);info=tarfile.TarInfo(name);info.size=len(b);info.mode=0o755 if name.endswith('/sing-box') else 0o644;z.addfile(info,io.BytesIO(b))
`, join(root, 'members.json'), archiveFile]);
    const bytes = readFileSync(archiveFile), releaseAsset = asset(rootName + t.suffix, bytes, 20 + Object.keys(contract.platforms).length);
    contract.platforms[key] = { ...provenance, finalPackageCronetSmoke, releaseAsset, archive: { name: releaseAsset.name, bytes: bytes.length, sha256: digest(bytes),
      members: Object.fromEntries(Object.entries(files).map(([n, b]) => [rootName + '/' + n, digest(b)])) },
    core: { member: t.core, bytes: files[t.core].length, sha256: digest(files[t.core]) },
    sidecar: t.sidecar ? { member: t.sidecar, bytes: files[t.sidecar].length, sha256: digest(files[t.sidecar]) } : null };
    transport.set(releaseAsset.url, bytes);
  }
  const rows = Object.values(contract.platforms).map(({ releaseAsset, ...r }) => r);
  const index = encode({ schema: 'polaris-production-assets-v1', repository: contract.repository, version, tag, tagObject,
    sourceCommit: commit, sourceTree: tree, platforms: [...rows, { target: 'linux/arm64' }, { target: 'windows/arm64' }] });
  contract.assetIndex = asset('polaris-box-assets.json', index, 25); transport.set(contract.assetIndex.url, index);
  const contractBytes = encode(contract);
  const manifest = { bundledCoreVersion: '1.15.0-alpha.8', desktopSourceBuild: { old: 'never accepted for release' }, desktopRelease: {
    schema: 'polaris-app-release-selection-v1', contractPath: 'scripts/desktop-core/release-contract.json',
    contractSha256: digest(contractBytes), version, tag, tagObject, sourceCommit: commit, sourceTree: tree, releaseId: 17,
    librarySha256: { linux: contract.platforms.linux.sidecar.sha256, win: contract.platforms.win.sidecar.sha256 } } };
  write(join(root, manifest.desktopRelease.contractPath), contractBytes);
  write(join(root, 'src-tauri/core-manifest.json'), encode(manifest));
  for (const name of ADAPTER_FILES) { mkdirSync(dirname(join(root, name)), { recursive: true }); copyFileSync(join(repo, name), join(root, name)); }
  const base = `https://api.github.com/repos/${contract.repository}`;
  transport.set(base, encode({ id: 1407705262, full_name: contract.repository }));
  const publicRelease = { id: 17, tag_name: tag, draft: false, html_url: `https://github.com/${contract.repository}/releases/tag/${tag}`,
    assets: [contract.assetIndex, ...Object.values(contract.platforms).map((r) => r.releaseAsset)].map((a) => ({ id: a.assetId, name: a.name,
      size: a.bytes, state: 'uploaded', browser_download_url: a.url, digest: `sha256:${a.sha256}` })) };
  transport.set(base + '/releases/17', encode(publicRelease));
  transport.set(base + '/git/ref/tags/' + tag, encode({ ref: 'refs/tags/' + tag, object: { type: 'tag', sha: tagObject } }));
  transport.set(base + '/git/tags/' + tagObject, encode({ sha: tagObject, tag, object: { type: 'commit', sha: commit } }));
  transport.set(base + '/git/commits/' + commit, encode({ sha: commit, tree: { sha: tree } }));
  const read = async (url) => { calls.push(url); assert(transport.has(url), `Unexpected HTTP: ${url}`); return transport.get(url); };
  const inspections = [];
  const run = (command, args, options) => {
    if (command === 'git') return args[0] === 'rev-parse' ? candidate : '';
    if (command === 'go') {
      const path = args.at(-1), hash = digest(readFileSync(path));
      const row = Object.values(contract.platforms).find((r) => r.core.sha256 === hash);
      assert(row, 'Unknown inert binary'); return args[0] === 'version' ? metadata(row) : row.buildID;
    }
    if (command === 'codesign' || command === 'file') {
      inspections.push([command, ...args]);
      const hash = digest(readFileSync(args.at(-1))), row = Object.values(contract.platforms).find((r) => r.core.sha256 === hash);
      assert(row && row.target.startsWith('darwin/'), 'Unknown inert Mac binary');
      const display = row.signatureObservation.core.codeSignatureInspection;
      if (command === 'file') return 'Mach-O 64-bit executable x86_64';
      if (args[0] === '--display') return display;
      assert.deepEqual(args.slice(0, -1), ['--verify', '--strict']);
      if (display.includes('code object is not signed at all')) return { status: 1, stderr: display };
      return '';
    }
    assert(['python3', 'python'].includes(command), 'Target execution prohibited'); return execFileSync(command, args, options);
  };
  return { root, contract, manifest, transport, calls, read, run, base, publicRelease, inspections };
}
async function withFixture(fn, options) { const f = fixture(options); try { await fn(f); } finally { rmSync(f.root, { recursive: true, force: true }); } }
function resources(f) {
  const dir = join(f.root, 'resources'), result = {};
  function walk(p, relative = '') { if (!existsSync(p)) return; for (const n of readdirSync(p, { withFileTypes: true })) {
    const path = join(p, n.name), name = relative + n.name;
    if (n.isDirectory()) walk(path, name + '/'); else result[name] = readFileSync(path).toString('hex');
  } }
  walk(dir); return result;
}

test('all four release archives keep exact cores, colocated libraries, notices and separate adapter receipts', async () => withFixture(async (f) => {
  validateReleaseContract(f.contract); const receipts = await consumeRelease(f.root, f.manifest, undefined, f);
  assert.equal(Object.keys(receipts).length, 4);
  for (const [key, row] of Object.entries(f.contract.platforms)) {
    const path = join(f.root, 'resources', key, row.core.member);
    const receipt = verifyPackagedSource(f.root, f.manifest, key, path, f.run);
    assert.equal(receipt.schema, 'polaris-app-release-consumption-v1');
    assert.equal(receipt.producer.providerContract, 'polaris-box-release-v1');
    assert.equal(receipt.adapter.facts.candidate, candidate); assert.equal(receipt.producer.fixtureOnly, true);
    assert(existsSync(join(dirname(path), 'core-release/LICENSE')));
    if (row.sidecar) assert.equal(inspectNativePayload(f.root, key, path, join(dirname(path), row.sidecar.member), f.run).schema, 'polaris-native-release-payload-input-v1');
  }
  assert.equal(f.calls.filter((url) => url.includes('/releases/17')).length, 2);
  assert(f.calls.every((url) => !url.includes('proxy.golang.org') && !url.includes('raw.githubusercontent.com')));
  assert(!existsSync(join(f.root, 'resources/.source-receipts')));
}));
test('unknown/duplicate/CLI-only architecture selection rejects without resources writes', async () => withFixture(async (f) => {
  for (const keys of [['linux', 'linux'], ['linux-arm64'], []]) await assert.rejects(consumeRelease(f.root, f.manifest, keys, f));
  assert.deepEqual(resources(f), {}); assert.equal(f.calls.length, 0);
}));
test('subset update preserves other platforms and unrelated files; current bytes are rechecked', async () => withFixture(async (f) => {
  write(join(f.root, 'resources/win/keep'), 'other-platform'); write(join(f.root, 'resources/linux/helper'), 'unrelated');
  await consumeRelease(f.root, f.manifest, ['linux'], f);
  assert.equal(readFileSync(join(f.root, 'resources/win/keep'), 'utf8'), 'other-platform');
  assert.equal(readFileSync(join(f.root, 'resources/linux/helper'), 'utf8'), 'unrelated');
  const core = join(f.root, 'resources/linux/sing-box'); writeFileSync(core, Buffer.alloc(256));
  assert.throws(() => verifyPackagedRelease(f.root, f.manifest, 'linux', core, f.run), /bytes differ/);
}));
test('malformed explicit selection never falls back to old source/mobile pins', async () => withFixture(async (f) => {
  for (const value of [null, [], {}, { ...f.manifest.desktopRelease, contractSha256: '0'.repeat(64) },
    { ...f.manifest.desktopRelease, librarySha256: { linux: '0'.repeat(64), win: 'f'.repeat(64) } }]) {
    const manifest = { ...f.manifest, desktopRelease: value }; assert(hasDesktopRelease(manifest));
    assert.throws(() => loadReleaseContract(f.root, manifest));
    await assert.rejects(consumeRelease(f.root, manifest, ['linux'], f));
  }
  assert.deepEqual(resources(f), {});
}));
test('repository/tag/source/asset IDs, URL and digest mismatch each preserve prior resources', async () => withFixture(async (f) => {
  write(join(f.root, 'resources/linux/sing-box'), 'previous'); const before = resources(f);
  const mutations = [ [f.base, { id: 1, full_name: f.contract.repository }],
    [f.base + '/git/ref/tags/' + tag, { ref: 'refs/tags/' + tag, object: { type: 'commit', sha: tagObject } }],
    [f.base + '/git/tags/' + tagObject, { sha: tagObject, tag, object: { type: 'commit', sha: '9'.repeat(40) } }],
    [f.base + '/git/commits/' + commit, { sha: commit, tree: { sha: '9'.repeat(40) } }],
    ...['id', 'browser_download_url', 'digest', 'size'].map((key) => {
      const release = structuredClone(f.publicRelease); release.assets[0][key] = key === 'id' || key === 'size' ? 999 : 'https://example.invalid/wrong';
      return [f.base + '/releases/17', release];
    }) ];
  for (const [url, value] of mutations) {
    const old = f.transport.get(url); f.transport.set(url, encode(value));
    await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], f));
    assert.deepEqual(resources(f), before); f.transport.set(url, old);
  }
}));
test('truncated/replaced index or archive fails before replacing any core or library', async () => withFixture(async (f) => {
  write(join(f.root, 'resources/linux/sing-box'), 'previous'); const before = resources(f);
  for (const url of [f.contract.assetIndex.url, f.contract.platforms.linux.releaseAsset.url]) {
    const original = f.transport.get(url); f.transport.set(url, original.subarray(0, original.length - 1));
    await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], f)); assert.deepEqual(resources(f), before);
    f.transport.set(url, original);
  }
}));
test('public asset replacement during download aborts the entire update', async () => withFixture(async (f) => {
  write(join(f.root, 'resources/linux/sing-box'), 'previous'); const before = resources(f); let snapshots = 0;
  const read = async (url) => {
    if (url === f.base + '/releases/17' && ++snapshots === 2) {
      const mutated = structuredClone(f.publicRelease); mutated.assets[0].id++; return encode(mutated);
    }
    return f.read(url);
  };
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], { ...f, read })); assert.deepEqual(resources(f), before);
}));
test('rename fault after first platform rolls back core, libraries and receipts for the whole selection', async () => withFixture(async (f) => {
  for (const key of ['linux', 'win']) write(join(f.root, 'resources', key, 'previous'), key);
  write(join(f.root, 'resources/.release-receipts/previous.json'), 'old-receipt'); const before = resources(f); let moves = 0;
  const rename = (a, b) => { if (++moves === 4) throw new Error('fixture rename failure'); renameSync(a, b); };
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux', 'win'], { ...f, rename }), /fixture rename failure/);
  assert.deepEqual(resources(f), before);
}));
test('exclusive lock and linked resource path fail without following a link or overwriting prior files', async () => withFixture(async (f) => {
  write(join(f.root, 'resources/.release-consumer.lock'), 'existing');
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], f), /EEXIST/);
  assert.equal(readFileSync(join(f.root, 'resources/.release-consumer.lock'), 'utf8'), 'existing');
  rmSync(join(f.root, 'resources/.release-consumer.lock'));
  const outside = join(f.root, 'outside'); mkdirSync(outside); write(join(outside, 'original'), 'protected');
  symlinkSync(outside, join(f.root, 'resources/linux'), process.platform === 'win32' ? 'junction' : 'dir');
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], f), /Non-regular/);
  assert.equal(readFileSync(join(outside, 'original'), 'utf8'), 'protected');
}));
test('portable canonical fingerprints retain Python ASCII encoding including Unicode and non-BMP text', () => {
  const value = { z: '界😀\u007f', a: ['é', 0] };
  assert.equal(portableCanonical(value), execFileSync(PYTHON, ['-B', '-c', 'import json,sys;print(json.dumps(json.load(sys.stdin),sort_keys=True,separators=(",",":")))'], { input: JSON.stringify(value), encoding: 'utf8' }).trim());
});
test('release CLI parsing accepts exactly the four App architecture keys and rejects source-mode flags', () => {
  assert.deepEqual(releasePlatforms(['--platform=mac-arm64,linux', '--force']), ['mac-arm64', 'linux']);
  for (const args of [['--producer'], ['--bundle-dir=old'], ['--candidate=' + candidate], ['--platform=linux,,win'],
    ['--platform=linux', '--platform=win'], ['--force', '--force'], ['--platform=linux,'], ['--platform=linux-arm64']])
    assert.throws(() => releasePlatforms(args));
});
test('failed rollback retains the original backup and lock; explicit recovery refuses live owners and changed files', async () => withFixture(async (f) => {
  for (const key of ['linux', 'win']) write(join(f.root, 'resources', key, 'previous'), key);
  const before = resources(f); let moves = 0;
  const rename = (a, b) => { moves++; if (moves === 4 || moves === 5) throw new Error('fixture rollback fault'); renameSync(a, b); };
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux', 'win'], { ...f, rename }), /recovery retained/);
  const lock = join(f.root, 'resources/.release-consumer.lock'); assert(existsSync(lock));
  assert.throws(() => recoverRelease(f.root), /still active/);
  const path = join(f.root, 'resources/linux/sing-box'), original = readFileSync(path); writeFileSync(path, 'external edit');
  assert.throws(() => recoverRelease(f.root, { isAlive: () => false }), /changed/); assert(existsSync(lock));
  writeFileSync(path, original);
  assert.equal(recoverRelease(f.root, { isAlive: () => false }).restoredPreviousResources, true);
  assert.deepEqual(resources(f), before);
}));
test('producer fingerprint/tags/BuildID or module graph tampering fails closed', async () => withFixture(async (f) => {
  for (const change of [(r) => { r.providerFingerprint = '0'.repeat(64); },
    (r) => { r.buildID = 'wrong'; }, (r) => { r.portableSourceIdentity.facts.sourceOverlay = 1; },
    (r) => { r.tags += ',with_purego'; }, (r) => { r.signaturePolicy = 'trust everything'; },
    (r) => { r.releaseAsset.url = 'https://example.invalid/asset'; }]) {
    const contract = structuredClone(f.contract); change(contract.platforms.linux);
    assert.throws(() => validateReleaseContract(contract));
  }
  const run = (command, args, options) => command === 'go' && args[0] === 'tool' ? 'wrong-id' : f.run(command, args, options);
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], { ...f, run }), /BuildID differs/);
  assert.deepEqual(resources(f), {});
  const run2 = (command, args, options) => command === 'go' && args[0] === 'version'
    ? String(f.run(command, args, options)).replace('vcs.modified=false', 'vcs.modified=true') : f.run(command, args, options);
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], { ...f, run: run2 }), /BuildInfo/);
  assert.deepEqual(resources(f), {});
}));
test('final carrier retains original notice/source bytes as well as exact core and Cronet bytes', async () => withFixture(async (f) => {
  await consumeRelease(f.root, f.manifest, ['linux'], f);
  const carrier = join(f.root, 'carrier/linux'); cpSync(join(f.root, 'resources/linux'), carrier, { recursive: true });
  assert.equal(verifyPackagedRelease(f.root, f.manifest, 'linux', join(carrier, 'sing-box'), f.run).platform, 'linux');
  writeFileSync(join(carrier, 'core-release/LICENSE'), 'wrong license');
  assert.throws(() => verifyPackagedRelease(f.root, f.manifest, 'linux', join(carrier, 'sing-box'), f.run), /notice\/source\/provenance/);
}));

test('actual inventory CLI admits precisely the release members and rejects missing/extra files in static and carrier trees', async () => withFixture(async (f) => {
  cpSync(join(repo, 'scripts'), join(f.root, 'scripts'), { recursive: true });
  // Production now has a bound contract; this private fixture retains its own exact bytes.
  write(join(f.root, f.manifest.desktopRelease.contractPath), encode(f.contract));
  copyFileSync(join(repo, 'src-tauri/build.rs'), join(f.root, 'src-tauri/build.rs'));
  const count = Number(readFileSync(join(repo, 'src-tauri/build.rs'), 'utf8').match(/^const EXPECTED_SRS_COUNT: usize = (\d+);$/m)[1]);
  for (const name of ['LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md']) write(join(f.root, name), 'inert');
  for (let i = 0; i < count; i++) write(join(f.root, `resources/data/inert-${i}.srs`), 'inert');
  for (const name of ['index.html', 'assets/inert.js', 'licenses/inert.txt', 'favicon.ico', 'favicon.svg', 'apple-touch-icon-180x180.png'])
    write(join(f.root, 'resources/dashboard', name), 'inert');
  await consumeRelease(f.root, f.manifest, undefined, f);
  for (const [key, label] of [['linux', 'linux'], ['win', 'windows'], ['mac-arm64', 'macos-arm64'], ['mac-x64', 'macos-x64']]) {
    const conf = `tauri.${key === 'linux' ? 'linux' : key === 'win' ? 'windows' : key === 'mac-arm64' ? 'macos-arm64' : 'macos-x64'}.conf.json`;
    copyFileSync(join(repo, 'src-tauri', conf), join(f.root, 'src-tauri', conf));
    write(join(f.root, 'resources', key, key === 'win' ? 'polaris-helper.exe' : 'polaris-helper'), 'inert');
    if (key === 'win') write(join(f.root, 'resources/win/polaris-cleaner.exe'), 'inert');
    const carrier = join(f.root, 'inventory-carrier', key);
    const scopes = key === 'linux' ? [join(carrier, 'deb/usr/lib/Polaris'), join(carrier, 'appimage/Polaris.AppDir/usr/lib/Polaris')]
      : key === 'win' ? [carrier] : [join(carrier, 'macos/Polaris.app/Contents/Resources')];
    for (const scope of scopes) {
      cpSync(join(f.root, 'src-tauri/core-manifest.json'), join(scope, 'core-manifest.json'), { recursive: true });
      for (const name of ['LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md', `resources/${key}`, 'resources/data', 'resources/dashboard']) {
        mkdirSync(dirname(join(scope, '_up_', name)), { recursive: true });
        cpSync(join(f.root, name), join(scope, '_up_', name), { recursive: true });
      }
    }
    for (const [flags, directory] of [[['--static'], join(f.root, 'resources', key)],
      [['--root', carrier], join(scopes[0], '_up_/resources', key)]]) {
      const run = () => spawnSync(process.execPath, [join(f.root, 'scripts/verify-packaging.mjs'), 'inventory', '--label', label, ...flags], { encoding: 'utf8' });
      let result = run(); assert.equal(result.status, 0, result.stdout + result.stderr);
      assert.match(result.stdout, /release-member:core-release\/source\/SOURCE-INDEX.json=1/);
      const missing = join(directory, 'core-release/source/SOURCE-INDEX.json'), original = readFileSync(missing); rmSync(missing);
      result = run(); assert.equal(result.status, 1, result.stdout + result.stderr);
      assert.match(result.stderr, /release-member:core-release\/source\/SOURCE-INDEX.json.*实为 0 份/);
      write(missing, original);
      const extra = join(directory, 'core-release/source/unreviewed.json'); write(extra, 'unreviewed');
      result = run(); assert.equal(result.status, 1, result.stdout + result.stderr); assert.match(result.stderr, /未登记/); rmSync(extra);
      result = run(); assert.equal(result.status, 0, result.stdout + result.stderr);
    }
  }
}));

test('producer Mac display must yield a unique state/CDHash with no unsigned arm64 exception', () => withFixture(async (f) => {
  for (const display of [undefined, '', 'CDHash=invalid\n', 'sing-box: code object is not signed at all\n',
    `CDHash=${'4'.repeat(40)}\nCDHash=invalid\n`,
    `CDHash=${'4'.repeat(40)}\nCDHash=${'5'.repeat(40)}\n`]) {
    const contract = structuredClone(f.contract); contract.platforms['mac-arm64'].signatureObservation.core.codeSignatureInspection = display;
    assert.throws(() => validateReleaseContract(contract), /signature|Mac/);
  }
}));

test('Mac consumption and final bundle entry enforce strict native verification and bound CDHash without signing', async () => withFixture(async (f) => {
  const key = 'mac-arm64';
  const badRuns = [(command, args, options) => command === 'codesign' && args[0] === '--verify'
    ? { status: 1, stderr: 'code object is not signed at all' } : f.run(command, args, options),
  (command, args, options) => command === 'codesign' && args[0] === '--verify'
    ? { status: 1, stderr: 'invalid signature' } : f.run(command, args, options),
  (command, args, options) => command === 'codesign' && args[0] === '--display'
    ? `CDHash=${'5'.repeat(40)}\n` : f.run(command, args, options)];
  for (const run of badRuns) {
    await assert.rejects(consumeRelease(f.root, f.manifest, [key], { ...f, host: 'darwin', run }), /signature|CDHash/);
    assert.deepEqual(resources(f), {});
  }
  const receipts = await consumeRelease(f.root, f.manifest, [key], { ...f, host: 'darwin' });
  assert.deepEqual(receipts[key].macCodeSignature, { state: 'signed', cdHash: '4'.repeat(40) });
  const carrier = join(f.root, 'signature-carrier'); cpSync(join(f.root, 'resources', key), carrier, { recursive: true });
  const core = join(carrier, 'sing-box'), before = digest(readFileSync(core));
  assert.equal(verifyPackagedSource(f.root, f.manifest, key, core, f.run, 'darwin').platform, key);
  for (const run of badRuns) assert.throws(() => verifyPackagedSource(f.root, f.manifest, key, core, run, 'darwin'), /signature|CDHash/);
  const file = join(f.root, 'resources/.release-receipts', `${key}.json`), original = readFileSync(file);
  for (const signature of [undefined, { state: 'invalid', cdHash: '4'.repeat(40) }, { state: 'signed', cdHash: '5'.repeat(40) }, { state: 'unsigned', cdHash: null }]) {
    const value = JSON.parse(original); if (signature === undefined) delete value.macCodeSignature; else value.macCodeSignature = signature;
    const { schema, fingerprint, ...facts } = value; value.fingerprint = digest(portableCanonical(facts)); write(file, encode(value));
    assert.throws(() => verifyPackagedSource(f.root, f.manifest, key, core, f.run, 'darwin'), /signature|CDHash/);
  }
  write(file, original); assert.equal(digest(readFileSync(core)), before);
  assert(f.inspections.some(([command, flag, strict]) => command === 'codesign' && flag === '--verify' && strict === '--strict'));
  assert(f.inspections.every(([, ...args]) => !args.includes('--sign')));
}));

test('unsigned Intel release retains only the existing thin-x86_64 native exception', async () => withFixture(async (f) => {
  const key = 'mac-x64', receipts = await consumeRelease(f.root, f.manifest, [key], { ...f, host: 'darwin' });
  assert.deepEqual(receipts[key].macCodeSignature, { state: 'unsigned', cdHash: null });
  const core = join(f.root, 'resources', key, 'sing-box');
  assert.equal(verifyPackagedSource(f.root, f.manifest, key, core, f.run, 'darwin').platform, key);
  const run = (command, args, options) => command === 'file' ? 'Mach-O universal x86_64 arm64' : f.run(command, args, options);
  assert.throws(() => verifyPackagedSource(f.root, f.manifest, key, core, run, 'darwin'), /thin Intel/);
}, { unsignedIntel: true }));
test('actual fetch CLIs reject malformed release mode and check-only Cronet has no module proxy access', async () => withFixture(async (f) => {
  cpSync(join(repo, 'scripts'), join(f.root, 'scripts'), { recursive: true });
  // Production now has a bound contract; this private fixture retains its own exact bytes.
  write(join(f.root, f.manifest.desktopRelease.contractPath), encode(f.contract));
  let result = spawnSync(process.execPath, [join(f.root, 'scripts/fetch-cronet.mjs'), '--check-only'], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr); assert.match(result.stdout, /exclusively from the core archive/);
  for (const args of [['--producer'], ['--platform=linux,,win']]) {
    result = spawnSync(process.execPath, [join(f.root, 'scripts/fetch-core.mjs'), ...args], { encoding: 'utf8' });
    assert.equal(result.status, 1); assert.match(result.stderr, /Release mode|architecture selection/);
  }
  result = spawnSync(process.execPath, [join(f.root, 'scripts/fetch-core.mjs'), '--bundle-dir=old', '--candidate=' + candidate], { encoding: 'utf8' });
  assert.equal(result.status, 1); assert.match(result.stderr, /ENOENT|Non-regular/);
  write(join(f.root, 'src-tauri/core-manifest.json'), encode({ ...f.manifest, desktopRelease: null }));
  result = spawnSync(process.execPath, [join(f.root, 'scripts/fetch-cronet.mjs'), '--check-only'], { encoding: 'utf8' });
  assert.equal(result.status, 1); assert.match(result.stderr, /Explicit release selection incomplete/);
  assert.deepEqual(resources(f), {});
}));
test('release notice identities are section-local and independent from mobile; old formal approval is not transferable', async () => withFixture(async (f) => {
  const mobile = { android: { sourceCommit: 'a'.repeat(40), manifestSha256: 'b'.repeat(64) },
    apple: { sourceCommit: 'd'.repeat(40), manifestSha256: 'e'.repeat(64) } };
  const desktop = [`  - 桌面 sing-box v1.15.0-alpha.12 (GPLv3) — https://github.com/polaris-arch/polaris-box`,
    '上游基线源码：https://github.com/SagerNet/sing-box/tree/v1.15.0-alpha.12', `桌面发布构建：${version}`,
    `固定源码：https://github.com/polaris-arch/polaris-box/tree/${commit}`, `源码 tree：${tree}`,
    `固定消费标签：https://github.com/polaris-arch/polaris-box/tree/${tag}`, `消费标签对象：${tagObject}`,
    `消费合同 SHA-256：${f.manifest.desktopRelease.contractSha256}`, `公开资产 index SHA-256：${f.contract.assetIndex.sha256}`,
    `桌面发布：https://github.com/polaris-arch/polaris-box/releases/tag/${tag}`];
  const blocks = Object.entries(mobile).flatMap(([key, row]) => {
    const label = key === 'android' ? 'Android libbox 基线' : 'Apple libbox 基线';
    return [`  - ${label} v1.15.0-alpha.8 (GPLv3) — https://github.com/SagerNet/sing-box`,
      '上游基线源码：https://github.com/SagerNet/sing-box/tree/v1.15.0-alpha.8',
      `固定源码：https://github.com/SagerNet/sing-box/tree/${row.sourceCommit}`, `source manifest SHA-256：${row.manifestSha256}`];
  });
  const notice = [...desktop, ...blocks].join('\n'); validateReleaseNotice(notice, f.manifest, f.contract, mobile);
  for (const line of desktop.slice(1)) assert.throws(() => validateReleaseNotice(notice.replace(line, ''), f.manifest, f.contract, mobile));
  assert.throws(() => validateReleaseNotice(notice.replace('v1.15.0-alpha.8', 'v1.15.0-alpha.12'), f.manifest, f.contract, mobile));
  assert.throws(() => validateReleaseNotice(notice.replace(desktop[1], desktop[1] + '\n' + desktop[1]), f.manifest, f.contract, mobile));
  assert.throws(() => assertSourceFirstRelease((path) => { assert.equal(path, 'src-tauri/core-manifest.json'); return encode(f.manifest); }), /does not inherit/);
}));

// Inert fixture acquisition checks the native-attempt receipt transport only.
// It does not claim a real native download, signature or final carrier gate.
test('published native gate rejects wrong attempt, host, source, adapter and asset snapshots', () => withFixture(async (f) => {
  const context = releaseGateContext({ GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: 'polaris-arch/Polaris',
    GITHUB_RUN_ID: '123', GITHUB_RUN_ATTEMPT: '1' });
  assert.throws(() => releaseGateContext({}), /Actions/);
  const receipts = await consumeRelease(f.root, f.manifest, ['linux'], f);
  const receipt = receipts.linux;
  const facts = { schema: 'polaris-app-native-release-gate-v1', candidate: receipt.adapter.facts.candidate,
    context, contractSha256: f.manifest.desktopRelease.contractSha256, platform: 'linux', host: 'linux', arch: 'x64',
    receipt, readback: JSON.parse(readFileSync(join(f.root, 'resources/.release-receipts/public-readback.json'), 'utf8')),
    indexSha256: f.contract.assetIndex.sha256,
    inspection: 'download-extract-buildinfo-buildid-cronet-native-signature',
    actualAppAcceptance: 'pending load, gvisor/mixed, feature and final carrier' };
  const freeze = (value) => { const { fingerprint, ...rest } = value; return { ...rest, fingerprint: digest(portableCanonical(rest)) }; };
  const gate = freeze(facts);
  const verify = (value) => verifyNativeReleaseGate(value, f.contract, f.manifest.desktopRelease, receipt.adapter, context, 'linux');
  verify(gate);
  for (const mutate of [
    (g) => { g.context.attempt = '2'; }, (g) => { g.context.runId = '456'; },
    (g) => { g.host = 'win32'; }, (g) => { g.arch = 'arm64'; },
    (g) => { g.contractSha256 = '0'.repeat(64); }, (g) => { g.indexSha256 = '0'.repeat(64); },
    (g) => { g.candidate = '0'.repeat(40); }, (g) => { g.actualAppAcceptance = 'passed'; },
    (g) => { g.readback.after.sourceCommit = '0'.repeat(40); },
    (g) => { g.receipt.producer.sourceCommit = '0'.repeat(40); },
    (g) => { g.receipt.adapter.facts.fileSha256['scripts/fetch-core.mjs'] = '0'.repeat(64); },
    (g) => { g.receipt.publicReadback.assets.pop(); },
  ]) {
    const changed = structuredClone(gate); mutate(changed); assert.throws(() => verify(freeze(changed)));
  }
}));


test('cumulative strict Risk accepts only the verified published selection and exact manual run', async () => withFixture(async (f) => {
  const ref = 'refs/heads/collab/s5-android-signal-20261010';
  const env = { POLARIS_NATIVE_PAYLOAD_VALIDATION: '1', GITHUB_ACTIONS: 'true',
    GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_REPOSITORY: 'polaris-arch/Polaris',
    GITHUB_REF: ref, GITHUB_SHA: candidate, GITHUB_RUN_ID: '900', GITHUB_RUN_ATTEMPT: '1',
    GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/release-risk.yml@${ref}` };
  assert.deepEqual(validationContext(candidate, env, f.root), { runId: '900', attempt: '1' });
  for (const change of [{ POLARIS_NATIVE_PAYLOAD_VALIDATION: '0' }, { GITHUB_ACTIONS: 'false' },
    { GITHUB_EVENT_NAME: 'push' }, { GITHUB_REPOSITORY: 'other/Polaris' },
    { GITHUB_REF: 'refs/heads/main' }, { GITHUB_SHA: '0'.repeat(40) },
    { GITHUB_RUN_ATTEMPT: '0' }, { GITHUB_RUN_ID: '0' },
    { GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/ci.yml@${ref}` },
    { POLARIS_NO_KERNEL_RUN: '1' }]) {
    assert.throws(() => validationContext(candidate, { ...env, ...change }, f.root), /exact non-tag/);
  }
  const path = join(f.root, f.manifest.desktopRelease.contractPath), original = readFileSync(path);
  write(path, Buffer.concat([original, Buffer.from(' ')]));
  assert.throws(() => validationContext(candidate, env, f.root), /SHA|hash|differs/i);
  write(path, original);
  const manifestPath = join(f.root, 'src-tauri/core-manifest.json');
  const sourceOnly = structuredClone(f.manifest); delete sourceOnly.desktopRelease;
  write(manifestPath, encode(sourceOnly));
  assert.throws(() => validationContext(candidate, env, f.root));
}));


test('original native receipt and final assembly remain separately bound, with no provenance projection bypass', async () => withFixture(async (f) => {
  await consumeRelease(f.root, f.manifest, Object.keys(RELEASE_TARGETS), f);
  for (const key of Object.keys(RELEASE_TARGETS)) {
    const row = f.contract.platforms[key], target = RELEASE_TARGETS[key];
    const rootName = `polaris-box-${row.version}-${target.os}-${target.arch}`;
    const bytes = readFileSync(join(f.root, 'resources', key, 'core-release/provenance.json'));
    verifyReleaseProvenance(bytes, row, key);
    assert.throws(() => verifyReleaseProvenance(Buffer.concat([bytes, Buffer.from(' ')]), row, key), /provenance bytes/);
    const original = JSON.parse(bytes);
    for (const mutate of [
      (p) => { delete p.archive; }, (p) => { p.sourceCommit = '0'.repeat(40); },
      (p) => { p.unreviewed = true; }, (p) => { p.finalPackageCronetSmoke = row.finalPackageCronetSmoke; },
      (p) => { p.archive.name = 'other.tar.gz'; },
      (p) => { p.archive.members[`${rootName}/${target.core}`] = '0'.repeat(64); },
      (p) => { p.archive.members['../escape'] = '0'.repeat(64); },
      (p) => { p.archive.unreviewed = true; },
    ]) {
      const changed = structuredClone(original); mutate(changed);
      const raw = encode(changed), pin = structuredClone(row);
      // Rebind only this synthetic member to exercise semantics beyond the byte hash.
      pin.archive.members[`${rootName}/provenance.json`] = digest(raw);
      assert.throws(() => verifyReleaseProvenance(raw, pin, key), /lineage|provenance differs/);
    }
    for (const mutate of [(r) => { delete r.finalPackageCronetSmoke; },
      (r) => { r.finalPackageCronetSmoke.gvisorCompiled = false; },
      (r) => { r.finalPackageCronetSmoke.os = 'other'; },
      (r) => { r.finalPackageCronetSmoke.go = 'go1.27.1'; },
      (r) => { r.finalPackageCronetSmoke.linkage = 'unknown'; }]) {
      const changed = structuredClone(row); mutate(changed);
      assert.throws(() => verifyReleaseProvenance(bytes, changed, key), /smoke binding/);
    }
  }
}));
