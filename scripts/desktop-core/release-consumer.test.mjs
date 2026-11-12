// Synthetic bytes and mocked public HTTP/Go metadata. No Git init, HTTP, core or C API execution.
import { writeSelectedCoreManifest } from '../lib/core-identity.mjs';
import assert from 'node:assert/strict';
import childProcess, { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync,
  rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { digest } from './source-graph.mjs';
import { consumeRelease, loadReleaseContract, portableCanonical,
  RELEASE_TARGETS, publicBytes, releasePublicReader, recoverRelease, releasePlatforms, validateReleaseContract, verifyPackagedRelease,
  verifyReleaseCore, releaseInstallationContract, UPSTREAM_INSTALLATION_DOMAIN } from './release-consumer.mjs';
import { verifyManagedMeshCore } from './managed-mesh-core.mjs';
import { inspectNativePayload } from '../native-payload-input.mjs';
import { cronetFixture } from '../lib/cronet-payload.test.mjs';

const repo = join(dirname(fileURLToPath(import.meta.url)), '../..');
const PYTHON = process.platform === 'win32' ? 'python' : 'python3';
const candidate = 'c'.repeat(40), commit = '1'.repeat(40), tree = '2'.repeat(40), tagObject = '3'.repeat(40);
const version = '1.15.0-polaris-alpha.12', tag = `v${version}`;
const write = (p, b) => { mkdirSync(dirname(p), { recursive: true }); writeFileSync(p, b); };
const encode = (v) => Buffer.from(JSON.stringify(v));

function coreFixture(key) {
  const t = RELEASE_TARGETS[key], b = Buffer.alloc(256);
  if (t.os === 'linux') { b.set([127, 69, 76, 70, 2, 1]); b.writeUInt16LE(62, 18); }
  else if (t.os === 'windows') { b.write('MZ'); b.writeUInt32LE(0x80, 60); b.write('PE\0\0', 0x80); b.writeUInt16LE(0x8664, 0x84); }
  else { b.writeUInt32LE(0xfeedfacf); b.writeUInt32LE(t.arch === 'amd64' ? 0x1000007 : 0x100000c, 4); }
  b.write(`inert-${key}`, 200); return b;
}
function metadata(row) {
  const g = row.buildGraph;
  return `/synthetic/sing-box: ${row.goVersion}\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n\tmod\tgithub.com/sagernet/sing-box\t(devel)\n`
    + g.localReplacements.map((r) => `\tdep\t${r.module}\t${r.version}\n\t=>\t${r.replacement.module}\t${r.replacement.version}\n`).join('')
    + [...g.normalModules, ...g.prebuiltAcquisitionModules].map((d) => `\tdep\t${d.module}\t${d.version}\t${d.h1}\n`).join('')
    + Object.entries(g.settings).map(([k, v]) => `\tbuild\t${k}=${v}\n`).join('');
}
function fixture(options = {}) {
  const root = mkdtempSync(join(tmpdir(), 'polaris-release-fixture-'));
  const transport = new Map(), calls = [], displays = {};
  const repository = 'polaris-arch/sing-box-polaris', goVersion = 'go1.26.8';
  const legal = (path, content) => ({ path, content, bytes: Buffer.byteLength(content), sha256: digest(content) });
  const contract = { schema: 'polaris-upstream-native-release-consumer-v1', providerContract: 'polaris-upstream-native-release-v1',
    repository, repositoryId: 1421657509, releaseId: 17, sourceCommit: commit, sourceTree: tree,
    version, tag, tagObject, sourcePolicy: { goVersion: goVersion.slice(2) }, producer: { fixtureOnly: true },
    sourceArchive: { name: `sing-box-polaris-${commit}.tar.gz`, url: `https://github.com/${repository}/archive/${commit}.tar.gz`,
      bytes: 1, sha256: 'f'.repeat(64) },
    legalFiles: [legal('licenses/LICENSE', 'synthetic license'), legal('licenses/NOTICE', 'synthetic notice'), legal('licenses/go-1.26.8/LICENSE', 'synthetic go license')], platforms: {} };
  const asset = (name, bytes, id) => ({ name, bytes: bytes.length, sha256: digest(bytes), assetId: id,
    releaseId: 17, url: `https://github.com/${repository}/releases/download/${tag}/${name}` });
  for (const [key, t] of Object.entries(RELEASE_TARGETS)) {
    const rootName = `sing-box-${version}-${t.os}-${t.arch}`;
    const tags = ['with_naive_outbound', 'with_gvisor', ...(t.sidecar ? ['with_purego'] : [])].sort().join(',');
    const signed = t.os === 'darwin' && !(options.unsignedIntel && key === 'mac-x64');
    displays[key] = signed ? `Executable=/synthetic/sing-box\nSignature=adhoc\nCDHash=${'4'.repeat(40)}\n`
      : 'sing-box: code object is not signed at all\n';
    const coreBytes = coreFixture(key), sidecarBytes = t.sidecar ? cronetFixture(key) : null;
    if (options.coreMarker !== undefined) coreBytes[255] = options.coreMarker;
    if (key === 'linux' && options.linuxMachine !== undefined) coreBytes.writeUInt16LE(options.linuxMachine, 18);
    const files = { [t.core]: coreBytes, LICENSE: Buffer.from('synthetic license'), NOTICE: Buffer.from('synthetic notice') };
    if (t.sidecar) files[t.sidecar] = sidecarBytes;
    files.SHA256SUMS = Buffer.from(Object.keys(files).sort().map((n) => `${digest(files[n])}  ${n}\n`).join(''));
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
    const bytes = readFileSync(archiveFile), releaseAsset = asset(`sing-box-polaris-${version.replace('-polaris-', '-')}-${t.os}-${t.arch}${t.suffix}`, bytes, 20 + Object.keys(contract.platforms).length);
    const row = { schema: 'polaris-upstream-native-receipt-v1', id: rootName, target: `${t.os}/${t.arch}`,
      variant: { linkage: t.sidecar ? 'shared-purego' : 'static-cgo', libc: '', goarm: '', gomips: '', go386: '', legacy: false },
      version, tag, sourceCommit: commit, sourceTree: tree, goVersion, tags, buildID: `synthetic-build-id-${key}`,
      buildGraph: { settings: { GOOS: t.os, GOARCH: t.arch, CGO_ENABLED: t.cgo, 'vcs.revision': commit,
        'vcs.modified': 'false', '-trimpath': 'true', '-tags': tags },
      normalModules: [{ module: 'example.com/inert', version: 'v1.0.0', h1: 'h1:AA=' }],
      prebuiltAcquisitionModules: t.sidecar ? [{ module: `github.com/sagernet/cronet-go/lib/${t.os}_amd64`, version: 'v1.0.0', h1: 'h1:BB=' }] : [],
      localReplacements: [{ module: 'github.com/sagernet/sing-tun', version: 'v0.9.7', h1: '',
        replacement: { module: './third_party/sing-tun', version: '(devel)', h1: '' } }] },
      archive: { name: rootName + t.suffix, releaseName: releaseAsset.name, root: rootName, bytes: bytes.length, sha256: digest(bytes),
        members: Object.fromEntries(Object.entries(files).map(([n, b]) => [rootName + '/' + n, digest(b)])) },
      core: { member: t.core, bytes: files[t.core].length, sha256: digest(files[t.core]) },
      sidecar: t.sidecar ? { member: t.sidecar, bytes: files[t.sidecar].length, sha256: digest(files[t.sidecar]) } : null,
      signatureObservation: t.os === 'darwin' ? { state: signed ? 'signed' : 'unsigned', cdHash: signed ? '4'.repeat(40) : null,
        trustValidated: false } : null, releaseAsset };
    row.rawBuildInfo = metadata(row);
    contract.platforms[key] = row;
    transport.set(releaseAsset.url, bytes);
  }
  const contractBytes = encode(contract);
  const manifest = { desktopRelease: { schema: 'polaris-app-release-selection-v1',
    contractPath: 'scripts/desktop-core/release-contract.json', contractSha256: digest(contractBytes) } };
  write(join(root, manifest.desktopRelease.contractPath), contractBytes);
  write(join(root, 'src-tauri/core-manifest.json'), encode(manifest));
  const base = `https://api.github.com/repos/${contract.repository}`;
  transport.set(base, encode({ id: contract.repositoryId, full_name: contract.repository, private: false }));
  const publicRelease = { id: 17, tag_name: tag, draft: false, html_url: `https://github.com/${contract.repository}/releases/tag/${tag}`,
    assets: Object.values(contract.platforms).map((r) => r.releaseAsset).map((a) => ({ id: a.assetId, name: a.name,
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
      assert(row, 'Unknown inert binary'); return args[0] === 'version' ? row.rawBuildInfo : row.buildID;
    }
    if (command === 'codesign' || command === 'file') {
      inspections.push([command, ...args]);
      const hash = digest(readFileSync(args.at(-1))), [key, row] = Object.entries(contract.platforms).find(([, r]) => r.core.sha256 === hash) ?? [];
      assert(row && row.target.startsWith('darwin/'), 'Unknown inert Mac binary');
      const display = displays[key];
      if (command === 'file') return 'Mach-O 64-bit executable x86_64';
      if (args[0] === '--display') return display;
      assert.deepEqual(args.slice(0, -1), ['--verify', '--strict']);
      if (display.includes('code object is not signed at all')) return { status: 1, stderr: display };
      return '';
    }
    assert(['python3', 'python'].includes(command), 'Target execution prohibited'); return execFileSync(command, args, options);
  };
  const linux = contract.platforms.linux;
  const linuxVersionOutput = `sing-box version ${version}\n\nEnvironment: ${goVersion} ${linux.target}\nTags: ${linux.tags}\n`
    + `Revision: ${commit}\nCGO: disabled\n`;
  return { root, contract, manifest, transport, calls, read, run, base, publicRelease, inspections, displays,
    linuxVersionOutput, blobCacheDir: false };
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

test('all four release archives keep exact cores, colocated libraries and notices', async () => withFixture(async (f) => {
  validateReleaseContract(f.contract); const receipts = await consumeRelease(f.root, f.manifest, undefined, f);
  assert.equal(Object.keys(receipts).length, 4);
  for (const [key, row] of Object.entries(f.contract.platforms)) {
    const path = join(f.root, 'resources', key, row.core.member);
    const receipt = verifyPackagedRelease(f.root, f.manifest, key, path, f.run);
    assert.equal(receipt.schema, 'polaris-app-release-consumption-v1');
    assert.deepEqual(receipt.producer, row);
    assert(existsSync(join(dirname(path), 'core-release/LICENSE')));
    if (row.sidecar) assert.equal(inspectNativePayload(f.root, key, path, join(dirname(path), row.sidecar.member), f.run).schema, 'polaris-native-release-payload-input-v1');
  }
  assert.equal(f.calls.filter((url) => url.includes('/releases/17')).length, 2);
  assert(f.calls.every((url) => !url.includes('proxy.golang.org') && !url.includes('raw.githubusercontent.com')));
  assert(!existsSync(join(f.root, 'resources/.source-receipts')));
}));
test('normal release consumption extracts each original archive with the bounded native extractor', async () => withFixture(async f => {
  const extractions = [];
  const run = (command, args, options) => {
    if (args[1]?.endsWith('release-archive.py')) {
      assert.equal(command, PYTHON);
      assert.equal(options.timeout, 120_000); assert.equal(options.killSignal, 'SIGKILL');
      assert.equal(options.encoding, 'utf8'); assert.equal(options.maxBuffer, 1024 * 1024);
      const request = JSON.parse(options.input);
      const key = Object.keys(RELEASE_TARGETS).find(key => f.contract.platforms[key].target === request.target);
      assert.ok(key);
      const row = f.contract.platforms[key];
      assert.deepEqual(request, { archive: row.archive, root: row.archive.name.slice(0, -RELEASE_TARGETS[key].suffix.length), target: row.target });
      extractions.push(key);
    }
    return f.run(command, args, options);
  };
  await consumeRelease(f.root, f.manifest, undefined, { ...f, run });
  assert.deepEqual(extractions.sort(), Object.keys(RELEASE_TARGETS).sort());
}));

test('cold archive failure returns through acquiring finally without retries or replacement', async () => {
  for (const [key, failure] of [
    ['linux', { code: 'ETIMEDOUT', status: null, signal: 'SIGKILL' }],
    ['mac-arm64', { status: 7, signal: null }],
  ]) await withFixture(async f => {
    await consumeRelease(f.root, f.manifest, ['linux'], f);
    const directory = join(f.root, 'resources');
    write(join(directory, '.release-consumer-unrelated/sentinel'), 'another transaction is not ours');
    const before = resources(f), entries = readdirSync(directory).sort();
    const downloads = f.calls.filter(url => url === f.contract.platforms[key].releaseAsset.url).length;
    let archiveCalls = 0, replacements = 0, ownedWork;
    const run = (command, args, options) => {
      if (args[1]?.endsWith('release-archive.py')) {
        archiveCalls++;
        assert.equal(options.timeout, 120_000); assert.equal(options.killSignal, 'SIGKILL');
        const journal = JSON.parse(readFileSync(join(directory, '.release-consumer.lock'), 'utf8'));
        assert.equal(journal.phase, 'acquiring');
        ownedWork = join(directory, journal.work); assert(existsSync(ownedWork));
        throw Object.assign(Error(`never_log_cold_input ${options.input} ${f.root}`), failure,
          { stderr: Buffer.from(`never_log_cold_stderr ${f.root}`) });
      }
      return f.run(command, args, options);
    };
    await assert.rejects(consumeRelease(f.root, f.manifest, [key], { ...f, run,
      rename: () => { replacements++; assert.fail('acquiring failure must not replace'); } }), error => {
      assert.match(error.message, /stage=native-archive-extraction; elapsedMs=\d+; timeoutMs=120000/);
      assert(error.message.includes(`code=${failure.code ?? 'unknown'}`));
      assert(error.message.includes(`status=${failure.status ?? 'unknown'}`));
      assert(error.message.includes(`signal=${failure.signal ?? 'unknown'}`));
      assert.doesNotMatch(String(error), /never_log_cold/);
      assert(!String(error).includes(f.root)); assert.equal(error.cause, undefined);
      return true;
    });
    assert.equal(archiveCalls, 1); assert.equal(replacements, 0);
    assert.equal(f.calls.filter(url => url === f.contract.platforms[key].releaseAsset.url).length - downloads, 1);
    assert(ownedWork); assert(!existsSync(ownedWork));
    assert(!existsSync(join(directory, '.release-consumer.lock')));
    assert.deepEqual(readdirSync(directory).sort(), entries);
    assert.deepEqual(resources(f), before, 'old payloads, receipts and unrelated work survive');
  });
});
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
    const manifest = { ...f.manifest, desktopRelease: value };
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
test('truncated/replaced archive fails before replacing any core or library', async () => withFixture(async (f) => {
  write(join(f.root, 'resources/linux/sing-box'), 'previous'); const before = resources(f);
  for (const url of [f.contract.platforms.linux.releaseAsset.url]) {
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
test('producer tags/asset, BuildID or module graph tampering fails closed', async () => withFixture(async (f) => {
  for (const change of [(r) => { r.tags += ',with_purego'; },
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
test('final carrier retains original notice bytes as well as exact core and Cronet bytes', async () => withFixture(async (f) => {
  await consumeRelease(f.root, f.manifest, ['linux'], f);
  const carrier = join(f.root, 'carrier/linux'); cpSync(join(f.root, 'resources/linux'), carrier, { recursive: true });
  assert.equal(verifyPackagedRelease(f.root, f.manifest, 'linux', join(carrier, 'sing-box'), f.run).platform, 'linux');
  writeFileSync(join(carrier, 'core-release/LICENSE'), 'wrong license');
  assert.throws(() => verifyPackagedRelease(f.root, f.manifest, 'linux', join(carrier, 'sing-box'), f.run), /notice\/provenance/);
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
    if (key === 'win') write(join(f.root, 'resources/win/polaris-helper-setup.exe'), 'inert');
    writeSelectedCoreManifest(join(f.root, 'resources', key), releaseInstallationContract(f.root, f.manifest), key, '1.0.0-fixture');
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

test('producer Mac observation must yield an untrusted valid state/CDHash', () => withFixture(async (f) => {
  for (const observation of [undefined, null, {}, { state: 'signed', cdHash: 'invalid', trustValidated: false },
    { state: 'signed', cdHash: '4'.repeat(40), trustValidated: true },
    { state: 'unsigned', cdHash: '4'.repeat(40), trustValidated: false }]) {
    const contract = structuredClone(f.contract); contract.platforms['mac-arm64'].signatureObservation = observation;
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
  const carrier = join(f.root, 'signature-carrier'), runtime = join(carrier, key);
  cpSync(join(f.root, 'resources', key), runtime, { recursive: true });
  const core = join(runtime, 'sing-box'), before = digest(readFileSync(core));
  assert.equal(verifyPackagedRelease(f.root, f.manifest, key, core, f.run, 'darwin').platform, key);
  for (const run of badRuns) assert.throws(() => verifyPackagedRelease(f.root, f.manifest, key, core, run, 'darwin'), /signature|CDHash/);
  const file = join(f.root, 'resources/.release-receipts', `${key}.json`), original = readFileSync(file);
  for (const signature of [undefined, { state: 'invalid', cdHash: '4'.repeat(40) }, { state: 'signed', cdHash: '5'.repeat(40) }, { state: 'unsigned', cdHash: null }]) {
    const value = JSON.parse(original); if (signature === undefined) delete value.macCodeSignature; else value.macCodeSignature = signature;
    const { schema, fingerprint, ...facts } = value; value.fingerprint = digest(portableCanonical(facts)); write(file, encode(value));
    assert.throws(() => verifyPackagedRelease(f.root, f.manifest, key, core, f.run, 'darwin'), /signature|CDHash/);
  }
  write(file, original); assert.equal(digest(readFileSync(core)), before);
  assert(f.inspections.some(([command, flag, strict]) => command === 'codesign' && flag === '--verify' && strict === '--strict'));
  assert(f.inspections.every(([, ...args]) => !args.includes('--sign')));
}));

test('unsigned Intel release retains only the existing thin-x86_64 native exception', async () => withFixture(async (f) => {
  const key = 'mac-x64', receipts = await consumeRelease(f.root, f.manifest, [key], { ...f, host: 'darwin' });
  assert.deepEqual(receipts[key].macCodeSignature, { state: 'unsigned', cdHash: null });
  const core = join(f.root, 'resources', key, 'sing-box');
  assert.equal(verifyPackagedRelease(f.root, f.manifest, key, core, f.run, 'darwin').platform, key);
  const run = (command, args, options) => command === 'file' ? 'Mach-O universal x86_64 arm64' : f.run(command, args, options);
  assert.throws(() => verifyPackagedRelease(f.root, f.manifest, key, core, run, 'darwin'), /thin Intel/);
}, { unsignedIntel: true }));
test('actual fetch CLI rejects malformed release mode and an incomplete selection', async () => withFixture(async (f) => {
  cpSync(join(repo, 'scripts'), join(f.root, 'scripts'), { recursive: true });
  // Production now has a bound contract; this private fixture retains its own exact bytes.
  write(join(f.root, f.manifest.desktopRelease.contractPath), encode(f.contract));
  let result;
  for (const args of [['--producer'], ['--platform=linux,,win']]) {
    result = spawnSync(process.execPath, [join(f.root, 'scripts/fetch-core.mjs'), ...args], { encoding: 'utf8' });
    assert.equal(result.status, 1); assert.match(result.stderr, /Release mode|architecture selection/);
  }
  write(join(f.root, 'src-tauri/core-manifest.json'), encode({ ...f.manifest, desktopRelease: null }));
  result = spawnSync(process.execPath, [join(f.root, 'scripts/fetch-core.mjs')], { encoding: 'utf8' });
  assert.equal(result.status, 1); assert.match(result.stderr, /Explicit release selection incomplete/);
  assert.deepEqual(resources(f), {});
}));
test('selected managed mesh identity binds original release bytes, receipt and full version observation', async () => withFixture(async f => {
  await consumeRelease(f.root,f.manifest,['linux'],f);
  const core=join(f.root,'resources/linux/sing-box'), row=f.contract.platforms.linux; let versions=0;
  const run=(command,args,options)=>{if(command===core){assert.deepEqual(args,['version']);versions++;return f.linuxVersionOutput;}return f.run(command,args,options);};
  const id=verifyManagedMeshCore(f.root,f.manifest,core,run,{os:'linux',arch:'x64'});
  assert.equal(id.sourceCommit,commit);assert.equal(id.sourceTree,tree);assert.equal(id.coreSha256,row.core.sha256);
  const raw=readFileSync(join(f.root,f.manifest.desktopRelease.contractPath));
  assert.equal(id.providerFingerprint,digest(Buffer.concat([Buffer.from(UPSTREAM_INSTALLATION_DOMAIN),raw])));assert.equal(id.buildID,row.buildID);assert.equal(versions,1);
}));
test('selected managed mesh wrong core bytes fail before executing version', async () => withFixture(async f => {
  await consumeRelease(f.root,f.manifest,['linux'],f);
  const core=join(f.root,'resources/linux/sing-box'), bytes=readFileSync(core);bytes[220]^=1;writeFileSync(core,bytes);let versions=0;
  const run=(command,args,options)=>{if(command===core){versions++;return f.linuxVersionOutput;}return f.run(command,args,options);};
  assert.throws(()=>verifyManagedMeshCore(f.root,f.manifest,core,run,{os:'linux',arch:'x64'}),/bytes differ/);assert.equal(versions,0);
}));
test('selected managed mesh wrong Revision and nonzero version both fail after strict metadata verification', async () => withFixture(async f => {
  await consumeRelease(f.root,f.manifest,['linux'],f);const core=join(f.root,'resources/linux/sing-box'), row=f.contract.platforms.linux;
  for(const fail of ['revision','status']){let versions=0;
   const run=(command,args,options)=>{if(command===core){versions++;if(fail==='status')throw Object.assign(Error('version status 7'),{status:7});return f.linuxVersionOutput.replace(`Revision: ${commit}`,`Revision: ${'9'.repeat(40)}`);}return f.run(command,args,options);};
   assert.throws(()=>verifyManagedMeshCore(f.root,f.manifest,core,run,{os:'linux',arch:'x64'}),fail==='status'?/version status 7/:/version output differs/);assert.equal(versions,1);
  }
}));
test('managed mesh refuses unsupported hosts before inspecting or executing a core', () => withFixture(f => {
  for(const host of [{os:'win32',arch:'arm64'},{os:'linux',arch:'arm64'}])
    assert.throws(()=>verifyManagedMeshCore(f.root,f.manifest,'/absent',()=>assert.fail('foreign execution'),host),/supported native desktop host/);
}));


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
  await publicBytes('https://api.github.com/repos/polaris-arch/sing-box-polaris/releases/17', 2, false, { fetch, env });
   assert.equal(calls.pop().headers.Authorization, 'Bearer inert-fixture-token');
   for (const outside of [{ GITHUB_ACTIONS: 'false', POLARIS_RELEASE_API_TOKEN: 'inert-fixture-token' },
     { GITHUB_ACTIONS: 'true', GH_TOKEN: 'ignored-fixture-token', GITHUB_TOKEN: 'ignored-fixture-token' }]) {
     await publicBytes('https://api.github.com/repos/polaris-arch/sing-box-polaris', 2, false, { fetch, env: outside });
     assert.equal(calls.pop().headers.Authorization, undefined);
   }
   for (const url of ['https://api.github.com/repos/other/repo',
     'https://api.github.com/repos/polaris-arch/sing-box-polaris?secret=fixture',
     'https://api.github.com/repos/polaris-arch/sing-box-polaris#fragment',
     'https://fixture-user@api.github.com/repos/polaris-arch/sing-box-polaris',
     'http://api.github.com/repos/polaris-arch/sing-box-polaris']) {
     await publicBytes(url, 2, false, { fetch, env });
     assert.equal(calls.pop().headers.Authorization, undefined);
   }
  const assetFetch = async (url, options) => { calls.push({ url, ...options });
    return url.includes('github.com/') ? new Response(null, { status: 302,
      headers: { Location: 'https://release-assets.githubusercontent.com/inert?signature=fixture' } }) : new Response('ok'); };
  await publicBytes('https://github.com/polaris-arch/sing-box-polaris/releases/download/inert/core.zip', 2, true, { fetch: assetFetch, env });
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
  await assert.rejects(publicBytes('https://api.github.com/repos/polaris-arch/sing-box-polaris?secret=fixture-query', 10, false, { fetch, env }),
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
  f.transport.set(f.base, encode({ id: 1421657509, full_name: f.contract.repository, private: true }));
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], f), /Repository identity differs/);
  assert.deepEqual(resources(f), before);
}));

const quotaNow = 1791755097242;
const transportURL = 'https://api.github.com/repos/polaris-arch/sing-box-polaris';
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
    return new Response(encode({ id: 1421657509, full_name: f.contract.repository, private: true })); } });
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
  const url = 'https://api.github.com/repos/polaris-arch/sing-box-polaris';
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
  await assert.rejects(read('https://api.github.com/repos/polaris-arch/sing-box-polaris', 10), /Release HTTP 429/);
  assert.equal(calls, 2); assert.deepEqual(waits, [30758]);
});
test('unknown, expired, excessive, asset and noncanonical denials never consume a retry', async () => {
  const base = 'https://api.github.com/repos/polaris-arch/sing-box-polaris';
  const cases = [
    { status: 401 }, { status: 404 }, { status: 500 }, { asset: true },
    { url: 'https://github.com/repos/polaris-arch/sing-box-polaris' },
    { url: 'http://api.github.com/repos/polaris-arch/sing-box-polaris' },
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
      : new Response(encode({ id: 1421657509, full_name: f.contract.repository, private: true })) });
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
  await assert.rejects(short('https://api.github.com/repos/polaris-arch/sing-box-polaris', 10), /Release HTTP 403/);
  assert.equal(requests, 1); assert.equal(waits, 0);
  const expired = releasePublicReader({ now: () => quotaNow, deadlineMs: quotaNow - 1, fetch });
  await assert.rejects(expired('https://api.github.com/repos/polaris-arch/sing-box-polaris', 10), /job deadline exceeded/);
  assert.equal(requests, 1);
  let clock = quotaNow; requests = 0;
  const cutoff = quotaNow + 31000;
  const late = releasePublicReader({ now: () => clock, deadlineMs: cutoff, fetch, report: () => {},
    sleep: async () => { waits++; clock = cutoff + 1; } });
  await assert.rejects(late('https://api.github.com/repos/polaris-arch/sing-box-polaris', 10), /job deadline exceeded/);
  assert.equal(requests, 1); assert.equal(waits, 1);
  for (const deadlineMs of ['bad', NaN, Infinity, quotaNow + 181 * 60000]) {
    assert.throws(() => releasePublicReader({ now: () => quotaNow, deadlineMs, fetch }), /absolute release-consumer deadline/);
  }
});

test('final Mac core uses its complete explicit release receipt for both targets without legacy metadata or signing', async () => withFixture(async (f) => {
  await consumeRelease(f.root, f.manifest, ['mac-x64', 'mac-arm64'], { ...f, host: 'darwin' });
  assert.equal(existsSync(join(f.root, 'resources/.source-receipts')), false);
  for (const key of ['mac-x64', 'mac-arm64']) {
    const stage = join(f.root, 'final-carrier', key); cpSync(join(f.root, 'resources', key), stage, { recursive: true });
    const binary = join(stage, 'sing-box'), before = readFileSync(binary);
    const receipt = verifyPackagedRelease(f.root, f.manifest, key, binary, f.run, 'darwin');
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
    assert.throws(() => verifyPackagedRelease(f.root, { ...f.manifest, desktopRelease }, 'mac-arm64', core, f.run, 'darwin'));
  }
  const receipt = join(f.root, 'resources/.release-receipts/mac-arm64.json'); rmSync(receipt);
  assert.throws(() => verifyPackagedRelease(f.root, f.manifest, 'mac-arm64', core, f.run, 'darwin'));
  assert.equal(readFileSync(join(f.root, 'resources/.source-receipts/mac-arm64.json'), 'utf8'), 'legacy must not be read');
}));
test('final Mac release verification closes changed CDHash, bytes, missing notice and added sidecar', async () => withFixture(async (f) => {
  await consumeRelease(f.root, f.manifest, ['mac-arm64'], { ...f, host: 'darwin' });
  const stage = join(f.root, 'final-carrier/mac-arm64'); cpSync(join(f.root, 'resources/mac-arm64'), stage, { recursive: true });
  const binary = join(stage, 'sing-box'), raw = readFileSync(binary);
  write(binary, Buffer.alloc(raw.length)); assert.throws(() => verifyPackagedRelease(f.root, f.manifest, 'mac-arm64', binary, f.run, 'darwin')); write(binary, raw);
  const wrong = (cmd, args, opts) => {
    const output = f.run(cmd, args, opts);
    return cmd === 'codesign' && args[0] === '--display'
      ? output.replace(/CDHash=([a-f0-9]{40})/, (_match, hash) => `CDHash=${hash[0] === '0' ? '1' : '0'}${hash.slice(1)}`) : output;
  };
  assert.throws(() => verifyPackagedRelease(f.root, f.manifest, 'mac-arm64', binary, wrong, 'darwin'), /signature/);
  const notice = join(stage, 'core-release/NOTICE'); const content = readFileSync(notice); rmSync(notice);
  assert.throws(() => verifyPackagedRelease(f.root, f.manifest, 'mac-arm64', binary, f.run, 'darwin')); write(notice, content);
  write(join(stage, 'libcronet.dylib'), 'extra');
  assert.throws(() => verifyPackagedRelease(f.root, f.manifest, 'mac-arm64', binary, f.run, 'darwin'), /Static carrier contains Cronet sidecar/);
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
    const ownedRoots = [];
    const ownRoot = (path) => {
      assert.match(basename(path), /^tmp\.[A-Za-z0-9]+$/, 'Only this mktemp invocation owns cleanup');
      ownedRoots.push(path); return path;
    };
    try {
    for (const step of mountSteps) {
      const commands = step[0].slice(0, step[0].indexOf('            hdiutil'));
      const mount = execFileSync('/bin/bash', ['-e', '-u', '-o', 'pipefail', '-c', commands + '\nprintf "%s" "$mnt"'], { env, encoding: 'utf8' });
      ownRoot(mount);
      const mountAlias = join(f.root, `mount-alias-${ownedRoots.length}`); symlinkSync(mount, mountAlias, 'dir');
      assert.equal(mount, fs.realpathSync(mount), 'A freshly-created mount root still crosses the TMPDIR alias');
      for (const key of ['mac-x64', 'mac-arm64']) {
        const payload = join(mount, `${key}.app/Contents/Resources`, key);
        cpSync(join(f.root, 'resources', key), payload, { recursive: true });
        const binary = join(payload, 'sing-box'), original = readFileSync(binary);
        assert.equal(verifyPackagedRelease(f.root, f.manifest, key, binary, f.run, 'darwin').platform, key);
        // Canonicalizing only the new temporary root does not admit a linked
        // payload directory, linked core, or the original alias spelling.
        const linkedDirectory = join(mount, `${key}-linked-payload`);
        symlinkSync(payload, linkedDirectory, 'dir');
        assert.throws(() => verifyPackagedRelease(f.root, f.manifest, key, join(linkedDirectory, 'sing-box'), f.run, 'darwin'), /Linked release ancestor/);
        const linkedCore = join(mount, `${key}-linked-core`, 'sing-box');
        mkdirSync(dirname(linkedCore)); symlinkSync(binary, linkedCore);
        assert(fs.lstatSync(linkedCore).isSymbolicLink(), 'the adversary must remain a real filesystem link');
        // The final-carrier gate checks lstat permissions before its core
        // verifier, so POSIX link permissions can reject this path first.
        // Independently exercise that verifier to retain the no-follow proof.
        const row = f.contract.platforms[key];
        const noInspection = () => assert.fail('A linked core must fail before Go or signature inspection');
        verifyReleaseCore(binary, row, key, f.run, 'darwin', f.run, f.contract);
        assert.throws(() => verifyReleaseCore(linkedCore, row, key, noInspection, 'darwin', noInspection, f.contract),
          /Non-regular release path/);
        const linkMode = fs.lstatSync(linkedCore).mode, sourceMode = fs.lstatSync(binary).mode;
        const modeDiffers = ((linkMode | sourceMode) & 0o7000) !== 0 || (linkMode & 0o7777) !== (sourceMode & 0o7777);
        const noCoreInspection = (command, args, options) => {
          assert.equal(command, 'git', 'Final-carrier rejection must precede core inspection');
          return f.run(command, args, options);
        };
        assert.throws(() => verifyPackagedRelease(f.root, f.manifest, key, linkedCore, noCoreInspection, 'darwin'),
          { message: modeDiffers ? 'Final release core permissions differ' : `Non-regular release path: ${linkedCore}` });
        const aliasBinary = join(mountAlias, `${key}.app/Contents/Resources`, key, 'sing-box');
        assert.throws(() => verifyPackagedRelease(f.root, f.manifest, key, aliasBinary, f.run, 'darwin'), /Linked release ancestor/);
        assert.deepEqual(readFileSync(binary), original);
      }
    }
    const writable = workflow.match(/            rw_dir=\$\(mktemp -d\)\n            rw_dir=[^\n]+\n            rw="\$rw_dir\/rw\.dmg"/);
    assert(writable, 'The writable DMG staging root must be exercised');
    const rw = execFileSync('/bin/bash', ['-e', '-u', '-o', 'pipefail', '-c', writable[0] + '\nprintf "%s" "$rw"'], { env, encoding: 'utf8' });
    ownRoot(dirname(rw));
    assert.equal(dirname(rw), fs.realpathSync(dirname(rw)));
    assert(f.inspections.every((call) => !call.includes('--sign') && !call.includes('--force')));
    } finally { for (const path of ownedRoots) rmSync(path, { recursive: true }); }
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
  const asset = 'https://github.com/polaris-arch/sing-box-polaris/releases/download/inert/core.zip';
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
      return new Response(encode({ id: 1421657509, full_name: f.contract.repository, private: true }), { headers: { etag: '"changed"' } });
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
      assert.equal(options.headers['If-None-Match'], count === 1 ? undefined : 'W/"one"');
      return count === 1 ? new Response(f.transport.get(url), { headers: { ETag: 'W/"one"' } })
        : new Response(null, { status: 304, headers: { eTaG: '"one"' } });
    }
    assert.equal(options.headers.Authorization, undefined); assert.equal(options.headers['If-None-Match'], undefined);
    return new Response(f.transport.get(url));
  } });
  const receipts = await consumeRelease(f.root, f.manifest, ['linux'], { ...f, read });
  assert.deepEqual(receipts.linux.producer, f.contract.platforms.linux);
  assert.equal([...counts].filter(([url]) => url.startsWith('https://api.github.com/')).length, 5);
  assert([...counts].filter(([url]) => url.startsWith('https://api.github.com/')).every(([, count]) => count === 2));
  assert.equal(counts.get(f.contract.platforms.linux.releaseAsset.url), 1);
}));


test('dedicated opaque Bearer credentials accept standard punctuation without exposing their bytes', async () => {
  for (const secret of ['inert_fixture_token', 'v1.fixture.payload.signature', 'fixture~token+part/other==']) {
    const reports = []; let calls = 0;
    const read = releasePublicReader({ env: { GITHUB_ACTIONS: 'true', POLARIS_RELEASE_API_TOKEN: secret },
      report: (text) => reports.push(text), fetch: async (url, options) => {
        calls++; assert.equal(options.headers.Authorization, `Bearer ${secret}`);
        return new Response('ok', { headers: { 'x-ratelimit-limit': '1000', 'x-ratelimit-remaining': '999' } });
      } });
    assert.equal((await read(transportURL, 2)).toString(), 'ok'); assert.equal(calls, 1);
    assert(reports.every((text) => !text.includes(secret)));
  }
  for (const secret of ['fixture token', 'fixture\r\nX-Fixture: injected', 'fixture\tvalue', 'é', '=leading', 'mid=dle', 'x'.repeat(4097)]) {
    assert.throws(() => releasePublicReader({ env: { GITHUB_ACTIONS: 'true', POLARIS_RELEASE_API_TOKEN: secret },
      fetch: async () => assert.fail('invalid credentials must not reach HTTP') }),
    (error) => error.message === 'Invalid dedicated release API credential');
  }
});

// Exercise the production reader entry and WHATWG header normalization, rather
// than calling a validator helper or substituting cached JSON for HTTP responses.
test('conditional HTTP accepts only equal opaque validators across weak/strong and header-name changes', async () => {
  for (const [first, second] of [['W/"Opaque"', '"Opaque"'], ['"Opaque"', 'W/"Opaque"'],
    ['W/"Opaque"', 'W/"Opaque"'], ['"Opaque"', null]]) {
    const calls = [], reports = [];
    const read = releasePublicReader({ env: {}, report: (s) => reports.push(s), fetch: async (url, options) => {
      calls.push({ url, headers: new Headers(options.headers), redirect: options.redirect });
      return calls.length === 1 ? new Response('abc', { headers: { ETag: first } })
        : new Response(null, { status: 304, headers: second === null ? {} : { eTaG: second } });
    } });
    const firstBytes = await read(transportURL, 3); firstBytes.fill(0);
    const secondBytes = await read(transportURL, 3); assert.equal(secondBytes.toString(), 'abc'); secondBytes.fill(0);
    assert.equal((await read(transportURL, 3)).toString(), 'abc');
    assert.equal(calls.length, 3); assert(calls.every((c) => c.url === transportURL && c.redirect === 'manual'));
    assert.equal(calls[0].headers.get('if-none-match'), null);
    assert(calls.slice(1).every((c) => c.headers.get('if-none-match') === first));
    assert(reports.every((s) => !s.includes('Opaque') && !s.includes('abc')));
    assert.equal(reports.filter((s) => s.includes('validator=weak-equivalent')).length,
      second !== null && first !== second ? 1 : 0);
  }
  for (const second of ['W/"opaque"', 'W/"other"', 'w/"Opaque"', '"Opaque", "other"', '*', 'invalid']) {
    let count = 0;
    const read = releasePublicReader({ env: {}, report: () => {}, fetch: async () => ++count === 1
      ? new Response('abc', { headers: { ETag: 'W/"Opaque"' } })
      : new Response(null, { status: 304, headers: { ETAG: second } }) });
    await read(transportURL, 3);
    await assert.rejects(read(transportURL, 3), (e) => /no intact matching metadata/.test(e.message)
      && !e.message.includes('Opaque') && !e.message.includes('W/') && !e.message.includes('abc'));
    assert.equal(count, 2);
  }
});
test('conditional cache requires the exact URL and intact bytes even for weak-equivalent 304', async () => {
  const bytes = Buffer.from('abc'), cache = new Map([[transportURL,
    { etag: 'W/"one"', bytes, size: 3, sha256: digest(bytes) }]]);
  const options = { env: {}, metadataCache: cache, fetch: async () =>
    new Response(null, { status: 304, headers: { ETag: '"one"' } }) };
  await assert.rejects(publicBytes(transportURL + '/releases/17', 3, false, options), /cached=false/);
  await assert.rejects(publicBytes(transportURL, 2, false, options), /withinLimit=false/);
  bytes[0] ^= 1;
  await assert.rejects(publicBytes(transportURL, 3, false, options), /intact=false/);
});
// Retain the real consumeRelease -> verifyExtracted -> verifyReleaseCore ->
// strict signature observer chain. Only OS process boundaries are substituted.
// A successful execFileSync result contains stdout only, as on the failed runner.
test('default native consumer captures codesign stderr/status without exposing the dedicated API token', async () => withFixture(async (f) => {
  const original = { execFileSync: childProcess.execFileSync, spawnSync: childProcess.spawnSync };
  const previous = process.env.POLARIS_RELEASE_API_TOKEN;
  const key = 'mac-arm64', display = f.displays[key];
  let observed = [], scenario = { status: 0, stdout: '', stderr: display };
  process.env.POLARIS_RELEASE_API_TOKEN = 'never_log_native_fixture_secret';
  childProcess.execFileSync = (command, args, settings) => {
    assert.equal(settings.env.POLARIS_RELEASE_API_TOKEN, undefined);
    if (command === 'codesign') {
      observed.push(['stdout-only', ...args]);
      return ''; // This reproduces the broken wrapper's lost successful stderr.
    }
    return ['python', 'python3'].includes(command) ? original.execFileSync(command, args, settings)
      : f.run(command, args, settings);
  };
  childProcess.spawnSync = (command, args, settings) => {
    assert.equal(settings.env.POLARIS_RELEASE_API_TOKEN, undefined);
    assert.equal(settings.encoding, 'utf8'); assert.equal(settings.stdio, 'pipe');
    assert.equal(command, 'codesign'); observed.push([command, ...args]);
    if (args[0] === '--verify') {
      assert.deepEqual(args.slice(0, -1), ['--verify', '--strict']);
      return { status: 0, stdout: '', stderr: '' };
    }
    assert.deepEqual(args.slice(0, -1), ['--display', '--verbose=4']); return scenario;
  };
  syncBuiltinESMExports();
  try {
    for (const bad of [{ status: 0, stdout: '', stderr: '' },
      { status: 0, stdout: '', stderr: display + display },
      { status: 1, stdout: '', stderr: display },
      { status: 0, stdout: '', stderr: `CDHash=${'5'.repeat(40)}\n` }]) {
      scenario = bad;
      await assert.rejects(consumeRelease(f.root, f.manifest, [key], { read: f.read, host: 'darwin' }), /CDHash/);
      assert.deepEqual(resources(f), {});
    }
    scenario = { status: 0, stdout: '', stderr: display };
    const receipts = await consumeRelease(f.root, f.manifest, [key], { read: f.read, host: 'darwin' });
    assert.deepEqual(receipts[key].macCodeSignature, { state: 'signed', cdHash: '4'.repeat(40) });
    assert(!JSON.stringify(receipts).includes('never_log_native_fixture_secret'));
    assert(observed.some(([command, flag]) => command === 'codesign' && flag === '--display'));
    assert(observed.every(([command, ...args]) => command === 'codesign' && !args.includes('--sign')));
  } finally {
    Object.assign(childProcess, original); syncBuiltinESMExports();
    if (previous === undefined) delete process.env.POLARIS_RELEASE_API_TOKEN;
    else process.env.POLARIS_RELEASE_API_TOKEN = previous;
  }
}));


// Same tag/version is intentionally insufficient: actual contract, archived
// bytes and acquisition receipts must change together. Native payloads are inert.
test('same-version repack rejects old receipts and stamps before admitting new core bytes', async () => withFixture(async old => {
  const next = fixture({ coreMarker: 7 });
  try {
    await consumeRelease(old.root, old.manifest, ['linux'], old);
    const core = join(old.root, 'resources/linux/sing-box');
    const previousBytes = readFileSync(core);
    const stamp = JSON.stringify({ 'core:linux': old.contract.version, 'cronet:linux': old.contract.version });
    write(join(old.root, 'resources/.fetch-stamp.json'), stamp);
    assert.equal(next.contract.version, old.contract.version);
    assert.equal(next.contract.tag, old.contract.tag);
    assert.notEqual(next.contract.platforms.linux.core.sha256, old.contract.platforms.linux.core.sha256);
    const contractBytes = readFileSync(join(next.root, next.manifest.desktopRelease.contractPath));
    write(join(old.root, old.manifest.desktopRelease.contractPath), contractBytes);
    assert.throws(() => verifyPackagedRelease(old.root, old.manifest, 'linux', core, old.run), /contract SHA/);
    assert.throws(() => verifyPackagedRelease(old.root, next.manifest, 'linux', core, next.run), /consumption receipt/);
    const before = resources(old);
    const staleArchive = async (...args) => args[0] === next.contract.platforms.linux.releaseAsset.url
      ? old.read(...args) : next.read(...args);
    await assert.rejects(consumeRelease(old.root, next.manifest, ['linux'], { ...next, read: staleArchive }), /archive SHA/);
    assert.deepEqual(resources(old), before, 'failed repack preserves all previous resources');
    const receipts = await consumeRelease(old.root, next.manifest, ['linux'], next);
    assert.equal(receipts.linux.contractSha256, next.manifest.desktopRelease.contractSha256);
    assert.notDeepEqual(readFileSync(core), previousBytes);
    assert.equal(digest(readFileSync(core)), next.contract.platforms.linux.core.sha256);
    assert.equal(readFileSync(join(old.root, 'resources/.fetch-stamp.json'), 'utf8'), stamp,
      'legacy version stamps are neither authority nor silently rewritten');
    verifyPackagedRelease(old.root, next.manifest, 'linux', core, next.run);
  } finally { rmSync(next.root, { recursive: true, force: true }); }
}));

test('NA02 unchanged bytes reject setuid/setgid/sticky on final core, sidecar and notice', async () => {
  if (process.platform === 'win32') return;
  for (const member of ['sing-box', 'libcronet.so', 'core-release/LICENSE']) {
    for (const bit of [0o4000, 0o2000, 0o1000]) await withFixture(async (f) => {
      await consumeRelease(f.root, f.manifest, ['linux'], f);
      const carrier = join(f.root, 'mode-carrier');
      cpSync(join(f.root, 'resources/linux'), carrier, { recursive: true });
      const path = join(carrier, member), before = readFileSync(path);
      fs.chmodSync(path, (fs.statSync(path).mode & 0o777) | bit);
      assert.deepEqual(readFileSync(path), before);
      assert.throws(() => verifyPackagedRelease(f.root, f.manifest, 'linux', join(carrier, 'sing-box'), f.run), /permissions/);
    });
  }
});

// Private raw-transport stores only; no new unit writes the host user's cache.
async function withBlobCache(t, fn, options) {
  const f = fixture(options), parent = mkdtempSync(join(tmpdir(), 'release-blob-cache-'));
  t.after(() => { rmSync(f.root, { recursive: true, force: true }); rmSync(parent, { recursive: true, force: true }); });
  f.blobCacheDir = join(fs.realpathSync(parent), 'store'); f.cacheReports = []; f.report = (line) => f.cacheReports.push(line);
  // Ask the real consumer to initialize only, then deliberately stop at its
  // first public read. No copied filesystem allowlist, payload or new trust API.
  const admissionStop = Error('private cache admission observation only');
  await assert.rejects(consumeRelease(f.root, f.manifest, ['linux'], { ...f, read: async () => { throw admissionStop; } }), error => error === admissionStop);
  f.cacheSupported = f.cacheReports.includes('Release blob cache ready');
  assert.equal(f.cacheReports.includes('Release blob cache unavailable'), !f.cacheSupported);
  assert.equal(cacheBlobs(f).length, 0); f.cacheReports.length = 0;
  await fn(f);
}
const cacheBlobs = (f) => existsSync(f.blobCacheDir) ? readdirSync(f.blobCacheDir).filter(name => name.endsWith('.blob')) : [];
const blobPath = (f, key = 'linux') => join(f.blobCacheDir, f.contract.platforms[key].releaseAsset.sha256 + '.blob');
async function warmBlobCache(f) {
  await consumeRelease(f.root, f.manifest, ['linux'], f);
  if (f.cacheSupported) assert.equal(digest(readFileSync(blobPath(f))), f.contract.platforms.linux.releaseAsset.sha256);
}
const payloadCalls = (f) => f.calls.filter((url) => Object.values(f.contract.platforms).some((row) => row.releaseAsset.url === url));

test('raw cache cold and warm preserve all four actual payloads, inspections and two public snapshots', async t => withBlobCache(t, async f => {
  const inspections = [], run = f.run;
  f.run = (...args) => { if (args[0] === 'go' || args[1][1]?.endsWith('release-archive.py')) inspections.push(args); return run(...args); };
  const cold = await consumeRelease(f.root, f.manifest, undefined, f), coldTree = resources(f), firstInspections = inspections.length;
  const coldCalls = f.calls.length, coldPayloads = payloadCalls(f).length;
  const warm = await consumeRelease(f.root, f.manifest, undefined, f);
  assert.deepEqual(warm, cold); assert.deepEqual(resources(f), coldTree);
  assert.equal(coldPayloads, 4); assert.equal(payloadCalls(f).length - coldPayloads, f.cacheSupported ? 0 : 4);
  assert.equal(f.calls.slice(coldCalls).filter(url => url.startsWith(f.base)).length, 10);
  assert.equal(f.calls.slice(coldCalls).filter(url => url === f.base + '/releases/17').length, 2);
  assert.equal(inspections.length - firstInspections, firstInspections);
  assert.equal(f.cacheReports.filter(x => x.endsWith(' hit')).length, f.cacheSupported ? 4 : 0);
  for (const [key,row] of Object.entries(f.contract.platforms)) verifyPackagedRelease(f.root, f.manifest, key,
    join(f.root, 'resources', key, row.core.member), f.run, 'linux');
}));

test('raw cache corrupt, truncated and another-platform bytes cannot impersonate a pinned archive', async t => withBlobCache(t, async f => {
  await warmBlobCache(f);
  if (!f.cacheSupported) { assert.equal(cacheBlobs(f).length, 0); return; }
  const pin = f.contract.platforms.linux.releaseAsset, original = f.transport.get(pin.url);
  for (const bad of [Buffer.alloc(original.length), original.subarray(0, original.length - 1), f.transport.get(f.contract.platforms.win.releaseAsset.url)]) {
    writeFileSync(blobPath(f), bad); const count = payloadCalls(f).length;
    await consumeRelease(f.root, f.manifest, ['linux'], f);
    assert.equal(payloadCalls(f).length, count + 1); assert.deepEqual(readFileSync(blobPath(f)), original);
    verifyPackagedRelease(f.root, f.manifest, 'linux', join(f.root, 'resources/linux/sing-box'), f.run);
  }
}));

test('raw cache symlinks and linked or foreign-owned metadata are misses, never repaired by chmod', async t => withBlobCache(t, async f => {
  await warmBlobCache(f);
  if (!f.cacheSupported) { assert.equal(cacheBlobs(f).length, 0); return; }
  const outside = join(dirname(f.blobCacheDir), 'outside'); writeFileSync(outside, 'do not overwrite');
  rmSync(blobPath(f)); symlinkSync(outside, blobPath(f));
  await consumeRelease(f.root, f.manifest, ['linux'], f);
  assert.equal(readFileSync(outside, 'utf8'), 'do not overwrite'); assert(fs.lstatSync(blobPath(f)).isSymbolicLink());
  rmSync(blobPath(f)); writeFileSync(blobPath(f), f.transport.get(f.contract.platforms.linux.releaseAsset.url), { mode: 0o600 });
  const hardlink = join(dirname(f.blobCacheDir), 'linked'); fs.linkSync(blobPath(f), hardlink);
  const count = payloadCalls(f).length; await consumeRelease(f.root, f.manifest, ['linux'], f);
  assert.equal(payloadCalls(f).length, count + 1); assert.equal(fs.lstatSync(blobPath(f)).nlink, 2);
  fs.unlinkSync(hardlink);
  const original = fs.lstatSync, mode = original(f.blobCacheDir).mode;
  fs.lstatSync = (path, ...args) => {
    const st = original(path, ...args);
    if (String(path) !== f.blobCacheDir) return st;
    return Object.defineProperty(Object.create(st), 'uid', { value: process.getuid() + 1 });
  }; syncBuiltinESMExports();
  try { const n = payloadCalls(f).length; await consumeRelease(f.root, f.manifest, ['linux'], f); assert.equal(payloadCalls(f).length, n + 1); }
  finally { fs.lstatSync = original; syncBuiltinESMExports(); }
  assert.equal(fs.lstatSync(f.blobCacheDir).mode, mode);
}));

test('raw cache full byte or entry capacity and abandoned partial entries never delete unknown bytes', async t => withBlobCache(t, async f => {
  if (!f.cacheSupported) { await consumeRelease(f.root, f.manifest, ['linux'], f); assert.equal(cacheBlobs(f).length, 0); return; }
  mkdirSync(f.blobCacheDir, { mode: 0o700, recursive: true });
  const names = ['0'.repeat(64) + '.blob', '1'.repeat(64) + '.blob'];
  for (const name of names) { const fd = fs.openSync(join(f.blobCacheDir, name), 'wx', 0o600); try { fs.ftruncateSync(fd, 512 * 1024 * 1024); } finally { fs.closeSync(fd); } }
  await consumeRelease(f.root, f.manifest, ['linux'], f);
  assert.deepEqual(readdirSync(f.blobCacheDir).sort(), names); assert.equal(fs.statSync(join(f.blobCacheDir,names[0])).size, 512 * 1024 * 1024);
  for (const name of names) rmSync(join(f.blobCacheDir,name));
  for (let n = 0; n < 64; n++) writeFileSync(join(f.blobCacheDir, n.toString(16).padStart(64,'0') + '.blob'), '', { mode: 0o600 });
  await consumeRelease(f.root, f.manifest, ['linux'], f); assert.equal(readdirSync(f.blobCacheDir).length,64); assert(!existsSync(blobPath(f)));
  for (const name of readdirSync(f.blobCacheDir)) rmSync(join(f.blobCacheDir,name));
  writeFileSync(join(f.blobCacheDir,'.pending'), 'unfinished-owner', { mode: 0o600 });
  await consumeRelease(f.root, f.manifest, ['linux'], f); assert.equal(readFileSync(join(f.blobCacheDir,'.pending'),'utf8'),'unfinished-owner'); assert(!existsSync(blobPath(f)));
  writeFileSync(join(f.blobCacheDir,'foreign-file'), 'foreign');
  await consumeRelease(f.root, f.manifest, ['linux'], f); assert.equal(readFileSync(join(f.blobCacheDir,'foreign-file'),'utf8'),'foreign');
}));

test('raw cache read-only storage and publication failure cannot replace successful consumer results', async t => withBlobCache(t, async f => {
  await warmBlobCache(f);
  if (!f.cacheSupported) { assert.equal(cacheBlobs(f).length, 0); return; }
  const core = readFileSync(join(f.root,'resources/linux/sing-box')), original = fs.renameSync;
  fs.renameSync = (from,to) => { if (String(from) === join(f.blobCacheDir,'.pending')) throw Object.assign(new Error('cache publication denied'), {code:'EACCES'}); return original(from,to); };
  syncBuiltinESMExports();
  try {
    rmSync(blobPath(f)); await consumeRelease(f.root,f.manifest,['linux'],f);
    assert.deepEqual(readFileSync(join(f.root,'resources/linux/sing-box')),core); assert(!existsSync(join(f.blobCacheDir,'.pending'))); assert(!existsSync(blobPath(f)));
  } finally { fs.renameSync=original; syncBuiltinESMExports(); }
  await warmBlobCache(f); fs.chmodSync(f.blobCacheDir,0o500);
  try { await consumeRelease(f.root,f.manifest,['linux'],f); assert.equal(fs.lstatSync(f.blobCacheDir).mode & 0o777,0o500); }
  finally { fs.chmodSync(f.blobCacheDir,0o700); }
  assert.deepEqual(readFileSync(join(f.root,'resources/linux/sing-box')),core);
}));

test('raw cache shared concurrent consumers and busy writer never publish partial transport or reuse receipts', async t => withBlobCache(t, async f => {
  const other=mkdtempSync(join(tmpdir(),'release-cache-other-'));t.after(()=>rmSync(other,{recursive:true,force:true}));cpSync(f.root,other,{recursive:true});
  const [first,second]=await Promise.all([consumeRelease(f.root,f.manifest,['linux'],f),consumeRelease(other,f.manifest,['linux'],f)]);
  assert.deepEqual(first.linux.producer,second.linux.producer); assert.deepEqual(readFileSync(join(other,'resources/linux/sing-box')),readFileSync(join(f.root,'resources/linux/sing-box')));
  if (!f.cacheSupported) { assert.equal(cacheBlobs(f).length, 0); return; }
  assert.equal(digest(readFileSync(blobPath(f))),f.contract.platforms.linux.releaseAsset.sha256); assert(!existsSync(join(f.blobCacheDir,'.pending'))); assert(!existsSync(join(f.blobCacheDir,'.writer')));
  // A distinct process holds the real same-filesystem write lock while this
  // ordinary consumer is forced to use its complete original network path.
  const child=childProcess.spawn(process.execPath,['--input-type=module','-e',`import {mkdirSync,rmdirSync} from 'node:fs';const p=process.argv[1];mkdirSync(p,{mode:0o700});process.stdout.write('ready\\n');process.stdin.resume();process.stdin.on('end',()=>{rmdirSync(p);});`,join(f.blobCacheDir,'.writer')],{stdio:['pipe','pipe','pipe'],timeout:10_000,killSignal:'SIGTERM'});
  let error='';child.stderr.on('data',b=>{error+=b;});
  const exited=new Promise((resolve,reject)=>{child.once('error',reject);child.once('exit',code=>code===0?resolve():reject(Error(error||`owner exit ${code}`)));});
  try {
    await new Promise((resolve,reject)=>{child.stdout.once('data',()=>resolve());child.once('error',reject);child.once('exit',code=>reject(Error(error||`owner exited before readiness: ${code}`)));});
    const n=payloadCalls(f).length; await consumeRelease(f.root,f.manifest,['linux'],f);assert.equal(payloadCalls(f).length,n+1);
  } finally {child.stdin.end();await exited;}
  assert(!existsSync(join(f.blobCacheDir,'.writer'))); assert.equal(digest(readFileSync(blobPath(f))),f.contract.platforms.linux.releaseAsset.sha256);
}));

test('raw cache warm hits preserve fresh metadata/profile/transaction gates after independent cache reset', async t => withBlobCache(t, async f => {
  await warmBlobCache(f); const installed=resources(f), originalRead=f.read, originalRun=f.run;
  const denial=Error('ordinary public metadata denied'); await assert.rejects(consumeRelease(f.root,f.manifest,['linux'],{...f,read:async()=>{throw denial;}}),error=>error===denial);assert.deepEqual(resources(f),installed);
  await assert.rejects(consumeRelease(f.root,f.manifest,['linux'],{...f,run:(command,args,options)=>command==='go' && args[0]==='version'?originalRun(command,args,options).replace('CGO_ENABLED=0','CGO_ENABLED=1'):originalRun(command,args,options)}),/BuildInfo|build settings/);assert.deepEqual(resources(f),installed);
  let snapshots=0;
  await assert.rejects(consumeRelease(f.root,f.manifest,['linux'],{...f,read:async url=>{
    const bytes=await originalRead(url);
    if(url===f.base+'/releases/17' && ++snapshots===2){const changed=JSON.parse(bytes);changed.assets[0].digest=null;return encode(changed);}return bytes;
  }}),/identity changed/);assert.equal(snapshots,2);assert.deepEqual(resources(f),installed);
  if(f.cacheSupported){
    for(const mutation of ['replace','delete','permission','EIO','initial-EIO']){
      const pin=f.contract.platforms.linux.releaseAsset, originalOpen=fs.openSync; let denied=mutation==='initial-EIO'; snapshots=0;const networkBefore=payloadCalls(f).length;
      fs.openSync=(path,...args)=>{if(denied && String(path)===blobPath(f))throw Object.assign(Error('cache-only '+mutation),{code:mutation==='permission'?'EACCES':'EIO'});return originalOpen(path,...args);};
      syncBuiltinESMExports();
      try{
        const receipt=await consumeRelease(f.root,f.manifest,['linux'],{...f,read:async url=>{
          const bytes=await originalRead(url);
          if(url===f.base+'/releases/17' && ++snapshots===2){
            if(mutation==='replace')writeFileSync(blobPath(f),Buffer.alloc(pin.bytes));
            else if(mutation==='delete')rmSync(blobPath(f));
            else denied=true;
          }
          return bytes;
        }});
        assert.equal(snapshots,2);assert.deepEqual(resources(f),installed);
        assert.equal(payloadCalls(f).length-networkBefore,mutation==='initial-EIO'?1:0);
        verifyPackagedRelease(f.root,f.manifest,'linux',join(f.root,'resources/linux/sing-box'),f.run);
      }finally{fs.openSync=originalOpen;syncBuiltinESMExports();}
      // Restore only this test's private inert transport for the next mutation.
      writeFileSync(blobPath(f),f.transport.get(pin.url),{mode:0o600});
    }
  }
  await assert.rejects(consumeRelease(f.root,f.manifest,['linux'],{...f,rename:(from,to)=>{if(from.endsWith('/stage/linux') || from.endsWith('\\stage\\linux'))throw Error('transaction rejected');return renameSync(from,to);}}),/transaction rejected/);assert.deepEqual(resources(f),installed);
}));

test('raw cache only receives a committed fully verified acquisition, never a cold rollback or bad actual ABI', async t => {
  await withBlobCache(t,async f=>{
    await assert.rejects(consumeRelease(f.root,f.manifest,['linux'],{...f,rename:()=>{throw Error('cold transaction rejected');}}),/cold transaction rejected/);
    assert.deepEqual(resources(f),{}); if(f.cacheSupported)assert.deepEqual(readdirSync(f.blobCacheDir),[]);
  });
  await withBlobCache(t,async f=>{
    if(f.cacheSupported){mkdirSync(f.blobCacheDir,{mode:0o700,recursive:true});writeFileSync(blobPath(f),f.transport.get(f.contract.platforms.linux.releaseAsset.url),{mode:0o600});}
    const n=payloadCalls(f).length;await assert.rejects(consumeRelease(f.root,f.manifest,['linux'],f),/ELF|architecture|machine/);
    assert.deepEqual(resources(f),{});assert.equal(payloadCalls(f).length-n,f.cacheSupported?0:1);
  },{linuxMachine:183});
});


test('raw cache short writes stay unpublished and partial I/O failures retain the normal committed result', async t => {
  for(const fail of [false,true])await withBlobCache(t,async f=>{
    if(!f.cacheSupported){await consumeRelease(f.root,f.manifest,['linux'],f);assert.equal(cacheBlobs(f).length, 0);return;}
    const original={openSync:fs.openSync,closeSync:fs.closeSync,writeSync:fs.writeSync}, pending=new Set();let writes=0;
    fs.openSync=(path,...args)=>{const fd=original.openSync(path,...args);if(String(path)===join(f.blobCacheDir,'.pending'))pending.add(fd);return fd;};
    fs.closeSync=fd=>{try{return original.closeSync(fd);}finally{pending.delete(fd);}};
    fs.writeSync=(fd,buffer,offset,length,...args)=>{
      if(!pending.has(fd))return original.writeSync(fd,buffer,offset,length,...args);
      assert(!existsSync(blobPath(f)),'partial transport must not be published');writes++;
      if(fail && writes===2)throw Object.assign(Error('inert partial cache write failure'),{code:'EIO'});
      return original.writeSync(fd,buffer,offset,Math.min(length,17),...args);
    };syncBuiltinESMExports();
    try{await consumeRelease(f.root,f.manifest,['linux'],f);}
    finally{Object.assign(fs,original);syncBuiltinESMExports();}
    assert(writes>1);assert(!existsSync(join(f.blobCacheDir,'.pending')));assert.equal(existsSync(blobPath(f)),!fail);
    if(!fail)assert.equal(digest(readFileSync(blobPath(f))),f.contract.platforms.linux.releaseAsset.sha256);
    verifyPackagedRelease(f.root,f.manifest,'linux',join(f.root,'resources/linux/sing-box'),f.run);
  });
});


test('raw cache unavailable filesystem or ancestry keeps normal acquisition and supported stores still hit', async t => withBlobCache(t,async f=>{
  const original={statfsSync:fs.statfsSync,lstatSync:fs.lstatSync};
  const canonicalParent=fs.realpathSync(dirname(f.blobCacheDir)), canonicalStore=join(canonicalParent,'store');
  // The empty directory is test-owned even when actual cache admission declined.
  mkdirSync(canonicalStore,{mode:0o700,recursive:true});
  const storeDev=original.lstatSync(canonicalStore).dev;
  assert.equal(typeof storeDev,'number');assert(Number.isSafeInteger(storeDev) && storeDev>=0);
  const differentSystemDev=storeDev===0?1:0, adjacentSystemDev=storeDev===0?1:storeDev-1;
  assert.notEqual(differentSystemDev,storeDev);assert.notEqual(adjacentSystemDev,storeDev);assert.equal(Math.abs(adjacentSystemDev-storeDev),1);
  for(const fault of ['unknown-filesystem','unsafe-ancestor','adjacent-device']){
    fs.statfsSync=(path,...args)=>{
      const st=original.statfsSync(path,...args);
      return ['unknown-filesystem','adjacent-device'].includes(fault) && String(path)===canonicalStore?Object.defineProperty(Object.create(st),'type',{value:-1}):st;
    };
    fs.lstatSync=(path,...args)=>{
      const st=original.lstatSync(path,...args);
      if(fault==='unsafe-ancestor' && String(path)===canonicalParent)return Object.defineProperty(Object.create(st),'mode',{value:st.mode|0o022});
      if(['unknown-filesystem','adjacent-device'].includes(fault) && process.platform==='darwin' && String(path)==='/private/var/tmp'){
        // Model an already different adjacent mount directly at the production
        // OS read boundary. Never add one to the system tmp's original device.
        const dev=fault==='adjacent-device'?adjacentSystemDev:differentSystemDev;assert.notEqual(dev,storeDev);
        return Object.defineProperty(Object.create(st),'dev',{value:dev});
      }
      return st;
    };syncBuiltinESMExports();
    try{
      f.cacheReports.length=0;const n=payloadCalls(f).length, at=f.calls.length;
      await consumeRelease(f.root,f.manifest,['linux'],f);
      assert.equal(payloadCalls(f).length,n+1);assert.equal(f.calls.slice(at).filter(url=>url===f.base+'/releases/17').length,2);
      assert(f.cacheReports.includes('Release blob cache unavailable'));assert(!f.cacheReports.includes('Release blob cache hit'));assert.equal(cacheBlobs(f).length,0);
      verifyPackagedRelease(f.root,f.manifest,'linux',join(f.root,'resources/linux/sing-box'),f.run);
    }finally{Object.assign(fs,original);syncBuiltinESMExports();}
  }
  f.cacheReports.length=0;await consumeRelease(f.root,f.manifest,['linux'],f);
  const n=payloadCalls(f).length;await consumeRelease(f.root,f.manifest,['linux'],f);
  assert.equal(payloadCalls(f).length-n,f.cacheSupported?0:1);
  assert.equal(f.cacheReports.includes('Release blob cache hit'),f.cacheSupported);
}));
