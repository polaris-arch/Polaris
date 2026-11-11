// Deliberately synthetic contract fixtures. Never publication or smoke evidence.
import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { dirname } from 'node:path';
import { cronetFixture } from '../lib/cronet-payload.test.mjs';
import fs, { mkdtempSync, writeFileSync, rmSync, mkdirSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { nativeReleaseGoVersion } from '../native-payload-input.mjs';
import { digest, desktopSourceGoVersion } from './source-graph.mjs';
import { consumeRelease, verifyPackagedRelease, RELEASE_TARGETS, validateReleaseContract, releasePayloadMembers, publicBytes, verifyReleaseCore, releaseSingTunDependency, releaseInstallationContract, UPSTREAM_INSTALLATION_DOMAIN } from './release-consumer.mjs';
import { selectedCoreManifest } from '../lib/core-identity.mjs';
import { managedFormalSelection } from './managed-mesh-core.mjs';
import { upstreamMetadata, upstreamPayloadPins, contractPublicAssets } from './upstream-release.mjs';
// Frozen G4 source coordinates, with synthetic payloads only. These tests do
// not assert that a release exists or that any native artifact has passed.
const hash = 'a'.repeat(64), commit = '55300c55560bcc604fd9675c5526b5713db81cc1',
  tree = '7767ba9a9901a061ff07db7716baed7896cfefbc';
function fixture(version = '1.15.0-polaris-alpha.12') {
  let id = 1;
  const contract = { schema: 'polaris-upstream-native-release-consumer-v1', providerContract: 'polaris-upstream-native-release-v1',
    repository: 'polaris-arch/sing-box-polaris', repositoryId: 1421657509, releaseId: 100,
    version, tag: `v${version}`, tagObject: 'd'.repeat(40), sourceCommit: commit, sourceTree: tree, sourcePolicy: { goVersion: '1.26.8' },
    producer: { repository: 'polaris-arch/sing-box-polaris', repositoryId: 1421657509, workflow: '.github/workflows/build.yml', workflowCommit: commit, runId: 1, runAttempt: 1, nativeJobs: Object.fromEntries(Object.entries({ linux: 'linux-amd64-purego', win: 'windows-amd64', 'mac-x64': 'darwin-amd64', 'mac-arm64': 'darwin-arm64' }).map(([key, id], i) => [key, { id: i + 1, name: `Native final-package Cronet and gVisor smoke (${id})` }])) },
    legalFiles: ['LICENSE', 'NOTICE', 'go-1.26.8/LICENSE'].map((name) => ({ path: `licenses/${name}`, content: name, bytes: Buffer.byteLength(name), sha256: digest(name) })), platforms: {} };
  const asset = (name) => ({ name, bytes: 1, sha256: hash, assetId: id++, releaseId: 100,
    url: `https://github.com/${contract.repository}/releases/download/${contract.tag}/${name}` });
  contract.sourceArchive = { name: `sing-box-polaris-${commit}.tar.gz`, bytes: 1, sha256: hash,
    url: `https://github.com/${contract.repository}/archive/${commit}.tar.gz` };
  for (const [key, target] of Object.entries(RELEASE_TARGETS)) {
    const root = `sing-box-${contract.version}-${target.os}-${target.arch}`;
    const payload = (member) => ({ member, bytes: 1, sha256: hash });
    const linkage = target.sidecar ? 'shared-purego' : 'static-cgo';
    const row = { schema: 'polaris-upstream-native-receipt-v1', id: root, target: `${target.os}/${target.arch}`,
      variant: { linkage, libc: '', goarm: '', gomips: '', go386: '', legacy: false },
      version: contract.version, tag: contract.tag, sourceCommit: commit, sourceTree: tree, goVersion: 'go1.26.8', buildID: '', rawBuildInfo: 'fixture: go1.26.8\n',
      tags: ['with_gvisor', 'with_naive_outbound', ...(target.sidecar ? ['with_purego'] : [])].join(','),
      sourceIdentity: { sourceOverlay: 0, goModSha256: hash, goSumSha256: hash, replacements: [{ module: 'github.com/sagernet/sing-tun', path: './third_party/sing-tun', tree }] },
      buildGraph: { settings: {}, normalModules: [{ module: 'example.invalid/fixture', version: 'v1.0.0', h1: 'h1:YQ=' }], prebuiltAcquisitionModules: [],
        localReplacements: [{ module: 'github.com/sagernet/sing-tun', version: 'v1.0.0', h1: '', replacement: { module: './third_party/sing-tun', version: '(devel)', h1: '' } }] },
      core: payload(target.core), sidecar: target.sidecar ? payload(target.sidecar) : null,
      archive: { name: root + target.suffix, root, bytes: 1, sha256: hash, members: Object.fromEntries([target.core, 'LICENSE', ...(target.sidecar ? [target.sidecar] : [])].map((name) => [`${root}/${name}`, hash])) },
      signatureObservation: key.startsWith('mac-') ? { state: 'unsigned', cdHash: null, trustValidated: false } : null,
      finalPackageCronetSmoke: { status: 'passed', os: target.os, arch: target.arch, go: 'go1.26.8', gvisorCompiled: true, naiveOutbound: true, linkage,
        version: '150.0.0.1', librarySha256: target.sidecar ? hash : null, coreSha256: hash }, releaseAsset: asset(root + target.suffix) };
    contract.platforms[key] = row;
  }
  // Original producer rows, as recorded in each installed provenance.json.
  const index = structuredClone({ platforms: Object.values(contract.platforms).map(({ releaseAsset, ...row }) => row) });
  for (const row of index.platforms) {
    row.archive.releaseName = 'sing-box-polaris-' + row.archive.name.slice('sing-box-'.length);
  }
  for (const row of Object.values(contract.platforms)) {
    row.archive.releaseName = 'sing-box-polaris-' + row.archive.name.slice('sing-box-'.length).replace('-polaris-', '-');
    row.releaseAsset.name = row.archive.releaseName;
    row.releaseAsset.url = `https://github.com/${contract.repository}/releases/download/${contract.tag}/${row.releaseAsset.name}`;
  }
  return { contract, index };
}

function installationFixture(t, contract = fixture().contract) {
  const root = mkdtempSync(join(tmpdir(), 'upstream-installation-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'scripts/desktop-core'), { recursive: true });
  const raw = Buffer.from(JSON.stringify(contract) + '\n');
  writeFileSync(join(root, 'scripts/desktop-core/release-contract.json'), raw);
  const manifest = { desktopRelease: { schema: 'polaris-app-release-selection-v1',
    contractPath: 'scripts/desktop-core/release-contract.json', contractSha256: digest(raw) } };
  return { root, raw, manifest, contract };
}

test('formal upstream installation derives a separate exact-contract identity without rewriting producer evidence', t => {
  const f = installationFixture(t), before = JSON.stringify(f.contract);
  const installed = releaseInstallationContract(f.root, f.manifest);
  const expected = digest(Buffer.concat([Buffer.from(UPSTREAM_INSTALLATION_DOMAIN), f.raw]));
  assert.notEqual(expected, digest(f.raw), 'temporary candidate digest is a different domain');
  for (const key of Object.keys(RELEASE_TARGETS)) {
    assert.equal(installed.platforms[key].providerFingerprint, expected);
    assert.deepEqual(installed.platforms[key].core, f.contract.platforms[key].core);
    assert.equal(selectedCoreManifest(installed, key, '1.0.0').core.helperSha256, null);
    assert.equal(f.contract.platforms[key].providerFingerprint, undefined);
  }
  assert.equal(JSON.stringify(f.contract), before);
  assert.deepEqual(readFileSync(join(f.root, f.manifest.desktopRelease.contractPath)), f.raw);
  writeFileSync(join(f.root, f.manifest.desktopRelease.contractPath), Buffer.concat([f.raw, Buffer.from(' ')]));
  assert.throws(() => releaseInstallationContract(f.root, f.manifest), /SHA differs/);
});
test('formal installation refuses a non-canonical contract path and copied provider labels', t => {
  const f = installationFixture(t);
  const pending = structuredClone(f.manifest);
  pending.desktopRelease.contractPath = 'scripts/desktop-core/pending-release-contract.json';
  writeFileSync(join(f.root, pending.desktopRelease.contractPath), f.raw);
  assert.throws(() => releaseInstallationContract(f.root, pending), /selection incomplete/);
  for (const name of ['providerFingerprint', 'providerIdentity']) {
    const bad = structuredClone(f.contract); bad.platforms.linux[name] = 'a'.repeat(64);
    const copy = installationFixture(t, bad);
    assert.throws(() => releaseInstallationContract(copy.root, copy.manifest), /unverified producer/);
  }
  const changed = structuredClone(f.contract); changed.producer.runAttempt++;
  const copy = installationFixture(t, changed);
  assert.notEqual(releaseInstallationContract(copy.root, copy.manifest).platforms.linux.providerFingerprint,
    releaseInstallationContract(f.root, f.manifest).platforms.linux.providerFingerprint);
});
test('formal managed checks select exact native rows and full source version on all four hosts', t => {
  const contract = fixture().contract;
  for (const [key, row] of Object.entries(contract.platforms)) row.buildGraph.settings.CGO_ENABLED = RELEASE_TARGETS[key].cgo;
  const f = installationFixture(t, contract);
  for (const [os, arch, key] of [['linux','x64','linux'], ['win32','x64','win'], ['darwin','x64','mac-x64'], ['darwin','arm64','mac-arm64']]) {
    const chosen = managedFormalSelection(f.root, f.manifest, { os, arch });
    assert.equal(chosen.key, key);
    assert.ok(chosen.expected.includes(`Revision: ${contract.sourceCommit}\n`));
    assert.ok(chosen.expected.includes(`Environment: ${contract.platforms[key].goVersion} ${contract.platforms[key].target}\n`));
  }
  assert.throws(() => managedFormalSelection(f.root, f.manifest, { os: 'linux', arch: 'arm64' }), /native desktop/);
});
test('upstream native contract keeps actual empty BuildID and unchanged native archive', () => {
  const { contract } = fixture();
  assert.equal(validateReleaseContract(contract), contract);
  assert.equal(contractPublicAssets(contract).length, 4);
  assert.ok(!releasePayloadMembers(contract, 'linux').includes(`core-release/source/${contract.sourceArchive.name}`));
  assert.equal(upstreamPayloadPins(contract, contract.platforms.linux)['licenses/LICENSE'], digest('LICENSE'));
});
test('repository, archive, legal and recipe facts cannot be fabricated by relabeling', () => {
  const changes = [
    (c) => { c.repositoryId = 1407705262; },
    (c) => { c.platforms.linux.variant.libc = 'musl'; },
    (c) => { c.platforms.linux.archive.root = 'polaris-box-legacy'; },
    (c) => { c.platforms.linux.tags = 'with_gvisor'; },
    (c) => { c.sourceArchive.url = c.sourceArchive.url.replace('sing-box-polaris', 'polaris-box'); },
    (c) => { c.legalFiles[0].content += 'changed'; },
  ];
  for (const change of changes) { const { contract } = fixture(); change(contract); assert.throws(() => validateReleaseContract(contract)); }
});
test('credential scope never covers query-string API requests', async () => {
  const seen = [], fetch = async (url, options) => { seen.push(options.headers.Authorization); return new Response('ok'); };
  const options = { repository: 'polaris-arch/sing-box-polaris', fetch, env: { GITHUB_ACTIONS: 'true', POLARIS_RELEASE_API_TOKEN: 'fixture' } };
  for (const url of [
    `https://api.github.com/repos/polaris-arch/sing-box-polaris/git/trees/${tree}?recursive=1`,
    `https://api.github.com/repos/polaris-arch/polaris-box/git/trees/${tree}?recursive=1`,
    `https://api.github.com/repos/polaris-arch/sing-box-polaris/git/trees/${tree}?recursive=1&x=1`,
    'https://api.github.com/repos/polaris-arch/sing-box-polaris?recursive=1',
  ]) await publicBytes(url, 10, false, options);
  assert.deepEqual(seen, [undefined, undefined, undefined, undefined]);
});
test('actual raw BuildInfo cannot be replaced by matching normalized fields', () => {
  const { contract } = fixture(), row = contract.platforms.linux;
  const bytes = Buffer.alloc(64); Buffer.from([127, 69, 76, 70, 2, 1]).copy(bytes); bytes.writeUInt16LE(62, 18);
  row.core = { ...row.core, bytes: bytes.length, sha256: digest(bytes) };
  row.buildGraph.settings = { GOOS: 'linux', GOARCH: 'amd64', CGO_ENABLED: '0', '-trimpath': 'true', '-tags': row.tags };
  const raw = `original-path: go1.26.8\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n\tmod\tgithub.com/sagernet/sing-box\t(devel)\n`
    + '\tdep\texample.invalid/fixture\tv1.0.0\th1:YQ=\n\tdep\tgithub.com/sagernet/sing-tun\tv1.0.0\n\t=>\t./third_party/sing-tun\t(devel)\n'
    + Object.entries(row.buildGraph.settings).map(([k,v]) => `\tbuild\t${k}=${v}\n`).join('');
  row.rawBuildInfo = raw;
  const root = mkdtempSync(join(tmpdir(), 'upstream-buildinfo-fixture-')), path = join(root, 'sing-box');
  writeFileSync(path, bytes);
  const run = (actual, buildID = '') => (command, args) => args[0] === 'version' ? actual : buildID;
  try {
    verifyReleaseCore(path, row, 'linux', run(raw.replace('original-path:', 'actual-local-path:')));
    // Same parsed settings but an extra raw line must not disappear in normalization.
    assert.throws(() => verifyReleaseCore(path, row, 'linux', run(raw + '\tignored\textra\n')), /raw BuildInfo/);
    assert.throws(() => verifyReleaseCore(path, row, 'linux', run(raw, 'invented-build-id')), /BuildID/);
    assert.throws(() => verifyReleaseCore(path, row, 'linux', run(raw.replace('go1.26.8', 'go1.25.5'))), /Go version/);
    assert.throws(() => verifyReleaseCore(path, row, 'linux', run(raw + '\tbuild\tGOOS=linux\n')), /Duplicate/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
test('actual desktop toolchain-selection entry reads the selected release contract', () => {
  const { contract } = fixture(), root = mkdtempSync(join(tmpdir(), 'upstream-go-selection-'));
  const bytes = JSON.stringify(contract) + '\n';
  const selection = { schema: 'polaris-app-release-selection-v1', contractPath: 'scripts/desktop-core/release-contract.json',
    contractSha256: digest(bytes) };
  try {
    mkdirSync(join(root, 'scripts/desktop-core'), { recursive: true });
    writeFileSync(join(root, selection.contractPath), bytes);
    assert.equal(desktopSourceGoVersion(root, { desktopRelease: selection }), '1.26.8');
    const actualRoot = fileURLToPath(new URL('../../', import.meta.url));
    const selectedManifest = JSON.parse(readFileSync(join(actualRoot, 'src-tauri/core-manifest.json')));
    const selectedContract = JSON.parse(readFileSync(join(actualRoot, selectedManifest.desktopRelease.contractPath)));
    assert.equal(desktopSourceGoVersion(actualRoot, selectedManifest), selectedContract.sourcePolicy.goVersion);
    contract.platforms.win.goVersion = 'go1.25.5';
    const changed = JSON.stringify(contract); selection.contractSha256 = digest(changed);
    writeFileSync(join(root, selection.contractPath), changed);
    assert.throws(() => desktopSourceGoVersion(root, { desktopRelease: selection }), /toolchain/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
test('Rust dependency-gate Node entry selects one sing-tun row without dropping the remote SDK graph', () => {
  const { contract } = fixture(), row = contract.platforms.linux;
  const selected = row.buildGraph.localReplacements[0];
  row.buildGraph.localReplacements.unshift({ module: 'github.com/sagernet/tailscale', version: 'v1.0.0', h1: '',
    replacement: { module: 'github.com/polaris-arch/tailscale', version: 'v1.0.1', h1: 'h1:YQ=' } });
  const expected = { module: selected.module, version: selected.version, h1: selected.h1 };
  assert.deepEqual(JSON.parse(JSON.stringify(releaseSingTunDependency(row))), expected);
  row.buildGraph.localReplacements.push(structuredClone(selected));
  assert.throws(() => releaseSingTunDependency(row), /Unique/);
  row.buildGraph.localReplacements = [];
  assert.throws(() => releaseSingTunDependency(row), /Unique/);
});

test('Polaris branded release version is preserved in every original native recipe', () => {
  const { contract } = fixture();
  validateReleaseContract(contract);
  assert.equal(contract.tag, 'v1.15.0-polaris-alpha.12');
  for (const [key, target] of Object.entries(RELEASE_TARGETS)) {
    assert.equal(contract.platforms[key].archive.root, `sing-box-1.15.0-polaris-alpha.12-${target.os}-${target.arch}`);
    assert.equal(contract.platforms[key].core.member, target.os === 'windows' ? 'sing-box.exe' : 'sing-box');
  }
  for (const version of ['1.15.0-alpha.12', '1.15.0-alpha.12.polaris.1', '1.15.0-polaris-alpha.012', '1.15.0-extended-alpha.12']) {
    const changed = structuredClone(contract); changed.version = version; changed.tag = `v${version}`;
    assert.throws(() => validateReleaseContract(changed), /identity/);
  }
});
// These remain deliberately synthetic bytes. The real b024 source coordinates
// exercise admission policy, never claim publication or successful native output.
function utlsFixture() {
  const { contract } = fixture();
  contract.sourceCommit = 'b0247cc930bc892a0ea03138906b548b6e84ac50';
  contract.sourceTree = 'b4b670f51d1c48b3c0be9ab882152e3f25a5d9b4';
  contract.producer.workflowCommit = contract.sourceCommit;
  for (const row of Object.values(contract.platforms)) {
    row.sourceCommit = contract.sourceCommit; row.sourceTree = contract.sourceTree;
    row.goVersion = 'go1.27.2'; row.rawBuildInfo = 'fixture: go1.27.2\n';
    row.finalPackageCronetSmoke.go = 'go1.27.2';
  }
  return contract;
}
// Test-only facts. No real candidate/release/asset is selected by these values.
function reviewedFixture() {
  const contract = utlsFixture();
  contract.sourceCommit = '1'.repeat(40); contract.sourceTree = '2'.repeat(40);
  contract.producer.workflowCommit = contract.sourceCommit;
  contract.sourcePolicy = { schema: 'polaris-reviewed-kernel-source-v4',
    sourceCommit: contract.sourceCommit, sourceTree: contract.sourceTree,
    goDirective: '1.27.0', goVersion: '1.27.2', toolchain: 'go1.27.2',
    goModSha256: hash, goSumSha256: hash,
    metacubex: { module: 'github.com/metacubex/utls', version: 'v1.8.8',
      replacementPath: './third_party/metacubex-utls', replacementTree: '3'.repeat(40), replacementFileCount: 355,
      bindingPath: 'release/dependency-sources/metacubex-utls-replacement.json', bindingSha256: '4'.repeat(64),
      patchPath: 'release/dependency-sources/metacubex-utls-reality-alpn.patch', patchSha256: '5'.repeat(64),
      upstreamCommit: '6'.repeat(40), upstreamH1: 'h1:' + 'A'.repeat(43) + '=', upstreamZipSha256: '7'.repeat(64) } };
  const directories = {
    'github.com/sagernet/sing-tun': 'sing-tun', 'github.com/sagernet/tailscale': 'tailscale',
    'github.com/metacubex/utls': 'metacubex-utls', 'github.com/sagernet/cronet-go': 'cronet-go',
    'github.com/sagernet/fswatch': 'fswatch', 'github.com/sagernet/sing': 'sing',
    'github.com/sagernet/sing-quic': 'sing-quic', 'github.com/sagernet/wireguard-go': 'wireguard-go',
    'github.com/database64128/tfo-go/v2': 'tfo-go', 'github.com/sagernet/sing-vmess': 'sing-vmess',
    'github.com/libp2p/go-nat': 'go-nat',
  };
  contract.sourcePolicy.vendors = Object.fromEntries(Object.entries(directories).map(([name, directory], index) => [name,
    name === contract.sourcePolicy.metacubex.module
      ? Object.fromEntries(['version', 'replacementPath', 'replacementTree', 'replacementFileCount']
        .map(key => [key, contract.sourcePolicy.metacubex[key]]))
      : { version: 'v0.0.0-fixture', replacementPath: `./third_party/${directory}`,
        replacementTree: (index + 1).toString(16).repeat(40), replacementFileCount: index + 1 }]));
  for (const row of Object.values(contract.platforms)) {
    row.sourceCommit = contract.sourceCommit; row.sourceTree = contract.sourceTree;
    row.sourceIdentity.replacements = Object.entries(contract.sourcePolicy.vendors).map(([module, pin]) =>
      ({ module, path: pin.replacementPath, tree: pin.replacementTree }));
    row.buildGraph.localReplacements = Object.entries(contract.sourcePolicy.vendors).map(([module, pin]) =>
      ({ module, version: pin.version, h1: '', replacement: { module: pin.replacementPath, version: '(devel)', h1: '' } }));
  }
  return contract;
}
test('actual Go1.27.2 BuildInfo preserves real local devel identity with no invented h1', () => {
  const contract = reviewedFixture(), row = contract.platforms.linux;
  const bytes = Buffer.alloc(64); Buffer.from([127, 69, 76, 70, 2, 1]).copy(bytes); bytes.writeUInt16LE(62, 18);
  row.core = { ...row.core, bytes: bytes.length, sha256: digest(bytes) };
  row.buildGraph.settings = { GOOS: 'linux', GOARCH: 'amd64', CGO_ENABLED: '0', '-trimpath': 'true', '-tags': row.tags };
  const raw = 'fixture-path: go1.27.2\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n\tmod\tgithub.com/sagernet/sing-box\t(devel)\n'
    + '\tdep\texample.invalid/fixture\tv1.0.0\th1:YQ=\n'
    + row.buildGraph.localReplacements.map(m => `\tdep\t${m.module}\t${m.version}\n\t=>\t${m.replacement.module}\t(devel)\n`).join('')
    + Object.entries(row.buildGraph.settings).map(([k, v]) => `\tbuild\t${k}=${v}\n`).join('');
  row.rawBuildInfo = raw;
  const root = mkdtempSync(join(tmpdir(), 'reviewed-buildinfo-')), path = join(root, 'sing-box'); writeFileSync(path, bytes);
  const run = text => (command, args) => args[0] === 'version' ? text : '';
  try {
    verifyReleaseCore(path, row, 'linux', run(raw), 'linux', undefined, contract);
    assert.throws(() => verifyReleaseCore(path, row, 'linux', run(raw.replace('go1.27.2', 'go1.25.5')), 'linux', undefined, contract), /Go version/);
    assert.throws(() => verifyReleaseCore(path, row, 'linux', run(raw.replace(': go1.27.2', ': prefixgo1.27.2')), 'linux', undefined, contract), /Go version/);
    assert.throws(() => verifyReleaseCore(path, row, 'linux', run(raw.replace('./third_party/metacubex-utls', './outside')), 'linux', undefined, contract), /BuildInfo|graph/);
    const mismatched = structuredClone(contract); mismatched.sourceTree = '0'.repeat(40);
    assert.throws(() => verifyReleaseCore(path, row, 'linux', run(raw), 'linux', undefined, mismatched), /source pair/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
test('reviewed v4 eleven inert BuildInfo matches all linked vendors on all four desktop targets', () => {
  const contract = reviewedFixture(), root = mkdtempSync(join(tmpdir(), 'reviewed-vendor-buildinfo-'));
  try {
    for (const [key, row] of Object.entries(contract.platforms)) {
      const target = RELEASE_TARGETS[key], bytes = Buffer.alloc(256), path = join(root, key);
      if (key === 'mac-arm64') row.signatureObservation = { state: 'signed', cdHash: '4'.repeat(40), trustValidated: false };
      if (key === 'linux') { bytes.set([127,69,76,70,2,1]); bytes.writeUInt16LE(62,18); }
      else if (key === 'win') { bytes.write('MZ'); bytes.writeUInt32LE(128,60); bytes.write('PE\0\0',128); bytes.writeUInt16LE(0x8664,132); }
      else { bytes.writeUInt32LE(0xfeedfacf); bytes.writeUInt32LE(key === 'mac-x64' ? 0x1000007 : 0x100000c,4); }
      row.core = { ...row.core, bytes: bytes.length, sha256: digest(bytes) };
      writeFileSync(path, bytes);
      row.buildGraph.settings = { GOOS: target.os, GOARCH: target.arch, CGO_ENABLED: target.cgo, '-trimpath': 'true', '-tags': row.tags };
      const raw = `fixture: ${row.goVersion}\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n\tmod\tgithub.com/sagernet/sing-box\t(devel)\n`
        + row.buildGraph.normalModules.map(m => `\tdep\t${m.module}\t${m.version}\t${m.h1}\n`).join('')
        + row.buildGraph.localReplacements.map(m => `\tdep\t${m.module}\t${m.version}\n\t=>\t${m.replacement.module}\t(devel)\n`).join('')
        + Object.entries(row.buildGraph.settings).map(([k,v]) => `\tbuild\t${k}=${v}\n`).join('');
      row.rawBuildInfo = raw;
      const run = text => (command, args) => { assert.equal(command, 'go');
        assert.ok(args[0] === 'version' || args[0] === 'tool' && args[1] === 'buildid');
        return args[0] === 'version' ? text : row.buildID; };
      assert.deepEqual(verifyReleaseCore(path, row, key, run(raw), 'linux', undefined, contract), row.buildGraph);
      for (const local of row.buildGraph.localReplacements) {
        const line = `\tdep\t${local.module}\t${local.version}\n\t=>\t${local.replacement.module}\t(devel)\n`;
        for (const changed of [raw.replace(line, ''), raw.replace(line, line.replace('(devel)', 'v9.9.9')),
          raw.replace(line, line.replace(local.replacement.module, './outside'))])
          assert.throws(() => verifyReleaseCore(path, row, key, run(changed), 'linux', undefined, contract), /BuildInfo|graph/);
      }
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('explicit public release names keep all four original archive layouts and byte pins', () => {
  const { contract, index } = fixture();
  validateReleaseContract(contract);
  for (const row of Object.values(contract.platforms)) {
    assert.equal(row.releaseAsset.name, row.archive.releaseName);
    assert.equal(row.releaseAsset.sha256, row.archive.sha256);
    assert.equal(row.releaseAsset.bytes, row.archive.bytes);
    assert.equal(row.releaseAsset.name.includes('1.15.0-polaris-'), false);
    assert.equal(row.version, '1.15.0-polaris-alpha.12');
    assert.equal(contract.tag, 'v1.15.0-polaris-alpha.12');
    const original = index.platforms.find(r => r.id === row.id);
    assert.deepEqual(JSON.parse(upstreamMetadata(contract, row)['provenance.json']).receipt, original);
    assert.ok(contractPublicAssets(contract).includes(row.releaseAsset));
  }
});
test('public release names reject explicit invalid values instead of falling back', () => {
  const name = fixture().contract.platforms.linux.archive.releaseName;
  const invalid = [null, undefined, '', 1, false, {}, [], '../' + name, '/' + name,
    'https://example.invalid/' + name, name.replace('linux-amd64', 'windows-amd64'),
    name.replace('1.15.0', '1.16.0'), name.replace('1.15.0-', '1.15.0-polaris-'), name.replace('sing-box-polaris-', 'sing-box-'),
    name.replace('sing-box-polaris-', 'sing-box-polaris-polaris-'), name + '?download=1',
    name.replaceAll('-', '\\'), name.toUpperCase(), name + '\n'];
  for (const value of invalid) {
    const { contract } = fixture();
    contract.platforms.linux.archive.releaseName = value;
    assert.throws(() => validateReleaseContract(contract), /release archive name/);
  }
  const { contract } = fixture();
  contract.platforms.linux.archive.publicName = name;
  assert.throws(() => validateReleaseContract(contract), /public-name alias/);
});
test('branded public names cannot relax asset identity, bytes or original layout', () => {
  const changes = [
    c => { c.platforms.linux.releaseAsset.name = c.platforms.linux.archive.name; },
    c => { c.platforms.linux.releaseAsset.url = c.platforms.linux.releaseAsset.url.replace('sing-box-polaris-1.', 'sing-box-1.'); },
    c => { c.platforms.linux.releaseAsset.sha256 = 'b'.repeat(64); },
    c => { c.platforms.linux.releaseAsset.bytes++; },
    c => { c.platforms.linux.releaseAsset.releaseId++; },
    c => { c.platforms.linux.releaseAsset.assetId = c.platforms.win.releaseAsset.assetId; },
    c => { c.platforms.linux.archive.name = c.platforms.linux.archive.releaseName; },
    c => { c.platforms.linux.archive.root = c.platforms.linux.archive.root.replace('sing-box-', 'sing-box-polaris-'); },
    c => { c.platforms.linux.id = c.platforms.linux.id.replace('sing-box-', 'sing-box-polaris-'); },
  ];
  for (const change of changes) {
    const { contract } = fixture(); change(contract);
    assert.throws(() => validateReleaseContract(contract));
  }
});

test('native upstream payload uses its validated top-level Go version without legacy-field fallback', () => {
  const { contract } = fixture(), row = contract.platforms.linux;
  validateReleaseContract(contract);
  assert.equal(Object.hasOwn(row, 'sourceFingerprint'), false);
  assert.equal(nativeReleaseGoVersion(contract, row), row.goVersion);
  const missing = structuredClone(row); delete missing.goVersion;
  missing.sourceFingerprint = { goVersion: row.goVersion };
  assert.throws(() => nativeReleaseGoVersion(contract, missing), /Go version/);
  assert.throws(() => nativeReleaseGoVersion(contract, { ...row, goVersion: 'go1.25.5' }), /Go version/);
  assert.throws(() => nativeReleaseGoVersion(contract, { ...row, schema: 'polaris-production-native-receipt-v1' }), /schema/);
});

// Integration uses inert binary headers and real archive extraction/hash checks.
// Go inspection is mocked. No native execution occurs.
test('published consume and packaged verification work with every Actions URL denied', async t => {
  const { contract } = fixture(), root = mkdtempSync(join(tmpdir(), 'published-no-actions-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const transport = new Map(), calls = [];
  const encode = value => Buffer.from(JSON.stringify(value));
  const put = (path, bytes) => { mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, bytes); };
  const payloads = new Map();
  for (const [key, row] of Object.entries(contract.platforms)) {
    const target = RELEASE_TARGETS[key], core = Buffer.alloc(256);
    if (key === 'mac-arm64') row.signatureObservation = { state: 'signed', cdHash: '4'.repeat(40), trustValidated: false };
    if (key === 'linux') { core.set([127,69,76,70,2,1]); core.writeUInt16LE(62,18); }
    else if (key === 'win') { core.write('MZ'); core.writeUInt32LE(128,60); core.write('PE\0\0',128); core.writeUInt16LE(0x8664,132); }
    else { core.writeUInt32LE(0xfeedfacf); core.writeUInt32LE(key === 'mac-x64' ? 0x1000007 : 0x100000c,4); }
    core.write(key,200);
    const files = { [target.core]: core, LICENSE: Buffer.from('LICENSE') };
    if (target.sidecar) files[target.sidecar] = cronetFixture(key);
    for (const value of [row.core, ...(row.sidecar ? [row.sidecar] : [])])
      Object.assign(value, { bytes: files[value.member].length, sha256: digest(files[value.member]) });
    Object.assign(row.finalPackageCronetSmoke, { coreSha256: row.core.sha256, librarySha256: row.sidecar?.sha256 ?? null });
    row.buildGraph.settings = { GOOS: target.os, GOARCH: target.arch, CGO_ENABLED: target.cgo, '-trimpath': 'true', '-tags': row.tags };
    row.rawBuildInfo = `fixture: ${row.goVersion}\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n\tmod\tgithub.com/sagernet/sing-box\t(devel)\n`
      + row.buildGraph.normalModules.map(m => `\tdep\t${m.module}\t${m.version}\t${m.h1}\n`).join('')
      + row.buildGraph.localReplacements.map(m => `\tdep\t${m.module}\t${m.version}\t${m.h1}\n\t=>\t${m.replacement.module}\t${m.replacement.version}\t${m.replacement.h1}\n`).join('')
      + Object.entries(row.buildGraph.settings).map(([k,v]) => `\tbuild\t${k}=${v}\n`).join('');
    const members = Object.fromEntries(Object.entries(files).map(([name,b]) => [`${row.archive.root}/${name}`, b.toString('base64')]));
    const archive = join(root, row.archive.name);
    execFileSync(process.platform === 'win32' ? 'python' : 'python3', ['-c', `import base64,io,json,sys,tarfile,zipfile
members=json.load(sys.stdin)
if sys.argv[1].endswith('.zip'):
 with zipfile.ZipFile(sys.argv[1],'w') as z:
  for n,v in members.items():
   i=zipfile.ZipInfo(n);i.external_attr=0o100755<<16;z.writestr(i,base64.b64decode(v))
else:
 with tarfile.open(sys.argv[1],'w:gz') as z:
  for n,v in members.items():
   b=base64.b64decode(v);i=tarfile.TarInfo(n);i.size=len(b);i.mode=0o755;z.addfile(i,io.BytesIO(b))`, archive], { input: JSON.stringify(members) });
    const bytes = readFileSync(archive);
    Object.assign(row.archive, { bytes: bytes.length, sha256: digest(bytes), members: Object.fromEntries(Object.entries(files).map(([name,b]) => [`${row.archive.root}/${name}`,digest(b)])) });
    Object.assign(row.releaseAsset, { bytes: bytes.length, sha256: digest(bytes) });
    transport.set(row.releaseAsset.url, bytes); payloads.set(row.core.sha256,row);
  }
  const raw = encode(contract), manifest = { desktopRelease: { schema: 'polaris-app-release-selection-v1', contractPath:'scripts/desktop-core/release-contract.json', contractSha256:digest(raw) } };
  put(join(root,manifest.desktopRelease.contractPath),raw);
  const api = `https://api.github.com/repos/${contract.repository}`;
  transport.set(api,encode({id:contract.repositoryId,full_name:contract.repository,private:false}));
  transport.set(api+`/releases/${contract.releaseId}`,encode({id:contract.releaseId,tag_name:contract.tag,draft:false,immutable:false,
    html_url:`https://github.com/${contract.repository}/releases/tag/${contract.tag}`,
    assets:contractPublicAssets(contract).map(a=>({id:a.assetId,name:a.name,size:a.bytes,state:'uploaded',browser_download_url:a.url,digest:`sha256:${a.sha256}`}))}));
  transport.set(api+`/git/ref/tags/${contract.tag}`,encode({ref:`refs/tags/${contract.tag}`,object:{type:'tag',sha:contract.tagObject}}));
  transport.set(api+`/git/tags/${contract.tagObject}`,encode({sha:contract.tagObject,tag:contract.tag,object:{type:'commit',sha:contract.sourceCommit}}));
  transport.set(api+`/git/commits/${contract.sourceCommit}`,encode({sha:contract.sourceCommit,tree:{sha:contract.sourceTree}}));
  const read = async url => { calls.push(url); assert(!url.includes('/actions/'),'Actions forbidden');assert(transport.has(url),url);return transport.get(url); };
  const run = (command,args,options) => {
    if(command==='go'){const row=payloads.get(digest(readFileSync(args.at(-1))));assert(row);return args[0]==='version'?row.rawBuildInfo:row.buildID;}
    return execFileSync(command,args,options);
  };
  const receipts=await consumeRelease(root,manifest,undefined,{read,run,host:'linux'});
  for(const [key,row] of Object.entries(contract.platforms)){
    const receipt=verifyPackagedRelease(root,manifest,key,join(root,'resources',key,row.core.member),run,'linux');
    assert.equal(Object.hasOwn(receipt.publicReadback,'producerReadback'),false);
  }
  assert.equal(Object.keys(receipts).length,4);
  assert.equal(calls.filter(url=>url.includes('/actions/')).length,0);
  const publicURL = api+`/releases/${contract.releaseId}`, publicBytes = transport.get(publicURL);
  for (const mutate of [r=>{r.id++;},r=>{r.draft=true;},r=>{r.tag_name+='changed';},
    r=>{r.assets[0].id++;},r=>{r.assets[0].digest='sha256:'+'0'.repeat(64);},
    r=>{r.assets[0].size++;},r=>{r.assets[0].browser_download_url+='?changed';}]) {
    const changed=JSON.parse(publicBytes);mutate(changed);transport.set(publicURL,encode(changed));
    await assert.rejects(consumeRelease(root,manifest,['linux'],{read,run}));
    transport.set(publicURL,publicBytes);
  }
  const assetURL=contract.platforms.linux.releaseAsset.url, original=transport.get(assetURL);
  transport.set(assetURL,Buffer.alloc(original.length));
  await assert.rejects(consumeRelease(root,manifest,['linux'],{read,run}));transport.set(assetURL,original);
  let reads=0;
  await assert.rejects(consumeRelease(root,manifest,['linux'],{run,read:async url=>{
    if(url===publicURL && ++reads===2){const r=JSON.parse(publicBytes);r.assets[0].digest=null;return encode(r);}
    return read(url);
  }}),/identity changed/);
  // Failed acquisitions never replace the already installed, verified payload.
  verifyPackagedRelease(root,manifest,'linux',join(root,'resources/linux/sing-box'),run,'linux');
  // Reuse this exact original archive fixture. Only Go inspection remains
  // mocked; the cache does not certify it.
  const cacheParent=mkdtempSync(join(tmpdir(),'published-blob-cache-')), blobCacheDir=join(fs.realpathSync(cacheParent),'store');
  t.after(()=>rmSync(cacheParent,{recursive:true,force:true}));
  const reports=[], options={read,run,host:'linux',blobCacheDir,report:line=>reports.push(line)};
  const admissionStop=Error('private upstream cache admission observation only');
  await assert.rejects(consumeRelease(root,manifest,['linux'],{...options,read:async()=>{throw admissionStop;}}),error=>error===admissionStop);
  const cacheSupported=reports.includes('Release blob cache ready');
  assert.equal(reports.includes('Release blob cache unavailable'),!cacheSupported);reports.length=0;

  const payloadURLs=Object.values(contract.platforms).map(row=>row.releaseAsset.url);
  const payloadCount=()=>calls.filter(url=>payloadURLs.includes(url)).length;
  const bytesAt=()=>Object.fromEntries(Object.entries(contract.platforms).map(([key,row])=>[key,digest(readFileSync(join(root,'resources',key,row.core.member)))]));
  await t.test('raw cache upstream cold/warm still reproves both public snapshots',async()=>{
    const n=payloadCount();
    const cold=await consumeRelease(root,manifest,undefined,options), coldBytes=bytesAt();
    assert.equal(payloadCount()-n,4);
    const at=calls.length, first=payloadCount();
    const warm=await consumeRelease(root,manifest,undefined,options);
    assert.deepEqual(warm,cold);assert.deepEqual(bytesAt(),coldBytes);
    assert.equal(payloadCount()-first,cacheSupported?0:4);
    assert.equal(calls.slice(at).filter(url=>url===publicURL).length,2);
    assert.equal(reports.filter(line=>line.endsWith(' hit')).length,cacheSupported?4:0);
    for(const key of Object.keys(contract.platforms))verifyPackagedRelease(root,manifest,key,join(root,'resources',key,contract.platforms[key].core.member),run,'linux');
  });
});


const versionCases = JSON.parse(readFileSync(new URL('../lib/polaris-version-fixtures.json', import.meta.url), 'utf8'));
test('shared version corpus retains patch tags and exact original/public archive mapping', () => {
  for (const version of versionCases.accepted) {
    const { contract, index } = fixture(version);
    assert.equal(validateReleaseContract(contract), contract, version);
    for (const row of Object.values(contract.platforms)) {
      assert.equal(row.version, version);
      assert.equal(row.tag, `v${version}`);
      assert.ok(row.archive.root.startsWith(`sing-box-${version}-`));
      assert.ok(row.releaseAsset.name.startsWith(`sing-box-polaris-${version.replace('-polaris-', '-')}-`));
      assert.ok(row.releaseAsset.url.includes(`/v${version}/`));
      assert.deepEqual(JSON.parse(upstreamMetadata(contract, row)['provenance.json']).receipt,
        index.platforms.find(original => original.id === row.id));
    }
  }
  for (const version of versionCases.rejected) {
    assert.throws(() => validateReleaseContract(fixture(version).contract), undefined, JSON.stringify(version));
  }
});
test('patch syntax never substitutes for complete release identity or matching raw versions', () => {
  const version = '1.15.0-polaris-alpha.12.1';
  for (const mutate of [
    c => { c.tag = 'v1.15.0-polaris-alpha.12'; },
    c => { c.repository = 'SagerNet/sing-box'; },
    c => { c.repositoryId = 1407705262; },
    c => { c.platforms.linux.version = '1.15.0-polaris-alpha.12'; },
    c => { c.platforms.linux.sourceCommit = 'f'.repeat(40); },
    c => { c.platforms.linux.sourceTree = 'f'.repeat(40); },
    c => { c.platforms.linux.releaseAsset.releaseId++; },
    c => { c.platforms.linux.releaseAsset.sha256 = 'f'.repeat(64); },
    c => { c.platforms.linux.releaseAsset.url = c.platforms.linux.releaseAsset.url.replace('.12.1/', '.12/'); },
    c => { c.platforms.linux.archive.releaseName = c.platforms.linux.archive.releaseName.replace('.12.1-', '.12-'); },
  ]) {
    const { contract } = fixture(version); mutate(contract);
    assert.throws(() => validateReleaseContract(contract));
  }
});
