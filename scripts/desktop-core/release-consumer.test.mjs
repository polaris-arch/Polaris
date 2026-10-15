// Synthetic bytes and mocked public HTTP/Go metadata. No Git init, HTTP, core or C API execution.
import { writeSelectedCoreManifest } from '../lib/core-identity.mjs';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync,
  rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { digest } from './source-graph.mjs';
import { ADAPTER_FILES, consumeRelease, hasDesktopRelease, loadReleaseContract, portableCanonical,
  RELEASE_TARGETS, publicBytes, releasePublicReader, recoverRelease, releasePlatforms, validateReleaseContract, validateReleaseNotice, verifyPackagedRelease,
  verifyReleaseProvenance, verifyMacPackagedCore } from './release-consumer.mjs';
import { verifyManagedMeshCore } from './managed-mesh-core.mjs';
import { inspectNativePayload } from '../native-payload-input.mjs';
import { releaseGateContext, verifyNativeReleaseGate } from './release-native-gates.mjs';
import { validationContext } from './validation-origin.mjs';
import { verifyPackagedSource } from './bundle.mjs';
import { coreWireHandoffName, createCoreWireHandoff, importCoreWireHandoff, packCoreWireHandoff, unpackCoreWireHandoff } from './wire-input-handoff.mjs';
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

test('actual 577d candidate source uses exact four App identities without acceptance claims', () => {
  const bytes = readFileSync(join(repo, 'scripts/desktop-core/release-contract.json'));
  assert.equal(bytes.length, 439026);
  assert.equal(digest(bytes), '0268c8ebfaf41f6b5976de7dec18deda6b5790eedc5c5c84a94704e36a8b4e9d');
  const actual = validateReleaseContract(JSON.parse(bytes));
  assert.equal(actual.sourceCommit, '577d4b9a089b71d2f0a4ca334b87264b55c10478');
  assert.equal(actual.tagObject, '5cedbe7a54594756da544af90ebfd1f403f2e091');
  assert.equal(actual.assetIndex.sha256, 'af5cb4704e1c4c216928d78d84af3c29322c7c1359a42eea9902c4a0823927d2');
  assert.deepEqual(Object.keys(actual.platforms).sort(), ['linux', 'mac-arm64', 'mac-x64', 'win']);
  const pending = JSON.parse(readFileSync(join(repo, 'scripts/desktop-core/pending-release-selection.json')));
  assert.equal(pending.active, false); assert.equal(pending.AppAccepted, false);
  assert.deepEqual(loadReleaseContract(repo, { desktopRelease: pending.selection }), actual);
  const active = JSON.parse(readFileSync(join(repo, 'src-tauri/core-manifest.json'))).desktopRelease;
  assert.equal(active.sourceCommit, actual.sourceCommit);
  const changed = structuredClone(pending.selection); changed.contractSha256 = 'a'.repeat(64);
  assert.throws(() => loadReleaseContract(repo, { desktopRelease: changed }), /SHA differs/);
  const changedVersion = structuredClone(actual); changedVersion.version = '1.15.0-alpha.12.polaris.1';
  assert.throws(() => validateReleaseContract(changedVersion));
});
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
      consumerAcceptance: 'pending', actualVersionOutput: `sing-box version ${version}\nRevision: ${commit}\n`, fixtureOnly: true };
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
members=json.load(open(sys.argv[1])); path=sys.argv[2]; zip_mode=int(sys.argv[3]); core_mode=int(sys.argv[4])
if path.endswith('.zip'):
 with zipfile.ZipFile(path,'w',zipfile.ZIP_DEFLATED) as z:
  for name,value in members.items():
   info=zipfile.ZipInfo(name);info.external_attr=((core_mode if name.endswith('/sing-box.exe') else 0) or zip_mode or (0o100755 if name.endswith('sing-box.exe') else 0o100644))<<16;z.writestr(info,base64.b64decode(value))
else:
 with tarfile.open(path,'w:gz') as z:
  for name,value in members.items():
   b=base64.b64decode(value);info=tarfile.TarInfo(name);info.size=len(b);info.mode=0o755 if name.endswith('/sing-box') else 0o644;z.addfile(info,io.BytesIO(b))
`, join(root, 'members.json'), archiveFile, String(options.windowsZipMode ?? 0), String(options.windowsCoreMode ?? 0)]);
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
  transport.set(base, encode({ id: 1407705262, full_name: contract.repository, private: false }));
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
test('Windows held-destination fixture preserves a continuous exclusive path while atomically replacing journals', async () => withFixture(async (f) => {
  const original = { openSync: fs.openSync, closeSync: fs.closeSync, renameSync: fs.renameSync };
  const held = new Map(); let journalReplacements = 0, checkedOwner = false;
  const lock = join(f.root, 'resources/.release-consumer.lock');
  fs.openSync = (...args) => { const fd = original.openSync(...args); held.set(fd, String(args[0])); return fd; };
  fs.closeSync = (fd) => { original.closeSync(fd); held.delete(fd); };
  fs.renameSync = (from, to) => {
    if ([...held.values()].includes(String(to))) throw Object.assign(Error('Windows fixture: open destination'), { code: 'EPERM' });
    if (String(to) === lock) { assert(existsSync(lock), 'journal replacement must not unlink the exclusive path'); journalReplacements++; }
    return original.renameSync(from, to);
  };
  syncBuiltinESMExports();
  try {
    mkdirSync(join(f.root, 'resources'), { recursive: true });
    // First show that this OS-semantics fixture rejects the real held-file case.
    const fd = fs.openSync(lock, 'wx'); const next = join(f.root, 'held-next'); write(next, 'inert');
    assert.throws(() => fs.renameSync(next, lock), { code: 'EPERM' }); fs.closeSync(fd); rmSync(lock); rmSync(next);
    const read = async (...args) => {
      if (!checkedOwner) {
        checkedOwner = true; assert(existsSync(lock));
        assert.throws(() => recoverRelease(f.root), /still active/);
        await assert.rejects(consumeRelease(f.root, f.manifest, ['win'], f), /EEXIST/);
      }
      return f.read(...args);
    };
    await consumeRelease(f.root, f.manifest, ['win'], { ...f, read });
    assert(checkedOwner); assert.equal(journalReplacements, 3); assert.equal(held.size, 0); assert(!existsSync(lock));
  } finally { Object.assign(fs, original); syncBuiltinESMExports(); }
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
    writeSelectedCoreManifest(join(f.root, 'resources', key), f.contract, key, '1.0.0-fixture');
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
  const guidance = ['`src-tauri/core-manifest.json` 的 `desktopRelease`', '`sourceDelivery` 指向固定公开来源。',
    `Desktop release: ${version} — https://github.com/polaris-arch/polaris-box/releases/tag/${tag}`,
    `Desktop source: https://github.com/polaris-arch/polaris-box/tree/${commit}`,
    `Desktop release contract: scripts/desktop-core/release-contract.json (SHA-256 ${f.manifest.desktopRelease.contractSha256})`,
    `Published asset index SHA-256: ${f.contract.assetIndex.sha256}`,
    'Desktop selection uses src-tauri/core-manifest.json desktopRelease.',
    'asset index sourceDelivery identifies the automatic product source archive,'];
  const notice = [...desktop, ...blocks, ...guidance].join('\n'); validateReleaseNotice(notice, f.manifest, f.contract, mobile);
  for (const line of guidance) assert.throws(() => validateReleaseNotice(notice.replace(line, ''), f.manifest, f.contract, mobile));
  for (const stale of ['desktopSourceBuild', 'scripts/desktop-core/source-manifest.json', 'Desktop v1.15.0-alpha.11',
    'Final native kernels, signatures, source archives remain pending']) {
    assert.throws(() => validateReleaseNotice(notice + '\n' + stale, f.manifest, f.contract, mobile), /obsolete desktop/);
  }
  assert.throws(() => validateReleaseNotice(notice.replace(guidance[2], guidance[2] + '\n' + guidance[2]), f.manifest, f.contract, mobile));
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


for (const ref of ['refs/heads/collab/s5-android-signal-20261010', 'refs/heads/collab/ui01-status-20261010'])
  test(`cumulative strict Risk ${ref} accepts only published selection and exact manual run`, async () => withFixture(async (f) => {
  const env = { POLARIS_NATIVE_PAYLOAD_VALIDATION: '1', GITHUB_ACTIONS: 'true',
    GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_REPOSITORY: 'polaris-arch/Polaris',
    GITHUB_REF: ref, GITHUB_SHA: candidate, GITHUB_RUN_ID: '900', GITHUB_RUN_ATTEMPT: '1',
    GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/release-risk.yml@${ref}` };
  assert.deepEqual(validationContext(candidate, env, f.root), { runId: '900', attempt: '1' });
  // Execute the actual Package Bash prefix and actual Python admission. These
  // fixtures never run a kernel, loader, installer or network request.
  copyFileSync(join(repo, 'scripts/desktop-core/validation-origin.mjs'), join(f.root, 'scripts/desktop-core/validation-origin.mjs'));
  const workflow = readFileSync(join(repo, '.github/workflows/package.yml'), 'utf8');
  const body = workflow.match(/      - name: Resolve platform matrix[\s\S]*?        run: \|\n([\s\S]*?)          # 全量矩阵/)[1];
  const bash = body.split('\n').map((line) => line.startsWith('          ') ? line.slice(10) : line).join('\n');
  const setupEnv = { ...process.env, ...env, POLARIS_NO_KERNEL_RUN: '0', CORE_CANDIDATE: candidate,
    EXPECTED_CANDIDATE: candidate, PLATFORMS_JSON: '["linux","windows"]',
    CORE_BUNDLE_ARTIFACT: `desktop-core-bundle-${candidate}-900-1`, CORE_RUN_ID: '900', CORE_RUN_ATTEMPT: '1',
    VALIDATION_KERNEL: 'true', VALIDATION_UPLOAD: 'true', VALIDATION_LIGHT: 'false', VALIDATION_SKIP_QUALITY: 'true' };
  const runBash = (changes = {}) => spawnSync('bash', ['-c', bash], { cwd: f.root, env: { ...setupEnv, ...changes }, encoding: 'utf8' });
  const python = `import importlib.util,json,sys
spec=importlib.util.spec_from_file_location('native',sys.argv[1]);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
print(json.dumps(m.ci_context('linux',json.loads(sys.argv[2]),'Linux','amd64',root=sys.argv[3])))`;
  const runPython = (changes = {}) => spawnSync(PYTHON, ['-B', '-c', python,
    join(repo, 'scripts/native-payload-proof.py'), JSON.stringify({ ...env, ...changes }), f.root], { encoding: 'utf8' });
  for (const run of [runBash, runPython]) {
    let result = run(); assert.equal(result.status, 0, result.stdout + result.stderr);
    for (const change of [{ POLARIS_NATIVE_PAYLOAD_VALIDATION: '0' }, { GITHUB_ACTIONS: 'false' },
      { GITHUB_EVENT_NAME: 'push' }, { GITHUB_REPOSITORY: 'other/Polaris' }, { GITHUB_REF: 'refs/heads/main' },
      { GITHUB_REF: 'refs/tags/v1' }, { GITHUB_SHA: 'invalid' }, { GITHUB_RUN_ID: '0' }, { GITHUB_RUN_ATTEMPT: '0' },
      { GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/ci.yml@${ref}` }, { POLARIS_NO_KERNEL_RUN: '1' }]) {
      // Native-validation=false is ordinary Package setup, so admission is not
      // invoked there; Python must still reject it.
      if (run === runBash && change.POLARIS_NATIVE_PAYLOAD_VALIDATION === '0') continue;
      assert.notEqual(run(change).status, 0, JSON.stringify(change));
    }
  }
  for (const change of [{ CORE_CANDIDATE: '0'.repeat(40) }, { EXPECTED_CANDIDATE: '0'.repeat(40) },
    { CORE_BUNDLE_ARTIFACT: 'desktop-core-bundle-wrong' }, { CORE_RUN_ID: '901' }, { CORE_RUN_ATTEMPT: '2' },
    { PLATFORMS_JSON: '["linux"]' }, { VALIDATION_KERNEL: 'false' }, { VALIDATION_UPLOAD: 'false' },
    { VALIDATION_LIGHT: 'true' }, { VALIDATION_SKIP_QUALITY: 'false' }]) assert.notEqual(runBash(change).status, 0, JSON.stringify(change));

  for (const change of [{ POLARIS_NATIVE_PAYLOAD_VALIDATION: '0' }, { GITHUB_ACTIONS: 'false' },
    { GITHUB_EVENT_NAME: 'push' }, { GITHUB_REPOSITORY: 'other/Polaris' },
    { GITHUB_REF: 'refs/heads/main' }, { GITHUB_REF: 'refs/heads/collab/ui01-unapproved' }, { GITHUB_SHA: '0'.repeat(40) },
    { GITHUB_RUN_ATTEMPT: '0' }, { GITHUB_RUN_ID: '0' },
    { GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/ci.yml@${ref}` },
    { POLARIS_NO_KERNEL_RUN: '1' }]) {
    assert.throws(() => validationContext(candidate, { ...env, ...change }, f.root), /exact non-tag/);
  }
  const path = join(f.root, f.manifest.desktopRelease.contractPath), original = readFileSync(path);
  write(path, Buffer.concat([original, Buffer.from(' ')]));
  assert.throws(() => validationContext(candidate, env, f.root), /SHA|hash|differs/i);
  assert.notEqual(runBash().status, 0); assert.notEqual(runPython().status, 0);
  write(path, original);
  const manifestPath = join(f.root, 'src-tauri/core-manifest.json');
  const sourceOnly = structuredClone(f.manifest); delete sourceOnly.desktopRelease;
  write(manifestPath, encode(sourceOnly));
  assert.throws(() => validationContext(candidate, env, f.root));
  assert.notEqual(runBash().status, 0); assert.notEqual(runPython().status, 0);
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

test('selected managed mesh identity binds original release bytes, receipt and full version observation', async () => withFixture(async f => {
  await consumeRelease(f.root,f.manifest,['linux'],f);
  const core=join(f.root,'resources/linux/sing-box'), row=f.contract.platforms.linux; let versions=0;
  const run=(command,args,options)=>{if(command===core){assert.deepEqual(args,['version']);versions++;return row.actualVersionOutput;}return f.run(command,args,options);};
  const id=verifyManagedMeshCore(f.root,f.manifest,core,run,{os:'linux',arch:'x64'});
  assert.equal(id.sourceCommit,commit);assert.equal(id.sourceTree,tree);assert.equal(id.coreSha256,row.core.sha256);
  assert.equal(id.providerFingerprint,row.providerFingerprint);assert.equal(id.buildID,row.buildID);assert.equal(versions,1);
}));
test('selected managed mesh wrong core bytes fail before executing version', async () => withFixture(async f => {
  await consumeRelease(f.root,f.manifest,['linux'],f);
  const core=join(f.root,'resources/linux/sing-box'), bytes=readFileSync(core);bytes[220]^=1;writeFileSync(core,bytes);let versions=0;
  const run=(command,args,options)=>{if(command===core){versions++;return f.contract.platforms.linux.actualVersionOutput;}return f.run(command,args,options);};
  assert.throws(()=>verifyManagedMeshCore(f.root,f.manifest,core,run,{os:'linux',arch:'x64'}),/bytes differ/);assert.equal(versions,0);
}));
test('selected managed mesh wrong Revision and nonzero version both fail after strict metadata verification', async () => withFixture(async f => {
  await consumeRelease(f.root,f.manifest,['linux'],f);const core=join(f.root,'resources/linux/sing-box'), row=f.contract.platforms.linux;
  for(const fail of ['revision','status']){let versions=0;
   const run=(command,args,options)=>{if(command===core){versions++;if(fail==='status')throw Object.assign(Error('version status 7'),{status:7});return row.actualVersionOutput.replace(`Revision: ${commit}`,`Revision: ${'9'.repeat(40)}`);}return f.run(command,args,options);};
   assert.throws(()=>verifyManagedMeshCore(f.root,f.manifest,core,run,{os:'linux',arch:'x64'}),fail==='status'?/version status 7/:/version output differs/);assert.equal(versions,1);
  }
}));
test('selected managed mesh refuses foreign hosts before inspecting or executing a core', () => {
  for(const host of [{os:'win32',arch:'x64'},{os:'linux',arch:'arm64'}])assert.throws(()=>verifyManagedMeshCore('/absent',{},'/absent',()=>assert.fail('foreign execution'),host),/native Linux amd64/);
});


test('explicit Windows target sanitizes only frozen regular0666 and exact sing-box.exe0777 into safe private copies', async () => withFixture(async (f) => {
  const row = f.contract.platforms.win, core = join(f.root, 'resources/win/sing-box.exe');
  const wrongTarget = join(f.root, 'wrong-target-unpacked');
  const rejected = spawnSync(PYTHON, ['-B', join(repo, 'scripts/desktop-core/release-archive.py'),
    join(f.root, 'inputs', row.archive.name), wrongTarget], { encoding: 'utf8',
    input: JSON.stringify({ archive: row.archive, root: row.archive.name.slice(0, -4), target: 'linux/amd64' }) });
  assert.notEqual(rejected.status, 0); assert(!existsSync(wrongTarget));
  await consumeRelease(f.root, f.manifest, ['win'], f);
  assert.equal(digest(readFileSync(core)), row.core.sha256);
  assert.equal(digest(readFileSync(join(f.root, 'resources/win/libcronet.dll'))), row.sidecar.sha256);
  // Windows chmod exposes only the read-only flag; POSIX can observe all bits.
  if (process.platform !== 'win32') {
    assert.equal(fs.statSync(core).mode & 0o777, 0o755);
    assert.equal(fs.statSync(join(f.root, 'resources/win/libcronet.dll')).mode & 0o777, 0o644);
  }
  assert.equal(verifyPackagedRelease(f.root, f.manifest, 'win', core, f.run).platform, 'win');
}, { windowsZipMode: 0o100666, windowsCoreMode: 0o100777 }));
test('Windows permission adaptation still rejects symlinks, setuid and non-core0777 modes before writes', async () => {
  for (const mode of [0o120666, 0o104666, 0o100777]) await withFixture(async (f) => {
    await assert.rejects(consumeRelease(f.root, f.manifest, ['win'], f));
    assert.deepEqual(resources(f), {});
  }, { windowsZipMode: mode });
});
test('dedicated Actions token is API-only while ambient tokens, assets and redirects stay anonymous', async () => {
  const calls = [], env = { GITHUB_ACTIONS: 'true', POLARIS_RELEASE_API_TOKEN: 'inert-fixture-token', GH_TOKEN: 'inert-gh-token' };
  const previous = Object.fromEntries(Object.keys(env).map((key) => [key, process.env[key]]));
  Object.assign(process.env, env);
  try {
  const fetch = async (url, options) => { calls.push({ url, ...options }); return new Response('ok'); };
  await publicBytes('https://api.github.com/repos/polaris-arch/polaris-box/releases/17', 2, false, { fetch, env });
   assert.equal(calls.pop().headers.Authorization, 'Bearer inert-fixture-token');
   for (const outside of [{ GITHUB_ACTIONS: 'false', POLARIS_RELEASE_API_TOKEN: 'inert-fixture-token' },
     { GITHUB_ACTIONS: 'true', GH_TOKEN: 'ignored-fixture-token', GITHUB_TOKEN: 'ignored-fixture-token' }]) {
     await publicBytes('https://api.github.com/repos/polaris-arch/polaris-box', 2, false, { fetch, env: outside });
     assert.equal(calls.pop().headers.Authorization, undefined);
   }
   for (const url of ['https://api.github.com/repos/other/repo',
     'https://api.github.com/repos/polaris-arch/polaris-box?secret=fixture',
     'https://api.github.com/repos/polaris-arch/polaris-box#fragment',
     'https://fixture-user@api.github.com/repos/polaris-arch/polaris-box',
     'http://api.github.com/repos/polaris-arch/polaris-box']) {
     await publicBytes(url, 2, false, { fetch, env });
     assert.equal(calls.pop().headers.Authorization, undefined);
   }
  const assetFetch = async (url, options) => { calls.push({ url, ...options });
    return url.includes('github.com/') ? new Response(null, { status: 302,
      headers: { Location: 'https://release-assets.githubusercontent.com/inert?signature=fixture' } }) : new Response('ok'); };
  await publicBytes('https://github.com/polaris-arch/polaris-box/releases/download/inert/core.zip', 2, true, { fetch: assetFetch, env });
  assert.equal(calls.length, 2); assert(calls.every((call) => call.headers.Authorization === undefined));
  assert(calls.every((call) => call.redirect === 'manual'));
  } finally { for (const [key, value] of Object.entries(previous)) {
    if (value === undefined) delete process.env[key]; else process.env[key] = value;
  } }
});
test('HTTP denial records phase, host, request-id and quota but neither credentials nor query and never blindly retries', async () => {
  const env = { GITHUB_ACTIONS: 'true', POLARIS_RELEASE_API_TOKEN: 'never-log-fixture-secret' }; let requests = 0;
  const fetch = async () => { requests++; return new Response('failure', { status: 403,
    headers: { 'x-github-request-id': 'ABCD:1234', 'x-ratelimit-limit': '60', 'x-ratelimit-remaining': '0',
      'x-ratelimit-reset': '123', 'x-ratelimit-resource': 'core', 'retry-after': '7' } }); };
  await assert.rejects(publicBytes('https://api.github.com/repos/polaris-arch/polaris-box?secret=fixture-query', 10, false, { fetch, env }),
    (error) => /Release HTTP 403/.test(error.message) && error.message.includes('stage=public-api host=api.github.com')
      && error.message.includes('x-github-request-id=ABCD:1234') && error.message.includes('x-ratelimit-limit=60')
      && error.message.includes('x-ratelimit-remaining=0') && error.message.includes('x-ratelimit-reset=123')
      && error.message.includes('x-ratelimit-resource=core') && error.message.includes('retry-after=7')
      && !error.message.includes(env.POLARIS_RELEASE_API_TOKEN) && !error.message.includes('fixture-query'));
  assert.equal(requests, 1);
});
test('transport errors and signed asset denials expose only safe phase-host diagnostics', async () => {
  await assert.rejects(publicBytes('https://github.com/inert?token=never-log-query', 10, true,
    { fetch: async () => { throw new TypeError('credentials=never-log-error'); } }),
    (error) => error.message.includes('stage=asset host=github.com')
      && !error.message.includes('never-log-query') && !error.message.includes('never-log-error'));
  let requests = 0;
  const fetch = async () => ++requests === 1 ? new Response(null, { status: 302,
    headers: { Location: 'https://release-assets.githubusercontent.com/inert?token=never-log-signed-query' } })
    : new Response('denied', { status: 403, headers: { 'x-github-request-id': 'EF01:5678' } });
  await assert.rejects(publicBytes('https://github.com/inert', 10, true, { fetch }),
    (error) => error.message.includes('stage=asset-redirect host=release-assets.githubusercontent.com')
      && error.message.includes('x-github-request-id=EF01:5678') && !error.message.includes('never-log-signed-query'));
  assert.equal(requests, 2);
});
test('repository public identity remains mandatory without relying on authenticated visibility', async () => withFixture(async (f) => {
  write(join(f.root, 'resources/linux/sing-box'), 'previous'); const before = resources(f);
  f.transport.set(f.base, encode({ id: 1407705262, full_name: f.contract.repository, private: true }));
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], f), /Repository identity differs/);
  assert.deepEqual(resources(f), before);
}));

const quotaNow = 1791755097242;
const transportURL = 'https://api.github.com/repos/polaris-arch/polaris-box';
const codedTransportError = (code) => new TypeError('never-log-outer-secret', {
  cause: Object.assign(new Error('never-log-cause-address-or-query'), { code }) });
test('recognized transient API transport gets one same-URL anonymous retry shared across all reads', async () => {
  const calls = [], waits = [], reports = [];
  const read = releasePublicReader({ now: () => quotaNow, deadlineMs: quotaNow + 60000,
    sleep: async (ms) => waits.push(ms), report: (s) => reports.push(s),
    fetch: async (url, options) => { calls.push({ url, options });
      if (calls.length !== 2) throw codedTransportError('ECONNRESET'); return new Response('ok'); } });
  assert.equal((await read(transportURL, 2)).toString(), 'ok');
  assert.deepEqual(waits, [1000]); assert.equal(calls.length, 2);
  assert.deepEqual(calls.map((call) => call.url), [transportURL, transportURL]);
  assert(calls.every((call) => call.options.redirect === 'manual' && call.options.headers.Authorization === undefined));
  assert.equal(reports.length, 2); assert(reports[0].includes('codes=ECONNRESET; causeComplete=true'));
  assert(reports[1].includes('Release API observation') && reports[1].includes('apiAuth=anonymous'));
  assert(!reports[0].includes('never-log'));
  await assert.rejects(read(transportURL + '/releases/17', 2), /codes=ECONNRESET/);
  assert.equal(calls.length, 3); assert.equal(waits.length, 1);
});
test('unknown, terminal, mixed, cyclic, clipped and cancelled transport errors fail without retry or raw causes', async () => {
  const cycle = codedTransportError('ECONNRESET'); cycle.cause.cause = cycle;
  let deep = codedTransportError('ECONNRESET'); for (let i = 0; i < 6; i++) deep = new Error('never-log-depth', { cause: deep });
  const errors = [new TypeError('never-log-unknown'), codedTransportError('never-log-unknown-code'),
    ...['ENOTFOUND', 'ECONNREFUSED', 'ERR_TLS_CERT_ALTNAME_INVALID', 'CERT_HAS_EXPIRED', 'EPERM'].map(codedTransportError),
    new AggregateError([Object.assign(Error('never-log-a'), { code: 'ECONNRESET' }), Error('never-log-b')]),
    new AggregateError([Object.assign(Error('never-log-a'), { code: 'ECONNRESET' }),
      Object.assign(Error('never-log-b'), { code: 'ENOTFOUND' })]), cycle, deep,
    new AggregateError(Array.from({ length: 17 }, () => codedTransportError('ECONNRESET'))),
    new Error('never-log-wrapper', { cause: Object.assign(Error('never-log-abort'), { name: 'AbortError', code: 'ECONNRESET' }) }),
    Object.assign(Error('never-log-timeout'), { name: 'TimeoutError', code: 'ETIMEDOUT' })];
  errors.push(Object.defineProperty(Error('never-log-getter'), 'code', { get() { throw Error('never-log-property'); } }));
  for (const error of errors) {
    let calls = 0, waits = 0;
    const read = releasePublicReader({ now: () => quotaNow, deadlineMs: quotaNow + 60000,
      fetch: async () => { calls++; throw error; }, sleep: async () => { waits++; }, report: () => assert.fail('unexpected recovery') });
    await assert.rejects(read(transportURL + '?secret=never-log-query', 2),
      (failure) => failure.message.includes('Release request failed') && !failure.message.includes('never-log'));
    assert.equal(calls, 1); assert.equal(waits, 0);
    // The error itself must refuse recovery even on a canonical endpoint.
    await assert.rejects(read(transportURL, 2), (failure) => !failure.message.includes('never-log'));
    assert.equal(calls, 2); assert.equal(waits, 0);
  }
});
test('transport diagnostic reads each code and root name once, keeping its closed vocabulary', async () => {
  let codeReads = 0, nameReads = 0;
  const error = Object.defineProperties(new Error('never-log-message'), {
    name: { get: () => ++nameReads === 1 ? 'TypeError' : 'never-log-name' },
    code: { get: () => ++codeReads === 1 ? 'ECONNRESET' : 'never-log-code' } });
  await assert.rejects(publicBytes(transportURL, 2, false, { fetch: async () => { throw error; } }),
    (failure) => failure.message.includes('codes=ECONNRESET') && !failure.message.includes('never-log'));
  assert.equal(codeReads, 1);
  assert.equal(nameReads, 1);
});
test('transient transport does not retry assets, queries, credentials, fragments or another repository', async () => {
  for (const [url, asset] of [[transportURL, true], [transportURL + '?signature=never-log-query', false],
    [transportURL + '#fragment', false], [transportURL.replace('https://', 'https://fixture-user@'), false],
    ['https://api.github.com/repos/other/repository', false], [transportURL.replace('https:', 'http:'), false]]) {
    let calls = 0;
    const read = releasePublicReader({ now: () => quotaNow, deadlineMs: quotaNow + 60000,
      fetch: async () => { calls++; throw codedTransportError('EAI_AGAIN'); }, sleep: async () => assert.fail('unexpected wait') });
    await assert.rejects(read(url, 2, asset), /codes=EAI_AGAIN/); assert.equal(calls, 1);
  }
});
test('transport retry stops at second error and cannot exceed original absolute deadline', async () => {
  let calls = 0, waits = 0;
  const fetch = async () => { calls++; throw codedTransportError('UND_ERR_CONNECT_TIMEOUT'); };
  const read = releasePublicReader({ now: () => quotaNow, deadlineMs: quotaNow + 60000, fetch,
    sleep: async () => { waits++; }, report: () => {} });
  await assert.rejects(read(transportURL, 2), /codes=UND_ERR_CONNECT_TIMEOUT/);
  assert.equal(calls, 2); assert.equal(waits, 1);
  calls = 0; waits = 0;
  const short = releasePublicReader({ now: () => quotaNow, deadlineMs: quotaNow + 1000, fetch,
    sleep: async () => { waits++; }, report: () => {} });
  await assert.rejects(short(transportURL, 2), /Release request failed/); assert.equal(calls, 1); assert.equal(waits, 0);
  let clock = quotaNow;
  const late = releasePublicReader({ now: () => clock, deadlineMs: quotaNow + 2000, fetch,
    sleep: async () => { clock += 2001; }, report: () => {} });
  await assert.rejects(late(transportURL, 2), /job deadline exceeded/); assert.equal(calls, 2);
});
test('transport recovery still rejects size and changed public identity without replacing resources', async () => withFixture(async (f) => {
  let calls = 0;
  const fetch = async () => { if (++calls === 1) throw codedTransportError('ECONNRESET'); return new Response('exceeds pin'); };
  const options = { now: () => quotaNow, deadlineMs: quotaNow + 60000, sleep: async () => {}, report: () => {} };
  await assert.rejects(releasePublicReader({ ...options, fetch })(transportURL, 2), /exceeds pinned size/); assert.equal(calls, 2);
  write(join(f.root, 'resources/linux/sing-box'), 'previous'); const before = resources(f); calls = 0;
  const changed = releasePublicReader({ ...options, fetch: async () => {
    if (++calls === 1) throw codedTransportError('ECONNRESET');
    return new Response(encode({ id: 1407705262, full_name: f.contract.repository, private: true })); } });
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], { ...f, read: changed }), /Repository identity differs/);
  assert.equal(calls, 2); assert.deepEqual(resources(f), before);
}));
test('body-stream errors never retry or write partial bytes and sanitize the cause', async () => withFixture(async (f) => {
  write(join(f.root, 'resources/linux/sing-box'), 'previous'); const before = resources(f); let calls = 0;
  const read = releasePublicReader({ now: () => quotaNow, deadlineMs: quotaNow + 60000,
    sleep: async () => assert.fail('body must not retry'), fetch: async () => { calls++;
      return { ok: true, status: 200, headers: new Headers(), body: (async function* () {
        yield Buffer.from('partial'); throw codedTransportError('ECONNRESET'); })() }; } });
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], { ...f, read }),
    (error) => /Release body read failed/.test(error.message) && error.message.includes('codes=ECONNRESET') && !error.message.includes('never-log'));
  assert.equal(calls, 1); assert.deepEqual(resources(f), before);
}));
test('response cancellation failures are bounded diagnostics without recovery', async () => {
  let calls = 0;
  const read = releasePublicReader({ now: () => quotaNow, deadlineMs: quotaNow + 60000,
    fetch: async () => { calls++; return { ok: false, status: 403, headers: new Headers(),
      body: { cancel: async () => { throw codedTransportError('UND_ERR_SOCKET'); } } }; },
    sleep: async () => assert.fail('cancel must not retry') });
  await assert.rejects(read(transportURL, 2),
    (error) => /Release body cancellation failed/.test(error.message) && !error.message.includes('never-log'));
  assert.equal(calls, 1);
});
const observedQuotaHeaders = { 'x-github-request-id': 'E3C0:358CB1:1F5A584:2541386:6ACC0359',
  'x-ratelimit-limit': '60', 'x-ratelimit-remaining': '0', 'x-ratelimit-reset': '1791755597',
  'x-ratelimit-resource': 'core' };
const shortQuotaHeaders = { ...observedQuotaHeaders, 'x-ratelimit-reset': String(Math.floor(quotaNow / 1000) + 30) };
test('short evidenced primary quota waits beyond reset once and retries the identical anonymous request', async () => {
  const calls = [], waits = [], reports = [];
  const read = releasePublicReader({ deadlineMs: quotaNow + 120 * 60000, now: () => quotaNow, sleep: async (ms) => waits.push(ms), report: (s) => reports.push(s),
    fetch: async (url, options) => { calls.push({ url, options });
      return calls.length === 2 ? new Response('ok') : new Response('denied', { status: 403, headers: shortQuotaHeaders }); } });
  const url = 'https://api.github.com/repos/polaris-arch/polaris-box';
  assert.equal((await read(url, 2)).toString(), 'ok');
  assert.deepEqual(waits, [30758]); assert.equal(calls.length, 2);
  assert.deepEqual(calls.map((c) => c.url), [url, url]);
  assert(calls.every((c) => c.options.headers.Authorization === undefined && c.options.redirect === 'manual'));
  assert.equal(reports.length, 2); assert(reports[0].includes('Release primary quota wait 30758ms'));
  assert(reports[1].includes('Release API observation'));
  // Both snapshots and subsequent endpoints share the exhausted one-retry budget.
  await assert.rejects(read(url + '/releases/17', 10), /Release HTTP 403/);
  assert.equal(calls.length, 3); assert.equal(waits.length, 1);
});
test('a second quota denial is terminal without another wait or request', async () => {
  let calls = 0; const waits = [];
  const read = releasePublicReader({ deadlineMs: quotaNow + 120 * 60000, now: () => quotaNow, sleep: async (ms) => waits.push(ms), report: () => {},
    fetch: async () => { calls++; return new Response('denied', { status: 429, headers: shortQuotaHeaders }); } });
  await assert.rejects(read('https://api.github.com/repos/polaris-arch/polaris-box', 10), /Release HTTP 429/);
  assert.equal(calls, 2); assert.deepEqual(waits, [30758]);
});
test('unknown, expired, excessive, asset and noncanonical denials never consume a retry', async () => {
  const base = 'https://api.github.com/repos/polaris-arch/polaris-box';
  const cases = [
    { status: 401 }, { status: 404 }, { status: 500 }, { asset: true },
    { url: 'https://github.com/repos/polaris-arch/polaris-box' },
    { url: 'http://api.github.com/repos/polaris-arch/polaris-box' },
    { url: base + '?signature=never-log-query' }, { url: base.replace('https://', 'https://fixture-user@') },
    { url: 'https://api.github.com/repos/other/repository' },
    { headers: { 'x-ratelimit-limit': '5000' } }, { headers: { 'x-ratelimit-remaining': '1' } },
    { headers: { 'x-ratelimit-resource': 'search' } }, { headers: { 'x-ratelimit-reset': '' } },
    { headers: { 'x-ratelimit-reset': 'NaN' } }, { headers: { 'x-ratelimit-reset': '1791755097' } },
    { headers: { 'x-ratelimit-reset': String(Math.floor(quotaNow / 1000) + 3661) } },
    { headers: { 'x-ratelimit-reset': '99999999999999999999' } },
    { headers: { 'retry-after': '3661' } }, { headers: { 'retry-after': 'bad' } },
  ];
  for (const c of cases) {
    let calls = 0, waits = 0;
    const read = releasePublicReader({ deadlineMs: quotaNow + 120 * 60000, now: () => quotaNow, sleep: async () => { waits++; }, report: () => {},
      fetch: async () => { calls++; return new Response('denied', { status: c.status ?? 403,
        headers: { ...shortQuotaHeaders, ...c.headers } }); } });
    await assert.rejects(read(c.url ?? base, 10, c.asset ?? false), /Release HTTP/);
    assert.equal(calls, 1, JSON.stringify(c)); assert.equal(waits, 0, JSON.stringify(c));
  }
});
test('quota recovery still enforces pinned response size and public identity without replacing resources', async () => withFixture(async (f) => {
  let calls = 0;
  const read = releasePublicReader({ deadlineMs: quotaNow + 120 * 60000, now: () => quotaNow, sleep: async () => {}, report: () => {},
    fetch: async () => ++calls === 1 ? new Response('denied', { status: 403, headers: shortQuotaHeaders })
      : new Response('larger than the pin') });
  await assert.rejects(read(f.base, 2), /exceeds pinned size/); assert.equal(calls, 2);
  write(join(f.root, 'resources/linux/sing-box'), 'previous'); const before = resources(f); calls = 0;
  const changed = releasePublicReader({ deadlineMs: quotaNow + 120 * 60000, now: () => quotaNow, sleep: async () => {}, report: () => {},
    fetch: async () => ++calls === 1 ? new Response('denied', { status: 403, headers: shortQuotaHeaders })
      : new Response(encode({ id: 1407705262, full_name: f.contract.repository, private: true })) });
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], { ...f, read: changed }), /Repository identity differs/);
  assert.equal(calls, 2); assert.deepEqual(resources(f), before);
}));
test('default consumption uses the anonymous reader and preserves strict original producer receipt', async () => withFixture(async (f) => {
  const previous = globalThis.fetch, calls = [];
  globalThis.fetch = async (url, options) => { calls.push({ url, options });
    assert(f.transport.has(url), 'Unexpected public endpoint'); return new Response(f.transport.get(url)); };
  try {
    const { read, ...options } = f;
    const receipts = await consumeRelease(f.root, f.manifest, ['linux'], options);
    assert.equal(receipts.linux.producer.sourceCommit, f.contract.sourceCommit);
    assert.deepEqual(receipts.linux.producer, f.contract.platforms.linux);
    assert(calls.length > 10); assert(calls.every((c) => c.options.headers.Authorization === undefined));
  } finally { globalThis.fetch = previous; }
}));

test('observed long resets, including the 27-minute Windows wait, fail before sleeping or retrying', async () => {
  for (const [now, reset] of [[quotaNow, observedQuotaHeaders['x-ratelimit-reset']],
    [1791756567959, '1791757286'], [1792030955775, '1792032577']]) {
    let requests = 0;
    const read = releasePublicReader({ env: {}, deadlineMs: now + 120 * 60000, now: () => now,
      sleep: async () => assert.fail('long hourly quota must not occupy the runner'), report: () => {},
      fetch: async () => { requests++; return new Response('denied', { status: 403,
        headers: { ...observedQuotaHeaders, 'x-ratelimit-reset': reset } }); } });
    await assert.rejects(read(transportURL, 2), /Release HTTP 403.*primary-quota-wait-budget-exceeded/);
    assert.equal(requests, 1);
  }
});
test('absolute job deadline forbids an overrun, expired requests and a retry after time advances past cutoff', async () => {
  let requests = 0, waits = 0;
  const fetch = async () => { requests++; return new Response('denied', { status: 403, headers: shortQuotaHeaders }); };
  const short = releasePublicReader({ now: () => quotaNow, deadlineMs: quotaNow + 30000, fetch,
    sleep: async () => { waits++; }, report: () => {} });
  await assert.rejects(short('https://api.github.com/repos/polaris-arch/polaris-box', 10), /Release HTTP 403/);
  assert.equal(requests, 1); assert.equal(waits, 0);
  const expired = releasePublicReader({ now: () => quotaNow, deadlineMs: quotaNow - 1, fetch });
  await assert.rejects(expired('https://api.github.com/repos/polaris-arch/polaris-box', 10), /job deadline exceeded/);
  assert.equal(requests, 1);
  let clock = quotaNow; requests = 0;
  const cutoff = quotaNow + 31000;
  const late = releasePublicReader({ now: () => clock, deadlineMs: cutoff, fetch, report: () => {},
    sleep: async () => { waits++; clock = cutoff + 1; } });
  await assert.rejects(late('https://api.github.com/repos/polaris-arch/polaris-box', 10), /job deadline exceeded/);
  assert.equal(requests, 1); assert.equal(waits, 1);
  for (const deadlineMs of ['bad', NaN, Infinity, quotaNow + 181 * 60000]) {
    assert.throws(() => releasePublicReader({ now: () => quotaNow, deadlineMs, fetch }), /absolute release-consumer deadline/);
  }
});

const handoffEnv = { GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: 'polaris-arch/Polaris', GITHUB_RUN_ID: '123',
  GITHUB_RUN_ATTEMPT: '1', GITHUB_REF: 'refs/heads/inert', POLARIS_NATIVE_PAYLOAD_VALIDATION: '0' };
async function handoffFixture(f) {
  await consumeRelease(f.root, f.manifest, undefined, f);
  const before = resources(f), bundle = join(f.root, 'core-wire-inputs');
  const options = { env: handoffEnv, run: f.run, host: 'linux' };
  const name = createCoreWireHandoff(f.root, f.manifest, bundle, candidate, options);
  for (const n of ['.release-receipts', ...Object.keys(RELEASE_TARGETS)]) rmSync(join(f.root, 'resources', n), { recursive: true });
  return { before, bundle, options, name };
}
test('same-attempt wire handoff runs complete resource verification without repeating public consumption', async () => withFixture(async (f) => {
  const h = await handoffFixture(f), calls = f.calls.length;
  assert.equal(h.name, coreWireHandoffName(candidate, { runId: '123', attempt: '1' }));
  const observation = importCoreWireHandoff(f.root, f.manifest, h.bundle, candidate, h.name, h.options);
  assert.equal(observation.candidate, candidate); assert.equal(observation.context.attempt, '1');
  assert.deepEqual(resources(f), h.before); assert.equal(f.calls.length, calls);
  for (const [key, target] of Object.entries(RELEASE_TARGETS)) {
    verifyPackagedRelease(f.root, f.manifest, key, join(f.root, 'resources', key, target.core), f.run, 'linux');
  }
}));
test('handoff rejects old attempts, source, artifact aliases, native mode and tags before resource writes', async () => withFixture(async (f) => {
  const h = await handoffFixture(f);
  const cases = [
    { options: { ...h.options, env: { ...handoffEnv, GITHUB_RUN_ATTEMPT: '2' } } },
    { options: { ...h.options, env: { ...handoffEnv, GITHUB_RUN_ID: '124' } } },
    { options: { ...h.options, env: { ...handoffEnv, POLARIS_NATIVE_PAYLOAD_VALIDATION: '1' } } },
    { options: { ...h.options, env: { ...handoffEnv, GITHUB_REF: 'refs/tags/inert' } } },
    { candidate: 'd'.repeat(40) }, { name: h.name + '-alias' },
  ];
  for (const c of cases) {
    assert.throws(() => importCoreWireHandoff(f.root, f.manifest, h.bundle, c.candidate ?? candidate,
      c.name ?? h.name, c.options ?? h.options)); assert.deepEqual(resources(f), {});
  }
}));
test('actual tar transport preserves executable modes despite artifact outer-file mode reset', async () => withFixture(async (f) => {
  const h = await handoffFixture(f), packed = join(f.root, 'wire-archive');
  const pin = packCoreWireHandoff(h.bundle, packed); assert(pin.archive.bytes > 0);
  assert.equal(pin.archive.memberModes['core-wire-inputs/resources/linux/sing-box'], 0o755);
  assert.equal(pin.archive.memberModes['core-wire-inputs/handoff.json'], 0o600);
  for (const n of readdirSync(packed)) fs.chmodSync(join(packed, n), 0o644);
  const payload = unpackCoreWireHandoff(packed, join(f.root, 'wire-extracted'));
  assert.equal(fs.statSync(join(payload, 'resources/linux/sing-box')).mode & 0o777, 0o755);
  importCoreWireHandoff(f.root, f.manifest, payload, candidate, h.name, h.options);
  assert.deepEqual(resources(f), h.before);
  const metadata = join(packed, 'archive.json'), frozenMetadata = readFileSync(metadata);
  const changedMode = structuredClone(pin); changedMode.archive.memberModes['core-wire-inputs/resources/linux/sing-box'] = 0o644;
  write(metadata, encode(changedMode));
  const badMode = join(f.root, 'wire-mode-extracted');
  assert.throws(() => unpackCoreWireHandoff(packed, badMode)); assert.equal(existsSync(badMode), false);
  write(metadata, frozenMetadata);
  const tar = join(packed, pin.archive.name), original = readFileSync(tar);
  const changed = Buffer.from(original); changed[Math.floor(changed.length / 2)] ^= 1; write(tar, changed);
  const rejected = join(f.root, 'wire-corrupt-extracted');
  assert.throws(() => unpackCoreWireHandoff(packed, rejected)); assert.equal(existsSync(rejected), false);
}));
test('handoff closes changed bytes, unsafe modes, links, extra members and changed manifest without writes', async () => withFixture(async (f) => {
  const h = await handoffFixture(f), core = join(h.bundle, 'resources/linux/sing-box'), original = readFileSync(core);
  const failed = () => { assert.throws(() => importCoreWireHandoff(f.root, f.manifest, h.bundle, candidate, h.name, h.options)); assert.deepEqual(resources(f), {}); };
  write(core, Buffer.alloc(original.length, 0)); failed(); write(core, original); fs.chmodSync(core, 0o755);
  fs.chmodSync(core, 0o777); failed(); fs.chmodSync(core, 0o755);
  const extra = join(h.bundle, 'resources/linux/extra'); write(extra, 'extra'); failed(); rmSync(extra);
  symlinkSync(core, extra); failed(); rmSync(extra);
  const manifest = join(f.root, 'src-tauri/core-manifest.json'), raw = readFileSync(manifest);
  write(manifest, Buffer.concat([raw, Buffer.from(' ')])); failed(); write(manifest, raw);
}));
test('handoff does not overwrite existing resources and invalid receipts roll back its own fresh copies', async () => withFixture(async (f) => {
  const h = await handoffFixture(f); write(join(f.root, 'resources/linux/sing-box'), 'existing'); const before = resources(f);
  assert.throws(() => importCoreWireHandoff(f.root, f.manifest, h.bundle, candidate, h.name, h.options), /fresh resource destinations/);
  assert.deepEqual(resources(f), before); rmSync(join(f.root, 'resources/linux'), { recursive: true });
  const proofPath = join(h.bundle, 'handoff.json'), proof = JSON.parse(readFileSync(proofPath));
  const receipt = join(h.bundle, 'resources/.release-receipts/linux.json'), raw = readFileSync(receipt);
  const changed = Buffer.from(raw); changed[changed.indexOf(Buffer.from('polaris-app-release-consumption-v1'))] = 'x'.charCodeAt(0); write(receipt, changed);
  const file = proof.files.find((r) => r.name === '.release-receipts/linux.json'); file.sha256 = digest(changed);
  const { fingerprint, ...facts } = proof; write(proofPath, encode({ ...facts, fingerprint: digest(portableCanonical(facts)) }));
  assert.throws(() => importCoreWireHandoff(f.root, f.manifest, h.bundle, candidate, h.name, h.options), /consumption receipt differs/);
  assert.deepEqual(resources(f), {});
}));

test('final Mac core uses its complete explicit release receipt for both targets without legacy metadata or signing', async () => withFixture(async (f) => {
  await consumeRelease(f.root, f.manifest, ['mac-x64', 'mac-arm64'], { ...f, host: 'darwin' });
  assert.equal(existsSync(join(f.root, 'resources/.source-receipts')), false);
  for (const key of ['mac-x64', 'mac-arm64']) {
    const stage = join(f.root, 'final-carrier', key); cpSync(join(f.root, 'resources', key), stage, { recursive: true });
    const binary = join(stage, 'sing-box'), before = readFileSync(binary);
    const receipt = verifyMacPackagedCore(f.root, f.manifest, key, binary, f.run);
    assert.equal(receipt.schema, 'polaris-app-release-consumption-v1');
    assert.deepEqual(receipt.producer, f.contract.platforms[key]); assert.deepEqual(readFileSync(binary), before);
    assert(f.inspections.every((call) => !call.includes('--sign') && !call.includes('--force')));
  }
}));
test('Mac release selection rejects invalid or missing release receipts without using present legacy receipts', async () => withFixture(async (f) => {
  await consumeRelease(f.root, f.manifest, ['mac-arm64'], { ...f, host: 'darwin' });
  const core = join(f.root, 'resources/mac-arm64/sing-box');
  write(join(f.root, 'resources/.source-receipts/mac-arm64.json'), 'legacy must not be read');
  for (const desktopRelease of [null, {}, { ...f.manifest.desktopRelease, contractSha256: '0'.repeat(64) }]) {
    assert.throws(() => verifyMacPackagedCore(f.root, { ...f.manifest, desktopRelease }, 'mac-arm64', core, f.run));
  }
  const receipt = join(f.root, 'resources/.release-receipts/mac-arm64.json'); rmSync(receipt);
  assert.throws(() => verifyMacPackagedCore(f.root, f.manifest, 'mac-arm64', core, f.run));
  assert.equal(readFileSync(join(f.root, 'resources/.source-receipts/mac-arm64.json'), 'utf8'), 'legacy must not be read');
}));
test('final Mac release verification closes changed CDHash, bytes, missing notice and added sidecar', async () => withFixture(async (f) => {
  await consumeRelease(f.root, f.manifest, ['mac-arm64'], { ...f, host: 'darwin' });
  const stage = join(f.root, 'final-carrier/mac-arm64'); cpSync(join(f.root, 'resources/mac-arm64'), stage, { recursive: true });
  const binary = join(stage, 'sing-box'), raw = readFileSync(binary);
  write(binary, Buffer.alloc(raw.length)); assert.throws(() => verifyMacPackagedCore(f.root, f.manifest, 'mac-arm64', binary, f.run)); write(binary, raw);
  const wrong = (cmd, args, opts) => {
    const output = f.run(cmd, args, opts);
    return cmd === 'codesign' && args[0] === '--display'
      ? output.replace(/CDHash=([a-f0-9]{40})/, (_match, hash) => `CDHash=${hash[0] === '0' ? '1' : '0'}${hash.slice(1)}`) : output;
  };
  assert.throws(() => verifyMacPackagedCore(f.root, f.manifest, 'mac-arm64', binary, wrong), /signature/);
  const notice = join(stage, 'core-release/NOTICE'); const content = readFileSync(notice); rmSync(notice);
  assert.throws(() => verifyMacPackagedCore(f.root, f.manifest, 'mac-arm64', binary, f.run)); write(notice, content);
  write(join(stage, 'libcronet.dylib'), 'extra');
  assert.throws(() => verifyMacPackagedCore(f.root, f.manifest, 'mac-arm64', binary, f.run), /Static carrier contains Cronet sidecar/);
  assert.deepEqual(readFileSync(binary), raw);
}));

test('DMG trusted temporary roots use physical paths while final Mac payload links still fail closed',
  { skip: process.platform === 'win32' ? 'POSIX shell path fixture; actual macOS packaging remains required' : false },
  async () => withFixture(async (f) => {
    await consumeRelease(f.root, f.manifest, ['mac-x64', 'mac-arm64'], { ...f, host: 'darwin' });
    const workflow = readFileSync(join(repo, '.github/workflows/package.yml'), 'utf8');
    const physical = join(f.root, 'physical-temp'), alias = join(f.root, 'temp-alias');
    mkdirSync(physical); symlinkSync(physical, alias, 'dir');
    const env = { ...process.env, TMPDIR: alias };
    const mountSteps = [...workflow.matchAll(/            mnt=\$\(mktemp -d\)\n(?:            mnt=[^\n]+\n)?            hdiutil attach[^\n]+/g)];
    assert.equal(mountSteps.length, 2, 'Both writable and final read-only DMG mount steps must be exercised');
    for (const step of mountSteps) {
      const commands = step[0].slice(0, step[0].indexOf('            hdiutil'));
      const mount = execFileSync('/bin/bash', ['-e', '-u', '-o', 'pipefail', '-c', commands + '\nprintf "%s" "$mnt"'], { env, encoding: 'utf8' });
      assert.equal(mount, fs.realpathSync(mount), 'A freshly-created mount root still crosses the TMPDIR alias');
      for (const key of ['mac-x64', 'mac-arm64']) {
        const payload = join(mount, `${key}.app/Contents/Resources`, key);
        cpSync(join(f.root, 'resources', key), payload, { recursive: true });
        const binary = join(payload, 'sing-box'), original = readFileSync(binary);
        assert.equal(verifyMacPackagedCore(f.root, f.manifest, key, binary, f.run).platform, key);
        // Canonicalizing only the new temporary root does not admit a linked
        // payload directory, linked core, or the original alias spelling.
        const linkedDirectory = join(mount, `${key}-linked-payload`);
        symlinkSync(payload, linkedDirectory, 'dir');
        assert.throws(() => verifyMacPackagedCore(f.root, f.manifest, key, join(linkedDirectory, 'sing-box'), f.run), /Linked release ancestor/);
        const linkedCore = join(mount, `${key}-linked-core`, 'sing-box');
        mkdirSync(dirname(linkedCore)); symlinkSync(binary, linkedCore);
        assert.throws(() => verifyMacPackagedCore(f.root, f.manifest, key, linkedCore, f.run), /Non-regular release path/);
        const aliasBinary = join(alias, mount.slice(physical.length + 1), `${key}.app/Contents/Resources`, key, 'sing-box');
        assert.throws(() => verifyMacPackagedCore(f.root, f.manifest, key, aliasBinary, f.run), /Linked release ancestor/);
        assert.deepEqual(readFileSync(binary), original);
      }
    }
    const writable = workflow.match(/            rw_dir=\$\(mktemp -d\)\n            rw_dir=[^\n]+\n            rw="\$rw_dir\/rw\.dmg"/);
    assert(writable, 'The writable DMG staging root must be exercised');
    const rw = execFileSync('/bin/bash', ['-e', '-u', '-o', 'pipefail', '-c', writable[0] + '\nprintf "%s" "$rw"'], { env, encoding: 'utf8' });
    assert.equal(dirname(rw), fs.realpathSync(dirname(rw)));
    assert(f.inspections.every((call) => !call.includes('--sign') && !call.includes('--force')));
  }));

test('API redirects cannot forward the dedicated token and unknown authenticated denials never fall back', async () => {
  const env = { GITHUB_ACTIONS: 'true', POLARIS_RELEASE_API_TOKEN: 'inert_fixture_token' };
  for (const status of [302, 401, 403, 404]) {
    let requests = 0;
    const read = releasePublicReader({ env, now: () => quotaNow, deadlineMs: quotaNow + 60000,
      sleep: async () => assert.fail('unknown denial must not wait'), fetch: async (url, options) => {
        requests++; assert.equal(options.headers.Authorization, 'Bearer inert_fixture_token');
        return new Response(null, { status, headers: { Location: 'https://release-assets.githubusercontent.com/inert' } }); } });
    await assert.rejects(read(transportURL, 2), status === 302 ? /Unapproved release redirect/ : new RegExp(`Release HTTP ${status}`));
    assert.equal(requests, 1);
  }
});
test('authorized core quota may recover once using the same Actions credential', async () => {
  const env = { GITHUB_ACTIONS: 'true', POLARIS_RELEASE_API_TOKEN: 'inert_fixture_token' };
  for (const limit of ['1000', '15000']) {
    const calls = [], waits = [];
    const read = releasePublicReader({ env, now: () => quotaNow, deadlineMs: quotaNow + 60000,
      sleep: async (ms) => waits.push(ms), report: () => {}, fetch: async (url, options) => {
        calls.push(options); return calls.length === 1 ? new Response('denied', { status: 403,
          headers: { ...shortQuotaHeaders, 'x-ratelimit-limit': limit } }) : new Response('ok'); } });
    assert.equal((await read(transportURL, 2)).toString(), 'ok'); assert.deepEqual(waits, [30758]);
    assert(calls.every((options) => options.headers.Authorization === 'Bearer inert_fixture_token'));
  }
});
test('conditional metadata performs fresh HTTP and returned Buffer mutation cannot corrupt cached bytes', async () => {
  const calls = [], reports = [];
  const read = releasePublicReader({ env: {}, report: (s) => reports.push(s), fetch: async (url, options) => {
    calls.push({ url, options }); return calls.length === 1 ? new Response('abc', { headers: { etag: '"one"' } })
      : new Response(null, { status: 304, headers: { etag: '"one"' } }); } });
  const first = await read(transportURL, 3); first.fill(0);
  assert.equal((await read(transportURL, 3)).toString(), 'abc'); assert.equal(calls.length, 2);
  assert.equal(calls[0].options.headers['If-None-Match'], undefined);
  assert.equal(calls[1].options.headers['If-None-Match'], '"one"');
  assert.equal(reports.length, 1); assert(!reports[0].includes('"one"'));
});
test('unsolicited, mismatched, smaller-limit and asset 304 responses never reuse metadata', async () => {
  for (const change of ['unsolicited', 'etag', 'limit', 'asset']) {
    let calls = 0;
    const read = releasePublicReader({ env: {}, report: () => {}, fetch: async () => {
      calls++; return calls === 1 && change !== 'unsolicited' ? new Response('abc', { headers: { etag: '"one"' } })
        : new Response(null, { status: 304, headers: { etag: change === 'etag' ? '"other"' : '"one"' } }); } });
    if (change !== 'unsolicited') await read(transportURL, 3);
    await assert.rejects(read(transportURL, change === 'limit' ? 2 : 3, change === 'asset'), /no intact matching metadata/);
  }
});
test('metadata cache is per-reader, bounded to five URLs, and never retains asset bytes', async () => {
  const calls = [];
  const fetch = async (url, options) => { calls.push({ url, options }); return new Response('ok', { headers: { etag: '"one"' } }); };
  const read = releasePublicReader({ env: {}, fetch, report: () => {} });
  for (let i = 0; i < 6; i++) await read(transportURL + `/releases/${i}`, 2);
  await read(transportURL + '/releases/5', 2); assert.equal(calls.at(-1).options.headers['If-None-Match'], undefined);
  await read(transportURL + '/releases/0', 2); assert.equal(calls.at(-1).options.headers['If-None-Match'], '"one"');
  await releasePublicReader({ env: {}, fetch, report: () => {} })(transportURL + '/releases/0', 2);
  assert.equal(calls.at(-1).options.headers['If-None-Match'], undefined);
  const asset = 'https://github.com/polaris-arch/polaris-box/releases/download/inert/core.zip';
  await read(asset, 2, true); await read(asset, 2, true);
  assert(calls.slice(-2).every((c) => c.options.headers['If-None-Match'] === undefined && c.options.headers.Authorization === undefined));
});
test('a changed 200 replaces conditional metadata and loss of ETag removes that cache entry', async () => {
  const calls = [];
  const read = releasePublicReader({ env: {}, report: () => {}, fetch: async (url, options) => {
    calls.push(options); return new Response(calls.length === 1 ? 'old' : 'new',
      { headers: calls.length < 3 ? { etag: calls.length === 1 ? '"old"' : '"new"' } : {} }); } });
  assert.equal((await read(transportURL, 3)).toString(), 'old');
  assert.equal((await read(transportURL, 3)).toString(), 'new');
  await read(transportURL, 3); await read(transportURL, 3);
  assert.deepEqual(calls.map((c) => c.headers['If-None-Match']), [undefined, '"old"', '"new"', undefined]);
});
test('conditional second snapshot rejects changed public identity and leaves old resources intact', async () => withFixture(async (f) => {
  write(join(f.root, 'resources/linux/sing-box'), 'previous'); const before = resources(f), counts = new Map();
  const read = releasePublicReader({ env: {}, report: () => {}, fetch: async (url, options) => {
    const count = (counts.get(url) ?? 0) + 1; counts.set(url, count); assert(f.transport.has(url));
    if (url === f.base && count === 2) {
      assert.equal(options.headers['If-None-Match'], '"one"');
      return new Response(encode({ id: 1407705262, full_name: f.contract.repository, private: true }), { headers: { etag: '"changed"' } });
    }
    assert.equal(options.headers['If-None-Match'], undefined);
    return new Response(f.transport.get(url), { headers: url.startsWith('https://api.github.com/') ? { etag: '"one"' } : {} });
  } });
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], { ...f, read }), /Repository identity differs/);
  assert.equal(counts.get(f.base), 2); assert.deepEqual(resources(f), before);
}));
test('dedicated API token is removed from consumer native-tool environments and receipts', async () => withFixture(async (f) => {
  const previous = process.env.POLARIS_RELEASE_API_TOKEN; process.env.POLARIS_RELEASE_API_TOKEN = 'never_log_fixture_secret';
  let tools = 0;
  try {
    const run = (command, args, settings) => { tools++; assert.equal(settings.env.POLARIS_RELEASE_API_TOKEN, undefined);
      return f.run(command, args, settings); };
    const receipts = await consumeRelease(f.root, f.manifest, ['linux'], { ...f, run });
    assert(tools > 0); assert(!JSON.stringify(receipts).includes('never_log_fixture_secret'));
  } finally {
    if (previous === undefined) delete process.env.POLARIS_RELEASE_API_TOKEN; else process.env.POLARIS_RELEASE_API_TOKEN = previous;
  }
}));
test('untrusted quota header text cannot echo credentials or arbitrary response content into diagnostics', async () => {
  const secret = 'never_log_fixture_secret', env = { GITHUB_ACTIONS: 'true', POLARIS_RELEASE_API_TOKEN: secret };
  await assert.rejects(publicBytes(transportURL, 2, false, { env, fetch: async () => new Response('never_log_body', { status: 403,
    headers: { 'x-github-request-id': secret, 'x-ratelimit-limit': 'never_log_header' } }) }),
  (error) => !error.message.includes('never_log') && error.message.includes('x-github-request-id=unknown'));
});
test('all five fresh second-snapshot 304 checks preserve strict receipts without downloading assets twice', async () => withFixture(async (f) => {
  const counts = new Map();
  const read = releasePublicReader({ env: {}, report: () => {}, fetch: async (url, options) => {
    assert(f.transport.has(url)); const count = (counts.get(url) ?? 0) + 1; counts.set(url, count);
    if (url.startsWith('https://api.github.com/')) {
      assert.equal(options.headers['If-None-Match'], count === 1 ? undefined : '"one"');
      return count === 1 ? new Response(f.transport.get(url), { headers: { etag: '"one"' } })
        : new Response(null, { status: 304, headers: { etag: '"one"' } });
    }
    assert.equal(options.headers.Authorization, undefined); assert.equal(options.headers['If-None-Match'], undefined);
    return new Response(f.transport.get(url));
  } });
  const receipts = await consumeRelease(f.root, f.manifest, ['linux'], { ...f, read });
  assert.deepEqual(receipts.linux.producer, f.contract.platforms.linux);
  assert.equal([...counts].filter(([url]) => url.startsWith('https://api.github.com/')).length, 5);
  assert([...counts].filter(([url]) => url.startsWith('https://api.github.com/')).every(([, count]) => count === 2));
  assert.equal(counts.get(f.contract.assetIndex.url), 1); assert.equal(counts.get(f.contract.platforms.linux.releaseAsset.url), 1);
}));
