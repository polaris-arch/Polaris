// Deliberately synthetic contract fixtures. Never publication or smoke evidence.
import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { dirname } from 'node:path';
import { cronetFixture } from '../lib/cronet-payload.test.mjs';
import { verifyNativeReleaseGate } from './release-native-gates.mjs';
import fs, { mkdtempSync, writeFileSync, rmSync, mkdirSync, readFileSync, copyFileSync } from 'node:fs';
import { tmpdir, devNull } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { nativeReleaseGoVersion } from '../native-payload-input.mjs';
import { digest, desktopSourceGoVersion, upstreamSourceGoVersion, reviewedSourcePolicy } from './source-graph.mjs';
import { ADAPTER_FILES, consumeRelease, verifyPackagedRelease, portableCanonical, RELEASE_TARGETS, validateReleaseContract, releasePayloadMembers, publicBytes, verifyReleaseCore, releaseSingTunDependency, validateReleaseNotice, releaseInstallationContract, UPSTREAM_INSTALLATION_DOMAIN } from './release-consumer.mjs';
import { selectedCoreManifest } from '../lib/core-identity.mjs';
import { managedFormalSelection } from './managed-mesh-core.mjs';
import { validateUpstreamIndex, upstreamMetadata, upstreamPayloadPins, contractPublicAssets, readUpstreamPublicEvidence, validateUpstreamReadback, readUpstreamSourceEvidence, validatePublishedUpstreamReadback } from './upstream-release.mjs';
// Frozen G4 source coordinates, with synthetic payloads only. These tests do
// not assert that a release exists or that any native artifact has passed.
const hash = 'a'.repeat(64), commit = '55300c55560bcc604fd9675c5526b5713db81cc1',
  tree = '7767ba9a9901a061ff07db7716baed7896cfefbc';
function fixture(version = '1.15.0-polaris-alpha.12') {
  let id = 1;
  const contract = { schema: 'polaris-upstream-native-release-consumer-v1', providerContract: 'polaris-upstream-native-release-v1',
    repository: 'polaris-arch/sing-box-polaris', repositoryId: 1421657509, releaseId: 100,
    version, tag: `v${version}`, tagObject: 'd'.repeat(40), sourceCommit: commit, sourceTree: tree,
    producer: { repository: 'polaris-arch/sing-box-polaris', repositoryId: 1421657509, workflow: '.github/workflows/build.yml', workflowCommit: commit, runId: 1, runAttempt: 1, nativeJobs: Object.fromEntries(Object.entries({ linux: 'linux-amd64-purego', win: 'windows-amd64', 'mac-x64': 'darwin-amd64', 'mac-arm64': 'darwin-arm64' }).map(([key, id], i) => [key, { id: i + 1, name: `Native final-package Cronet and gVisor smoke (${id})` }])) },
    legalFiles: ['LICENSE', 'NOTICE'].map((name) => ({ path: `licenses/${name}`, content: name, bytes: Buffer.byteLength(name), sha256: digest(name) })), platforms: {} };
  const asset = (name) => ({ name, bytes: 1, sha256: hash, assetId: id++, releaseId: 100,
    url: `https://github.com/${contract.repository}/releases/download/${contract.tag}/${name}` });
  contract.assetIndex = asset('polaris-upstream-native-assets.json');
  contract.sourceArchive = asset(`sing-box-polaris-source-${commit}.tar.gz`);
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
  const { schema, providerContract, releaseId, assetIndex, platforms, sourceArchive, ...base } = contract;
  const index = { schema: 'polaris-upstream-native-assets-v1', ...base,
    sourceArchive: { name: sourceArchive.name, bytes: sourceArchive.bytes, sha256: sourceArchive.sha256 },
    platforms: Object.values(platforms).map(({ releaseAsset, ...row }) => row) };
  return { contract, index };
}

function installationFixture(t, contract = fixture().contract) {
  const root = mkdtempSync(join(tmpdir(), 'upstream-installation-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'scripts/desktop-core'), { recursive: true });
  const raw = Buffer.from(JSON.stringify(contract) + '\n');
  writeFileSync(join(root, 'scripts/desktop-core/release-contract.json'), raw);
  const manifest = { desktopRelease: { schema: 'polaris-app-release-selection-v1',
    contractPath: 'scripts/desktop-core/release-contract.json', contractSha256: digest(raw),
    ...Object.fromEntries(['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId'].map(key => [key, contract[key]])),
    librarySha256: { linux: contract.platforms.linux.sidecar.sha256, win: contract.platforms.win.sidecar.sha256 } } };
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
test('formal installation refuses mixed selection, copied provider labels and absent real contract pins', t => {
  const f = installationFixture(t);
  assert.throws(() => releaseInstallationContract(f.root, { ...f.manifest, desktopTemporary: {} }), /cannot coexist/);
  const pending = structuredClone(f.manifest);
  pending.desktopRelease.contractPath = 'scripts/desktop-core/pending-release-contract.json';
  writeFileSync(join(f.root, pending.desktopRelease.contractPath), f.raw);
  assert.throws(() => releaseInstallationContract(f.root, pending), /compiled canonical contract/);
  for (const name of ['providerFingerprint', 'providerIdentity']) {
    const bad = structuredClone(f.contract); bad.platforms.linux[name] = 'a'.repeat(64);
    const copy = installationFixture(t, bad);
    assert.throws(() => releaseInstallationContract(copy.root, copy.manifest), /unverified producer/);
  }
  for (const mutate of [c => { c.assetIndex.sha256 = null; }, c => { c.producer.runId = null; },
    c => { c.platforms.win.finalPackageCronetSmoke.coreSha256 = '0'.repeat(64); }]) {
    const bad = structuredClone(f.contract); mutate(bad); const copy = installationFixture(t, bad);
    assert.throws(() => releaseInstallationContract(copy.root, copy.manifest));
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
    assert.equal(chosen.key, key); assert.equal(chosen.upstream, true);
    assert.ok(chosen.expected.includes(`Revision: ${contract.sourceCommit}\n`));
    assert.ok(chosen.expected.includes(`Environment: ${contract.platforms[key].goVersion} ${contract.platforms[key].target}\n`));
  }
  assert.throws(() => managedFormalSelection(f.root, f.manifest, { os: 'linux', arch: 'arm64' }), /native desktop/);
  const raw = readFileSync(new URL('./managed-mesh-core.mjs', import.meta.url), 'utf8');
  assert.ok(raw.indexOf('verifyPackagedRelease(root, manifest, key, core, run, host.os)') < raw.indexOf("const output = run(core, ['version'], { encoding: 'utf8' })"));
  assert.match(raw, /demand\(output === expected, 'Managed mesh core version output differs'\)/);
});
test('upstream native contract keeps actual empty BuildID and unchanged native archive', () => {
  const { contract, index } = fixture();
  assert.equal(validateReleaseContract(contract), contract);
  assert.equal(validateUpstreamIndex(index, contract), index);
  assert.equal(contractPublicAssets(contract).length, 6);
  assert.ok(releasePayloadMembers(contract, 'linux').includes(`core-release/source/${contract.sourceArchive.name}`));
  assert.equal(upstreamPayloadPins(contract, contract.platforms.linux)['licenses/LICENSE'], digest('LICENSE'));
});
test('complete upstream index can contain unrelated architecture and libc variants', () => {
  const { contract, index } = fixture();
  index.platforms.push({ ...structuredClone(index.platforms[0]), id: index.platforms[0].id + '-musl', finalPackageCronetSmoke: null });
  validateUpstreamIndex(index, contract);
  index.platforms.push(structuredClone(index.platforms[0]));
  assert.throws(() => validateUpstreamIndex(index, contract), /identities/);
});
test('new provenance, smoke, repository, archive and recipe cannot be fabricated by relabeling', () => {
  const changes = [
    (c) => { c.repositoryId = 1407705262; },
    (c) => { c.platforms.linux.finalPackageCronetSmoke = null; },
    (c) => { c.platforms.linux.finalPackageCronetSmoke.coreSha256 = '0'.repeat(64); },
    (c) => { c.platforms.linux.variant.libc = 'musl'; },
    (c) => { c.platforms.linux.archive.root = 'polaris-box-legacy'; },
    (c) => { c.platforms.linux.tags = 'with_gvisor'; },
    (c) => { c.sourceArchive.url = c.sourceArchive.url.replace('sing-box-polaris', 'polaris-box'); },
    (c) => { c.producer.workflowCommit = '0'.repeat(40); },
    (c) => { c.legalFiles[0].content += 'changed'; },
  ];
  for (const change of changes) { const { contract } = fixture(); change(contract); assert.throws(() => validateReleaseContract(contract)); }
});
function evidenceFixture(contract) {
  const responses = new Map();
  responses.set('/actions/runs/1/attempts/1', { id: 1, run_attempt: 1, status: 'completed', conclusion: 'success', head_sha: commit,
    path: contract.producer.workflow, workflow_id: 10, event: 'workflow_dispatch',
    repository: { id: 1421657509, full_name: contract.repository }, head_repository: { id: 1421657509, full_name: contract.repository } });
  responses.set('/actions/workflows/10', { id: 10, path: contract.producer.workflow });
  for (const job of Object.values(contract.producer.nativeJobs)) responses.set(`/actions/jobs/${job.id}`,
    { id: job.id, name: job.name, run_id: 1, run_attempt: 1, head_sha: commit, status: 'completed', conclusion: 'success' });
  responses.set(`/git/trees/${tree}?recursive=1`, { sha: tree, truncated: false, tree: [{ mode: '160000', type: 'commit', path: 'clients/android', sha: 'e'.repeat(40) }] });
  return responses;
}
test('upstream producer identity is checked against public run, workflow, jobs and source tree', async () => {
  const { contract } = fixture(), responses = evidenceFixture(contract);
  const snapshot = await readUpstreamPublicEvidence(contract, async (path) => structuredClone(responses.get(path)));
  validateUpstreamReadback(snapshot, contract);
  assert.deepEqual(snapshot.sourceGitlinks, [{ path: 'clients/android', sha: 'e'.repeat(40) }]);
  const corruptions = [
    (r) => { r.get('/actions/runs/1/attempts/1').head_repository.id = 1407705262; },
    (r) => { r.get('/actions/runs/1/attempts/1').head_sha = '0'.repeat(40); },
    (r) => { r.get('/actions/runs/1/attempts/1').conclusion = null; },
    (r) => { r.get('/actions/workflows/10').path = '.github/workflows/other.yml'; },
    (r) => { r.get('/actions/jobs/1').run_attempt = 2; },
    (r) => { r.get('/actions/jobs/1').run_id = 2; },
    (r) => { r.get('/actions/jobs/1').name += ' unexpected'; },
    (r) => { r.get(`/git/trees/${tree}?recursive=1`).truncated = true; },
    (r) => { r.get(`/git/trees/${tree}?recursive=1`).tree[0].type = 'blob'; },
  ];
  for (const corrupt of corruptions) {
    const changed = evidenceFixture(contract); corrupt(changed);
    await assert.rejects(readUpstreamPublicEvidence(contract, async (path) => structuredClone(changed.get(path))));
  }
});
test('recursive Git tree credential scope allows only exact selected-repository query', async () => {
  const seen = [], fetch = async (url, options) => { seen.push(options.headers.Authorization); return new Response('ok'); };
  const options = { repository: 'polaris-arch/sing-box-polaris', fetch, env: { GITHUB_ACTIONS: 'true', POLARIS_RELEASE_API_TOKEN: 'fixture' } };
  for (const url of [
    `https://api.github.com/repos/polaris-arch/sing-box-polaris/git/trees/${tree}?recursive=1`,
    `https://api.github.com/repos/polaris-arch/polaris-box/git/trees/${tree}?recursive=1`,
    `https://api.github.com/repos/polaris-arch/sing-box-polaris/git/trees/${tree}?recursive=1&x=1`,
    'https://api.github.com/repos/polaris-arch/sing-box-polaris?recursive=1',
  ]) await publicBytes(url, 10, false, options);
  assert.deepEqual(seen, ['Bearer fixture', undefined, undefined, undefined]);
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
test('actual desktop toolchain-selection entry handles both release contract generations', () => {
  const { contract } = fixture(), root = mkdtempSync(join(tmpdir(), 'upstream-go-selection-'));
  const bytes = JSON.stringify(contract) + '\n';
  const selection = { schema: 'polaris-app-release-selection-v1', contractPath: 'scripts/desktop-core/release-contract.json',
    contractSha256: digest(bytes), ...Object.fromEntries(['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId'].map((key) => [key, contract[key]])),
    librarySha256: { linux: contract.platforms.linux.sidecar.sha256, win: contract.platforms.win.sidecar.sha256 } };
  try {
    mkdirSync(join(root, 'scripts/desktop-core'), { recursive: true });
    writeFileSync(join(root, selection.contractPath), bytes);
    assert.equal(desktopSourceGoVersion(root, { desktopRelease: selection }), '1.26.8');
    const actualRoot = fileURLToPath(new URL('../../', import.meta.url));
    const selectedManifest = JSON.parse(readFileSync(join(actualRoot, 'src-tauri/core-manifest.json')));
    const selectedContract = JSON.parse(readFileSync(join(actualRoot, (selectedManifest.desktopRelease ?? selectedManifest.desktopTemporary).contractPath)));
    assert.equal(desktopSourceGoVersion(actualRoot, selectedManifest),
      selectedManifest.desktopTemporary ? '1.27.2' : selectedContract.schema === 'polaris-upstream-native-release-consumer-v1' ? upstreamSourceGoVersion(selectedContract).slice(2) : '1.25.5');
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
  assert.deepEqual(releaseSingTunDependency({ schema: 'polaris-production-native-receipt-v1', buildGraph: { localReplacement: expected } }), expected);
  const rust = readFileSync(new URL('../../crates/config-engine/tests/core_dep_fingerprint.rs', import.meta.url), 'utf8');
  assert.ok(rust.includes('JSON.stringify(releaseSingTunDependency(c.platforms[key]))'));
  assert.ok(!rust.includes('JSON.stringify(c.platforms[key].buildGraph.localReplacement)'));
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
    assert.equal(contract.platforms[key].releaseAsset.name, contract.platforms[key].archive.root + target.suffix);
    assert.equal(contract.platforms[key].core.member, target.os === 'windows' ? 'sing-box.exe' : 'sing-box');
  }
  for (const version of ['1.15.0-alpha.12', '1.15.0-alpha.12.polaris.1', '1.15.0-polaris-alpha.012', '1.15.0-extended-alpha.12']) {
    const changed = structuredClone(contract); changed.version = version; changed.tag = `v${version}`;
    assert.throws(() => validateReleaseContract(changed), /identity/);
  }
});
test('branded desktop NOTICE uses actual release version while retaining the upstream baseline', () => {
  const { contract } = fixture();
  const manifest = { bundledCoreVersion: '1.15.0-alpha.8', desktopRelease: { contractPath: 'scripts/desktop-core/release-contract.json', contractSha256: hash } };
  const mobile = { android: { sourceCommit: 'e'.repeat(40), manifestSha256: hash }, apple: { sourceCommit: 'f'.repeat(40), manifestSha256: hash } };
  const base = `https://github.com/${contract.repository}`;
  const first = `  - 桌面 sing-box v${contract.version} (GPLv3) — ${base}`;
  const lines = [first, '上游基线源码：https://github.com/SagerNet/sing-box/tree/v1.15.0-alpha.12',
    `桌面发布构建：${contract.version}`, `固定源码：${base}/tree/${commit}`, `源码 tree：${tree}`,
    `固定消费标签：${base}/tree/${contract.tag}`, `消费标签对象：${contract.tagObject}`,
    `消费合同 SHA-256：${hash}`, `公开资产 index SHA-256：${contract.assetIndex.sha256}`, `桌面发布：${base}/releases/tag/${contract.tag}`];
  for (const [key, source] of Object.entries(mobile)) {
    const label = key === 'android' ? 'Android libbox 基线' : 'Apple libbox 基线';
    lines.push(`  - ${label} v1.15.0-alpha.8 (GPLv3) — https://github.com/SagerNet/sing-box`,
      '上游基线源码：https://github.com/SagerNet/sing-box/tree/v1.15.0-alpha.8',
      `固定源码：https://github.com/SagerNet/sing-box/tree/${source.sourceCommit}`, `source manifest SHA-256：${source.manifestSha256}`);
  }
  lines.push('`src-tauri/core-manifest.json` 的 `desktopRelease`', '`sourceArchive` 指向同批完整源码归档。',
    `Desktop release: ${contract.version} — ${base}/releases/tag/${contract.tag}`, `Desktop source: ${base}/tree/${commit}`,
    `Desktop release contract: scripts/desktop-core/release-contract.json (SHA-256 ${hash})`, `Published asset index SHA-256: ${contract.assetIndex.sha256}`,
    'Desktop selection uses src-tauri/core-manifest.json desktopRelease.', 'asset index sourceArchive identifies the same-run complete source archive,');
  const notice = lines.join('\n');
  validateReleaseNotice(notice, manifest, contract, mobile);
  assert.throws(() => validateReleaseNotice(notice.replace(first, first.replace(contract.version, '1.15.0-alpha.12')), manifest, contract, mobile), /baseline/);
});

// These remain deliberately synthetic bytes. The real b024 source coordinates
// exercise admission policy, never claim publication or successful native output.
function utlsFixture() {
  const { contract } = fixture();
  contract.sourceCommit = 'b0247cc930bc892a0ea03138906b548b6e84ac50';
  contract.sourceTree = 'b4b670f51d1c48b3c0be9ab882152e3f25a5d9b4';
  contract.producer.workflowCommit = contract.sourceCommit;
  contract.sourceArchive.name = `sing-box-polaris-source-${contract.sourceCommit}.tar.gz`;
  contract.sourceArchive.url = `https://github.com/${contract.repository}/releases/download/${contract.tag}/${contract.sourceArchive.name}`;
  for (const row of Object.values(contract.platforms)) {
    row.sourceCommit = contract.sourceCommit; row.sourceTree = contract.sourceTree;
    row.goVersion = 'go1.27.2'; row.rawBuildInfo = 'fixture: go1.27.2\n';
    row.finalPackageCronetSmoke.go = 'go1.27.2';
  }
  return contract;
}
test('only the exact frozen new source admits Go 1.27.2; legacy G4 and v1 keep their original toolchains', () => {
  const contract = utlsFixture();
  assert.equal(validateReleaseContract(contract), contract);
  for (const version of ['go1.26.8', 'go1.27.1', 'go1.28.0', 'arbitrary']) {
    const changed = structuredClone(contract);
    changed.platforms.linux.goVersion = version;
    changed.platforms.linux.finalPackageCronetSmoke.go = version;
    assert.throws(() => validateReleaseContract(changed), /toolchain/);
  }
  for (const change of [
    c => { c.sourceTree = '0'.repeat(40); for (const row of Object.values(c.platforms)) row.sourceTree = c.sourceTree; },
    c => { c.sourceCommit = '55300c55560bcc604fd9675c5526b5713db81cc1'; c.producer.workflowCommit = c.sourceCommit; for (const row of Object.values(c.platforms)) row.sourceCommit = c.sourceCommit; },
    c => { c.sourceCommit = '1514e973812c3d3d9c6366cc85ebea98f3a1ce84'; c.sourceTree = '6e4f1d3e24399e28f7fcb0bb96c73043d455c19f'; c.producer.workflowCommit = c.sourceCommit; for (const row of Object.values(c.platforms)) { row.sourceCommit = c.sourceCommit; row.sourceTree = c.sourceTree; } },
  ]) {
    const changed = structuredClone(contract); change(changed);
    changed.sourceArchive.name = `sing-box-polaris-source-${changed.sourceCommit}.tar.gz`;
    changed.sourceArchive.url = `https://github.com/${changed.repository}/releases/download/${changed.tag}/${changed.sourceArchive.name}`;
    assert.throws(() => validateReleaseContract(changed), /toolchain/);
  }
  const root = mkdtempSync(join(tmpdir(), 'utls-go-selection-'));
  try {
    mkdirSync(join(root, 'scripts/desktop-core'), { recursive: true });
    const bytes = JSON.stringify(contract) + '\n';
    const contractPath = 'scripts/desktop-core/release-contract.json';
    writeFileSync(join(root, contractPath), bytes);
    const selection = { schema: 'polaris-app-release-selection-v1', contractPath, contractSha256: digest(bytes),
      ...Object.fromEntries(['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId'].map(k => [k, contract[k]])),
      librarySha256: { linux: hash, win: hash } };
    assert.equal(desktopSourceGoVersion(root, { desktopRelease: selection }), '1.27.2');
  } finally { rmSync(root, { recursive: true, force: true }); }
});

// Test-only facts. No real candidate/release/asset is selected by these values.
function reviewedFixture(v2 = false) {
  const contract = utlsFixture();
  contract.sourceCommit = '1'.repeat(40); contract.sourceTree = '2'.repeat(40);
  contract.producer.workflowCommit = contract.sourceCommit;
  contract.sourceArchive.name = `sing-box-polaris-source-${contract.sourceCommit}.tar.gz`;
  contract.sourceArchive.url = `https://github.com/${contract.repository}/releases/download/${contract.tag}/${contract.sourceArchive.name}`;
  contract.sourcePolicy = { schema: 'polaris-reviewed-kernel-source-v1',
    sourceCommit: contract.sourceCommit, sourceTree: contract.sourceTree,
    goDirective: '1.27.0', goVersion: '1.27.2', toolchain: 'go1.27.2',
    goModSha256: hash, goSumSha256: hash,
    metacubex: { module: 'github.com/metacubex/utls', version: 'v1.8.8',
      replacementPath: './third_party/metacubex-utls', replacementTree: '3'.repeat(40), replacementFileCount: 355,
      bindingPath: 'release/dependency-sources/metacubex-utls-replacement.json', bindingSha256: '4'.repeat(64),
      patchPath: 'release/dependency-sources/metacubex-utls-reality-alpn.patch', patchSha256: '5'.repeat(64),
      upstreamCommit: '6'.repeat(40), upstreamH1: 'h1:' + 'A'.repeat(43) + '=', upstreamZipSha256: '7'.repeat(64) } };
  for (const row of Object.values(contract.platforms)) {
    row.sourceCommit = contract.sourceCommit; row.sourceTree = contract.sourceTree;
    row.sourceIdentity.replacements.push({ module: 'github.com/metacubex/utls',
      path: contract.sourcePolicy.metacubex.replacementPath, tree: contract.sourcePolicy.metacubex.replacementTree });
    row.buildGraph.localReplacements.push({ module: 'github.com/metacubex/utls', version: 'v1.8.8', h1: '',
      replacement: { module: './third_party/metacubex-utls', version: '(devel)', h1: '' } });
  }
  if (v2) {
    const directories = {
      'github.com/sagernet/sing-tun': 'sing-tun', 'github.com/sagernet/tailscale': 'tailscale',
      'github.com/metacubex/utls': 'metacubex-utls', 'github.com/sagernet/cronet-go': 'cronet-go',
      'github.com/sagernet/fswatch': 'fswatch', 'github.com/sagernet/sing': 'sing',
      'github.com/sagernet/sing-quic': 'sing-quic', 'github.com/sagernet/wireguard-go': 'wireguard-go',
      'github.com/database64128/tfo-go/v2': 'tfo-go',
    };
    if (v2 === 3) directories['github.com/sagernet/sing-vmess'] = 'sing-vmess';
    contract.sourcePolicy.schema = v2 === 3 ? 'polaris-reviewed-kernel-source-v3' : 'polaris-reviewed-kernel-source-v2';
    contract.sourcePolicy.vendors = Object.fromEntries(Object.entries(directories).map(([name, directory], index) => [name,
      name === contract.sourcePolicy.metacubex.module
        ? Object.fromEntries(['version', 'replacementPath', 'replacementTree', 'replacementFileCount']
          .map(key => [key, contract.sourcePolicy.metacubex[key]]))
        : { version: 'v0.0.0-fixture', replacementPath: `./third_party/${directory}`,
          replacementTree: (index + 1).toString(16).repeat(40), replacementFileCount: index + 1 }]));
    for (const row of Object.values(contract.platforms)) {
      row.sourceIdentity.replacements = Object.entries(contract.sourcePolicy.vendors).map(([module, pin]) =>
        ({ module, path: pin.replacementPath, tree: pin.replacementTree }));
      row.buildGraph.localReplacements = Object.entries(contract.sourcePolicy.vendors).map(([module, pin]) =>
        ({ module, version: pin.version, h1: '', replacement: { module: pin.replacementPath, version: '(devel)', h1: '' } }));
    }
  }
  return contract;
}
function reviewedSelection(contract, bytes) {
  return { schema: 'polaris-app-release-selection-v1', contractPath: 'scripts/desktop-core/release-contract.json',
    contractSha256: digest(bytes), ...Object.fromEntries(['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId'].map(k => [k, contract[k]])),
    librarySha256: { linux: contract.platforms.linux.sidecar.sha256, win: contract.platforms.win.sidecar.sha256 } };
}
test('reviewed final-source pair admits only its exact Go and Meta source facts', () => {
  const contract = reviewedFixture();
  assert.equal(validateReleaseContract(contract), contract);
  assert.equal(upstreamSourceGoVersion(contract), 'go1.27.2');
  const changes = [
    c => { c.sourceCommit = '0'.repeat(40); }, c => { c.sourceTree = '0'.repeat(40); },
    c => { c.sourcePolicy = null; }, c => { delete c.sourcePolicy; },
    c => { c.sourcePolicy.goDirective = '1.27.2'; }, c => { c.sourcePolicy.goVersion = '1.28.0'; },
    c => { c.sourcePolicy.toolchain = 'go1.27.1'; }, c => { c.sourcePolicy.metacubex.replacementFileCount = 354; },
    c => { c.platforms.linux.goVersion = 'go1.26.8'; },
    c => { c.platforms.linux.sourceIdentity.goModSha256 = '0'.repeat(64); },
    c => { c.platforms.linux.sourceIdentity.replacements[1].tree = '0'.repeat(40); },
    c => { c.platforms.linux.buildGraph.localReplacements[1].replacement.module = './unreviewed'; },
    c => { c.platforms.linux.buildGraph.localReplacements[1].replacement.h1 = c.sourcePolicy.metacubex.upstreamH1; },
    c => { c.platforms.linux.buildGraph.localReplacements[1].h1 = c.sourcePolicy.metacubex.upstreamH1; },
    c => { c.platforms.linux.buildGraph.localReplacements.push({ module: 'example.invalid/extra', version: 'v1', h1: '', replacement: { module: './extra', version: '(devel)', h1: '' } }); },
  ];
  for (const change of changes) { const changed = structuredClone(contract); change(changed); assert.throws(() => validateReleaseContract(changed)); }
});
test('reviewed v2 schema requires all nine exact vendor declarations and agrees with the shared archive validator', () => {
  const contract = reviewedFixture(true), policy = contract.sourcePolicy;
  assert.equal(reviewedSourcePolicy(contract), policy);
  assert.equal(validateReleaseContract(contract), contract);
  assert.equal(upstreamSourceGoVersion(contract), 'go1.27.2');
  const python = readFileSync(new URL('../mobile-core/source_policy.py', import.meta.url), 'utf8');
  const declaration = python.match(/VENDOR_DIRECTORIES = \{([\s\S]*?)\n\}/);
  assert.ok(declaration, 'shared Python schema inventory must remain inspectable without executing it');
  const directories = Object.fromEntries([...declaration[1].matchAll(/'([^']+)': '([^']+)'/g)].map(m => [m[1], m[2]]));
  assert.deepEqual(Object.fromEntries(Object.entries(policy.vendors).map(([name, pin]) =>
    [name, pin.replacementPath.slice('./third_party/'.length)])), directories);
  const changes = [
    c => { c.sourcePolicy.schema = 'polaris-reviewed-kernel-source-v3'; },
    c => { c.sourcePolicy.schema = 'polaris-reviewed-kernel-source-v1'; },
    c => { delete c.sourcePolicy.vendors; }, c => { c.sourcePolicy.vendors = null; },
    c => { c.sourcePolicy.vendors = []; }, c => { c.sourcePolicy.extra = true; },
    c => { c.sourcePolicy.vendors['example.invalid/extra'] = structuredClone(policy.vendors['github.com/sagernet/sing']); },
    c => { c.sourcePolicy.sourceCommit = '0'.repeat(40); }, c => { c.sourcePolicy.sourceTree = '0'.repeat(40); },
    c => { c.sourcePolicy.goDirective = '1.27.2'; }, c => { c.sourcePolicy.goVersion = '1.28.0'; },
    c => { c.sourcePolicy.toolchain = 'go1.27.1'; }, c => { c.sourcePolicy.goModSha256 = 'bad'; },
    c => { c.sourcePolicy.metacubex.replacementTree = '0'.repeat(40); },
  ];
  for (const name of Object.keys(policy.vendors)) {
    changes.push(c => { delete c.sourcePolicy.vendors[name]; });
    for (const [field, value] of [['extra', true], ['version', null], ['version', 'v1'], ['version', 'v1.2.3\n'],
      ['replacementPath', './third_party/../outside'], ['replacementTree', 'bad'], ['replacementTree', 'A'.repeat(40)],
      ['replacementFileCount', 0], ['replacementFileCount', true], ['replacementFileCount', 1.5], ['replacementFileCount', 65537]])
      changes.push(c => { c.sourcePolicy.vendors[name][field] = value; });
    for (const field of ['version', 'replacementPath', 'replacementTree', 'replacementFileCount'])
      changes.push(c => { delete c.sourcePolicy.vendors[name][field]; });
  }
  for (const change of changes) {
    const changed = structuredClone(contract); change(changed);
    assert.throws(() => reviewedSourcePolicy(changed));
  }
  assert.equal(validateReleaseContract(reviewedFixture()).sourcePolicy.schema, 'polaris-reviewed-kernel-source-v1');
});
test('reviewed v3 admits exactly ten vendors without changing historical v2 or inventing release facts', () => {
  const contract = reviewedFixture(3);
  assert.equal(validateReleaseContract(contract), contract);
  assert.equal(Object.keys(reviewedSourcePolicy(contract).vendors).length, 10);
  assert.equal(Object.keys(reviewedSourcePolicy(reviewedFixture(true)).vendors).length, 9);
  const changes = [
    c => { c.sourcePolicy.schema = 'polaris-reviewed-kernel-source-v2'; },
    c => { c.sourcePolicy.schema = 'polaris-reviewed-kernel-source-v4'; },
    c => { delete c.sourcePolicy.vendors; },
    c => { c.sourcePolicy.vendors.extra = c.sourcePolicy.vendors['github.com/sagernet/sing']; },
    c => { c.tagObject = null; }, c => { c.releaseId = null; },
    c => { c.producer.runId = null; }, c => { c.assetIndex.sha256 = '0'.repeat(64); c.assetIndex.bytes = 0; },
  ];
  for (const name of Object.keys(contract.sourcePolicy.vendors)) {
    changes.push(c => { delete c.sourcePolicy.vendors[name]; });
    for (const [field, value] of [['version', 'v1'], ['replacementPath', './third_party/../outside'],
      ['replacementTree', 'bad'], ['replacementFileCount', 0], ['replacementFileCount', true]])
      changes.push(c => { c.sourcePolicy.vendors[name][field] = value; });
  }
  for (const change of changes) {
    const changed = structuredClone(contract); change(changed);
    assert.throws(() => validateReleaseContract(changed));
  }
});
for (const schema of [true, 3]) test(`reviewed ${schema === 3 ? 'v3 ten' : 'v2 nine'} desktop rows retain exact source and linked inventories on every platform`, () => {
  const contract = reviewedFixture(schema);
  for (const key of Object.keys(contract.platforms)) {
    for (const name of Object.keys(contract.sourcePolicy.vendors)) {
      const changes = [
        (r, s) => { s.tree = '0'.repeat(40); }, (r, s) => { s.path = './outside'; },
        r => { r.sourceIdentity.replacements = r.sourceIdentity.replacements.filter(s => s.module !== name); },
        (r, s) => { r.sourceIdentity.replacements.push(structuredClone(s)); },
        r => { r.buildGraph.localReplacements = r.buildGraph.localReplacements.filter(l => l.module !== name); },
        (r, s, l) => { r.buildGraph.localReplacements.push(structuredClone(l)); },
        (r, s, l) => { l.version = 'v9.9.9'; }, (r, s, l) => { l.h1 = 'h1:YQ='; },
        (r, s, l) => { l.replacement.module = './outside'; },
        (r, s, l) => { l.replacement.version = 'v1.0.0'; }, (r, s, l) => { l.replacement.h1 = 'h1:YQ='; },
        (r, s, l) => { l.replacement = { module: name, version: 'v1.0.0', h1: 'h1:YQ=' }; },
        r => { r.buildGraph.localReplacements = r.buildGraph.localReplacements.filter(l => l.module !== name);
          r.buildGraph.normalModules.push({ module: name, version: 'v1.0.0', h1: 'h1:YQ=' }); },
      ];
      for (const change of changes) {
        const changed = structuredClone(contract), row = changed.platforms[key];
        change(row, row.sourceIdentity.replacements.find(s => s.module === name), row.buildGraph.localReplacements.find(l => l.module === name));
        assert.throws(() => validateReleaseContract(changed), undefined, `${key}: ${name}`);
      }
    }
  }
  for (const change of [
    c => { c.platforms.linux.sourceIdentity.replacements.push({ module: 'example.invalid/extra', path: './extra', tree }); },
    c => { c.platforms.linux.buildGraph.localReplacements.push({ module: 'example.invalid/extra', version: 'v1.0.0', h1: '',
      replacement: { module: './extra', version: '(devel)', h1: '' } }); },
    c => { c.platforms.linux.sourceIdentity.goSumSha256 = '0'.repeat(64); },
    c => { c.platforms.linux.goVersion = 'go1.26.8'; },
  ]) { const changed = structuredClone(contract); change(changed); assert.throws(() => validateReleaseContract(changed)); }
});
test('reviewed v2 source schema never supplies missing final tag producer or public asset facts', () => {
  const contract = reviewedFixture(true);
  for (const change of [c => { c.tagObject = null; }, c => { c.releaseId = null; },
    c => { c.producer.runId = null; }, c => { c.producer.workflowCommit = '0'.repeat(40); },
    c => { c.platforms.linux.releaseAsset.sha256 = '0'.repeat(64); },
    c => { c.platforms.linux.finalPackageCronetSmoke.status = 'pending'; },
    c => { c.sourceArchive.sha256 = null; }]) {
    const changed = structuredClone(contract); change(changed);
    assert.equal(reviewedSourcePolicy(changed), changed.sourcePolicy);
    assert.throws(() => validateReleaseContract(changed));
  }
});
test('same release version cannot reuse an old contract SHA or source selection', () => {
  const old = fixture().contract, next = reviewedFixture();
  assert.equal(old.version, next.version);
  const root = mkdtempSync(join(tmpdir(), 'reviewed-source-selection-'));
  try {
    mkdirSync(join(root, 'scripts/desktop-core'), { recursive: true });
    const oldBytes = JSON.stringify(old), nextBytes = JSON.stringify(next);
    const selection = reviewedSelection(old, oldBytes);
    writeFileSync(join(root, selection.contractPath), oldBytes);
    assert.equal(desktopSourceGoVersion(root, { desktopRelease: selection }), '1.26.8');
    writeFileSync(join(root, selection.contractPath), nextBytes);
    assert.throws(() => desktopSourceGoVersion(root, { desktopRelease: selection }), /SHA/);
    selection.contractSha256 = digest(nextBytes);
    assert.throws(() => desktopSourceGoVersion(root, { desktopRelease: selection }), /differs/);
    const current = reviewedSelection(next, nextBytes);
    assert.equal(desktopSourceGoVersion(root, { desktopRelease: current }), '1.27.2');
    next.platforms.win.goVersion = 'go1.25.5';
    const changed = JSON.stringify(next); current.contractSha256 = digest(changed);
    writeFileSync(join(root, current.contractPath), changed);
    assert.throws(() => desktopSourceGoVersion(root, { desktopRelease: current }), /toolchain/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
test('actual Go1.27.2 BuildInfo preserves real local devel identity with no invented h1', () => {
  const contract = reviewedFixture(), row = contract.platforms.linux;
  const bytes = Buffer.alloc(64); Buffer.from([127, 69, 76, 70, 2, 1]).copy(bytes); bytes.writeUInt16LE(62, 18);
  row.core = { ...row.core, bytes: bytes.length, sha256: digest(bytes) };
  row.buildGraph.settings = { GOOS: 'linux', GOARCH: 'amd64', CGO_ENABLED: '0', '-trimpath': 'true', '-tags': row.tags };
  const raw = 'fixture-path: go1.27.2\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n\tmod\tgithub.com/sagernet/sing-box\t(devel)\n'
    + '\tdep\texample.invalid/fixture\tv1.0.0\th1:YQ=\n\tdep\tgithub.com/sagernet/sing-tun\tv1.0.0\n\t=>\t./third_party/sing-tun\t(devel)\n'
    + '\tdep\tgithub.com/metacubex/utls\tv1.8.8\n\t=>\t./third_party/metacubex-utls\t(devel)\n'
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
for (const schema of [true, 3]) test(`reviewed ${schema === 3 ? 'v3 ten' : 'v2 nine'} inert BuildInfo matches all linked vendors on all four desktop targets`, () => {
  const contract = reviewedFixture(schema), root = mkdtempSync(join(tmpdir(), 'reviewed-vendor-buildinfo-'));
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

test('unknown historical source pairs never inherit a toolchain from a release label', () => {
  const { contract } = fixture();
  assert.equal(upstreamSourceGoVersion(contract), 'go1.26.8');
  for (const context of [
    { ...contract, sourceCommit: '0'.repeat(40) },
    { ...contract, sourceTree: '0'.repeat(40) },
    { ...contract, sourceCommit: 'b0247cc930bc892a0ea03138906b548b6e84ac50' },
  ]) assert.throws(() => upstreamSourceGoVersion(context), /source|tree/);
});
test('source replacement inventory cannot claim an unlinked module or invented local sum', () => {
  for (const change of [
    row => { row.sourceIdentity.replacements.push({ module: 'example.invalid/unlinked', path: './unlinked', tree }); },
    row => { row.buildGraph.localReplacements[0].h1 = 'h1:YQ='; },
    row => { row.sourceIdentity.replacements[0].path = './unlinked'; },
  ]) {
    const { contract } = fixture(); change(contract.platforms.linux);
    assert.throws(() => validateReleaseContract(contract), /replacement/);
  }
});

function brandedFixture(version) {
  const value = fixture(version);
  value.index = structuredClone(value.index);
  for (const row of value.index.platforms) {
    row.archive.releaseName = 'sing-box-polaris-' + row.archive.name.slice('sing-box-'.length);
  }
  for (const row of Object.values(value.contract.platforms)) {
    row.archive.releaseName = 'sing-box-polaris-' + row.archive.name.slice('sing-box-'.length).replace('-polaris-', '-');
    row.releaseAsset.name = row.archive.releaseName;
    row.releaseAsset.url = `https://github.com/${value.contract.repository}/releases/download/${value.contract.tag}/${row.releaseAsset.name}`;
  }
  return value;
}
test('explicit public release names keep all four original archive layouts and byte pins', () => {
  const legacy = fixture(), { contract, index } = brandedFixture();
  validateReleaseContract(contract);
  validateUpstreamIndex(index, contract);
  for (const [key, row] of Object.entries(contract.platforms)) {
    const { releaseName, ...archive } = row.archive;
    assert.deepEqual(archive, legacy.contract.platforms[key].archive);
    assert.equal(row.id, legacy.contract.platforms[key].id);
    assert.equal(row.releaseAsset.name, releaseName);
    assert.equal(row.releaseAsset.sha256, row.archive.sha256);
    assert.equal(row.releaseAsset.bytes, row.archive.bytes);
    assert.equal(row.releaseAsset.name.includes('1.15.0-polaris-'), false);
    assert.equal(row.version, '1.15.0-polaris-alpha.12');
    assert.equal(contract.tag, 'v1.15.0-polaris-alpha.12');
    const original = index.platforms.find(r => r.id === row.id);
    assert.deepEqual(JSON.parse(upstreamMetadata(contract, row)['provenance.json']).receipt, original);
    assert.ok(contractPublicAssets(contract).includes(row.releaseAsset));
  }
  // Legacy absence still passes the existing immutable legacy name contract.
  validateReleaseContract(legacy.contract);
  validateUpstreamIndex(legacy.index, legacy.contract);
});
test('public release names reject explicit invalid values instead of falling back', () => {
  const name = brandedFixture().contract.platforms.linux.archive.releaseName;
  const invalid = [null, undefined, '', 1, false, {}, [], '../' + name, '/' + name,
    'https://example.invalid/' + name, name.replace('linux-amd64', 'windows-amd64'),
    name.replace('1.15.0', '1.16.0'), name.replace('1.15.0-', '1.15.0-polaris-'), name.replace('sing-box-polaris-', 'sing-box-'),
    name.replace('sing-box-polaris-', 'sing-box-polaris-polaris-'), name + '?download=1',
    name.replaceAll('-', '\\'), name.toUpperCase(), name + '\n'];
  for (const value of invalid) {
    const { contract } = brandedFixture();
    contract.platforms.linux.archive.releaseName = value;
    assert.throws(() => validateReleaseContract(contract), /release archive name/);
  }
  const { contract } = fixture();
  contract.platforms.linux.archive.publicName = name;
  assert.throws(() => validateReleaseContract(contract), /public-name alias/);
});
test('branded public names cannot relax asset identity, bytes, original layout or indexed receipt binding', () => {
  const changes = [
    c => { c.platforms.linux.releaseAsset.name = c.platforms.linux.archive.name; },
    c => { c.platforms.linux.releaseAsset.url = c.platforms.linux.releaseAsset.url.replace('sing-box-polaris-1.', 'sing-box-1.'); },
    c => { c.platforms.linux.releaseAsset.sha256 = 'b'.repeat(64); },
    c => { c.platforms.linux.releaseAsset.bytes++; },
    c => { c.platforms.linux.releaseAsset.releaseId++; },
    c => { c.platforms.linux.releaseAsset.assetId = c.sourceArchive.assetId; },
    c => { c.platforms.linux.archive.name = c.platforms.linux.archive.releaseName; },
    c => { c.platforms.linux.archive.root = c.platforms.linux.archive.root.replace('sing-box-', 'sing-box-polaris-'); },
    c => { c.platforms.linux.id = c.platforms.linux.id.replace('sing-box-', 'sing-box-polaris-'); },
  ];
  for (const change of changes) {
    const { contract } = brandedFixture(); change(contract);
    assert.throws(() => validateReleaseContract(contract));
  }
  for (const change of [row => { delete row.archive.releaseName; }, row => { row.archive.releaseName += '-other'; },
    row => { row.archive.releaseName = row.archive.releaseName.replace('1.15.0-polaris-', '1.15.0-'); },
    row => { row.archive.members[`${row.archive.root}/LICENSE`] = 'b'.repeat(64); },
    row => { row.rawBuildInfo += 'changed'; }]) {
    const { contract, index } = brandedFixture();
    const altered = structuredClone(index); change(altered.platforms[0]);
    assert.throws(() => validateUpstreamIndex(altered, contract), /indexed recipe|release archive name/);
  }
});

test('explicit public names in unselected index recipes obey the same closed mapping', () => {
  const { contract, index } = brandedFixture();
  const extra = structuredClone(index.platforms[0]); extra.id += '-unselected';
  index.platforms.push(extra);
  validateUpstreamIndex(index, contract);
  extra.archive.releaseName = 'https://example.invalid/archive.tar.gz';
  assert.throws(() => validateUpstreamIndex(index, contract), /release archive name/);
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
  const legacy = { schema: 'polaris-box-release-consumer-contract-v1' };
  const legacyRow = { schema: 'polaris-production-native-receipt-v1', sourceFingerprint: { goVersion: 'go1.25.5' } };
  assert.equal(nativeReleaseGoVersion(legacy, legacyRow), 'go1.25.5');
  assert.throws(() => nativeReleaseGoVersion(legacy, { schema: legacyRow.schema, goVersion: 'go1.25.5' }), /Go version/);
  assert.throws(() => nativeReleaseGoVersion(legacy, row), /schema/);
});

function committedMaterialsFixture() {
  const { contract, index } = fixture();
  const raw = Buffer.from(JSON.stringify(index) + '\n');
  contract.materialTransport = 'committed-index-github-source-v1';
  contract.producerSourceArchive = { ...index.sourceArchive };
  contract.assetIndex = { name: 'polaris-upstream-native-assets.json', path: 'scripts/desktop-core/release-asset-index.json', bytes: raw.length, sha256: digest(raw) };
  contract.sourceArchive = { transport: 'github-commit-archive-v1', name: `sing-box-polaris-${commit}.tar.gz`, bytes: 2, sha256: 'b'.repeat(64),
    url: `https://github.com/${contract.repository}/archive/${commit}.tar.gz` };
  return { contract, index, raw };
}
test('committed original index and fixed commit source remove only the two public sidecar dependencies', t => {
  const { contract, index, raw } = committedMaterialsFixture();
  validateReleaseContract(contract); validateUpstreamIndex(index, contract);
  assert.deepEqual(contractPublicAssets(contract), Object.values(contract.platforms).map(r => r.releaseAsset));
  assert.notEqual(contract.sourceArchive.sha256, contract.producerSourceArchive.sha256);
  const f = installationFixture(t, contract);
  assert.throws(() => releaseInstallationContract(f.root, f.manifest), /ENOENT/);
  writeFileSync(join(f.root, contract.assetIndex.path), raw);
  assert.equal(releaseInstallationContract(f.root, f.manifest).sourceArchive.transport, 'github-commit-archive-v1');
  writeFileSync(join(f.root, contract.assetIndex.path), Buffer.concat([raw, Buffer.from(' ')]));
  assert.throws(() => releaseInstallationContract(f.root, f.manifest), /Release bytes differ/);
});
test('committed materials reject aliases, omitted transport, mixed asset IDs and relabeled producer source', () => {
  const { contract, index } = committedMaterialsFixture();
  for (const mutate of [c => { c.materialTransport = 'anything'; }, c => { delete c.materialTransport; },
    c => { c.assetIndex.path = '../outside.json'; }, c => { c.assetIndex.assetId = 123; },
    c => { c.sourceArchive.assetId = 123; }, c => { c.sourceArchive.transport = 'release'; },
    c => { c.sourceArchive.url = c.sourceArchive.url.replace(commit, 'main'); },
    c => { c.sourceArchive.name = c.producerSourceArchive.name; },
    c => { c.producerSourceArchive.sha256 = c.sourceArchive.sha256; },
    c => { c.platforms.linux.releaseAsset.assetId = c.platforms.win.releaseAsset.assetId; }]) {
    const bad = structuredClone(contract); mutate(bad);
    assert.throws(() => { validateReleaseContract(bad); validateUpstreamIndex(index, bad); });
  }
});
test('automatic commit archive permits only its exact anonymous codeload redirect', async () => {
  const url = `https://github.com/polaris-arch/sing-box-polaris/archive/${commit}.tar.gz`;
  const destination = `https://codeload.github.com/polaris-arch/sing-box-polaris/tar.gz/${commit}`;
  const seen = [];
  const options = { repository: 'polaris-arch/sing-box-polaris', apiToken: 'fixture', fetch: async (at, opts) => {
    seen.push([at, opts.headers.Authorization]);
    return at === url ? new Response(null, { status: 302, headers: { location: destination } }) : new Response('ok');
  } };
  assert.equal(String(await publicBytes(url, 2, true, options)), 'ok');
  assert.deepEqual(seen, [[url, undefined], [destination, undefined]]);
  for (const wrong of [destination.replace(commit, 'main'), destination.replace(commit, '0'.repeat(40)),
    destination.replace('polaris-arch', 'elsewhere'), destination + '?token=bad',
    'https://release-assets.githubusercontent.com/other', destination.replace('https:', 'http:')]) {
    await assert.rejects(publicBytes(url, 2, true, { ...options, fetch: async () => new Response(null, { status: 302, headers: { location: wrong } }) }), /Unapproved release redirect/);
  }
  await assert.rejects(publicBytes(url, 2, true, { ...options, fetch: async () => new Response(null, { status: 302, headers: { location: destination } }) }), /Unapproved release redirect/);
});


test('published source observation needs no Actions and cannot claim fresh producer success', async () => {
  const { contract } = fixture(), responses = evidenceFixture(contract), calls = [];
  const request = async path => { calls.push(path); assert(!path.startsWith('/actions/')); return structuredClone(responses.get(path)); };
  const observed = await readUpstreamSourceEvidence(contract, request);
  validatePublishedUpstreamReadback(observed, contract);
  assert.deepEqual(calls, [`/git/trees/${tree}?recursive=1`]);
  assert.equal(Object.hasOwn(observed, 'producerReadback'), false);
  assert.throws(() => validateUpstreamReadback(observed, contract), /producer readback/);
  assert.throws(() => validatePublishedUpstreamReadback({ ...observed, producerReadback: {} }, contract), /live producer/);
  for (const sourceGitlinks of [undefined, null, [{ path: '../escape', sha: 'e'.repeat(40) }],
    [{ path: 'x', sha: 'bad' }], [...observed.sourceGitlinks, ...observed.sourceGitlinks]])
    assert.throws(() => validatePublishedUpstreamReadback({ sourceGitlinks }, contract));
  for (const mutate of [r => { r.sha = '0'.repeat(40); }, r => { r.truncated = true; },
    r => { r.tree = null; }, r => { r.tree[0].mode = '100644'; }]) {
    const response = structuredClone(responses.get(`/git/trees/${tree}?recursive=1`)); mutate(response);
    await assert.rejects(readUpstreamSourceEvidence(contract, async () => response));
  }
  for (const unavailable of ['/actions/runs/1/attempts/1', '/actions/workflows/10', '/actions/jobs/1'])
    await assert.rejects(readUpstreamPublicEvidence(contract, async path => {
      if (path === unavailable) throw new Error('Actions unavailable');
      return structuredClone(responses.get(path));
    }), /Actions unavailable/);
});

// Integration uses inert binary headers and real archive extraction/hash checks.
// Go inspection and source-tree subprocess are mocked; full source validation is
// independently exercised by test-source-archive.py. No native execution occurs.
test('published consume, packaged verification and native gate work with every Actions URL denied', async t => {
  const { contract } = fixture(), root = mkdtempSync(join(tmpdir(), 'published-no-actions-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const transport = new Map(), calls = [], sourceChecks = [];
  const encode = value => Buffer.from(JSON.stringify(value));
  const put = (path, bytes) => { mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, bytes); };
  const sourceBytes = Buffer.from('inert source: subprocess proof is mocked');
  Object.assign(contract.sourceArchive, { bytes: sourceBytes.length, sha256: digest(sourceBytes) });
  transport.set(contract.sourceArchive.url, sourceBytes);
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
    execFileSync('python3', ['-c', `import base64,io,json,sys,tarfile,zipfile
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
  const { schema, providerContract, releaseId, assetIndex, platforms, sourceArchive, ...base } = contract;
  const index = encode({ schema: 'polaris-upstream-native-assets-v1', ...base,
    sourceArchive: { name: sourceArchive.name, bytes: sourceArchive.bytes, sha256: sourceArchive.sha256 },
    platforms: Object.values(platforms).map(({releaseAsset,...row})=>row) });
  Object.assign(contract.assetIndex,{ bytes:index.length,sha256:digest(index) });
  // Match the active transport: reviewed index in App, fixed-commit source archive.
  contract.materialTransport = 'committed-index-github-source-v1';
  contract.producerSourceArchive = { name: sourceArchive.name, bytes: sourceArchive.bytes, sha256: sourceArchive.sha256 };
  contract.assetIndex = { name: contract.assetIndex.name, bytes: index.length, sha256: digest(index), path: 'scripts/desktop-core/release-asset-index.json' };
  contract.sourceArchive = { transport: 'github-commit-archive-v1', name: `sing-box-polaris-${contract.sourceCommit}.tar.gz`,
    bytes: sourceBytes.length, sha256: digest(sourceBytes), url: `https://github.com/${contract.repository}/archive/${contract.sourceCommit}.tar.gz` };
  transport.set(contract.sourceArchive.url, sourceBytes);
  put(join(root,contract.assetIndex.path),index);
  const raw = encode(contract), manifest = { desktopRelease: { schema: 'polaris-app-release-selection-v1', contractPath:'scripts/desktop-core/release-contract.json', contractSha256:digest(raw),
    ...Object.fromEntries(['version','tag','tagObject','sourceCommit','sourceTree','releaseId'].map(k=>[k,contract[k]])),
    librarySha256:{linux:contract.platforms.linux.sidecar.sha256,win:contract.platforms.win.sidecar.sha256} } };
  const repo = join(dirname(fileURLToPath(import.meta.url)),'../..');
  for (const name of ADAPTER_FILES) { mkdirSync(dirname(join(root,name)),{recursive:true});copyFileSync(join(repo,name),join(root,name)); }
  put(join(root,manifest.desktopRelease.contractPath),raw);
  const api = `https://api.github.com/repos/${contract.repository}`;
  transport.set(api,encode({id:contract.repositoryId,full_name:contract.repository,private:false}));
  transport.set(api+`/releases/${contract.releaseId}`,encode({id:contract.releaseId,tag_name:contract.tag,draft:false,immutable:false,
    html_url:`https://github.com/${contract.repository}/releases/tag/${contract.tag}`,
    assets:contractPublicAssets(contract).map(a=>({id:a.assetId,name:a.name,size:a.bytes,state:'uploaded',browser_download_url:a.url,digest:`sha256:${a.sha256}`}))}));
  transport.set(api+`/git/ref/tags/${contract.tag}`,encode({ref:`refs/tags/${contract.tag}`,object:{type:'tag',sha:contract.tagObject}}));
  transport.set(api+`/git/tags/${contract.tagObject}`,encode({sha:contract.tagObject,tag:contract.tag,object:{type:'commit',sha:contract.sourceCommit}}));
  transport.set(api+`/git/commits/${contract.sourceCommit}`,encode({sha:contract.sourceCommit,tree:{sha:contract.sourceTree}}));
  transport.set(api+`/git/trees/${contract.sourceTree}?recursive=1`,encode({sha:contract.sourceTree,truncated:false,tree:[]}));
  const read = async url => { calls.push(url); assert(!url.includes('/actions/'),'Actions forbidden');assert(transport.has(url),url);return transport.get(url); };
  const run = (command,args,options) => {
    if(command==='git')return args[0]==='rev-parse'?'c'.repeat(40):'';
    if(command==='go'){const row=payloads.get(digest(readFileSync(args.at(-1))));assert(row);return args[0]==='version'?row.rawBuildInfo:row.buildID;}
    if(args[1]?.endsWith('release-archive.py') && options.input && JSON.parse(options.input).sourceArchive){assert.equal(args.at(-1),'--source-legal-pins');sourceChecks.push(JSON.parse(options.input));return '{}';}
    return execFileSync(command,args,options);
  };
  const receipts=await consumeRelease(root,manifest,undefined,{read,run,host:'linux'});
  for(const [key,row] of Object.entries(contract.platforms)){
    const receipt=verifyPackagedRelease(root,manifest,key,join(root,'resources',key,row.core.member),run,'linux');
    assert.equal(Object.hasOwn(receipt.publicReadback,'producerReadback'),false);
    const context={repository:'polaris-arch/Polaris',runId:'123',attempt:'1'};
    const facts={schema:'polaris-app-native-release-gate-v1',candidate:receipt.adapter.facts.candidate,context,contractSha256:manifest.desktopRelease.contractSha256,
      platform:key,host:{linux:'linux',win:'win32','mac-x64':'darwin','mac-arm64':'darwin'}[key],arch:key==='mac-arm64'?'arm64':'x64',receipt,
      readback:JSON.parse(readFileSync(join(root,'resources/.release-receipts/public-readback.json'))),indexSha256:contract.assetIndex.sha256,
      inspection:'download-extract-buildinfo-buildid-cronet-native-signature',actualAppAcceptance:'pending load, gvisor/mixed, feature and final carrier'};
    verifyNativeReleaseGate({...facts,fingerprint:digest(portableCanonical(facts))},contract,manifest.desktopRelease,receipt.adapter,context,key);
  }
  assert.equal(Object.keys(receipts).length,4);assert.equal(sourceChecks.length,5);
  assert(sourceChecks.every(c=>c.sourceTree===contract.sourceTree&&c.sourceGitlinks.length===0));
  for (const check of sourceChecks) assert.deepEqual(check.legalFiles, contract.legalFiles.map(({path,bytes,sha256})=>({path,bytes,sha256})));
  assert.equal(calls.filter(url=>url.includes('/actions/')).length,0);
  assert.equal(calls.filter(url=>url.includes('/git/trees/')).length,2);
  const publicURL = api+`/releases/${contract.releaseId}`, publicBytes = transport.get(publicURL);
  for (const mutate of [r=>{r.id++;},r=>{r.draft=true;},r=>{r.tag_name+='changed';},
    r=>{r.assets[0].id++;},r=>{r.assets[0].digest='sha256:'+'0'.repeat(64);},
    r=>{r.assets[0].size++;},r=>{r.assets[0].browser_download_url+='?changed';}]) {
    const changed=JSON.parse(publicBytes);mutate(changed);transport.set(publicURL,encode(changed));
    await assert.rejects(consumeRelease(root,manifest,['linux'],{read,run}));
    transport.set(publicURL,publicBytes);
  }
  for (const url of [contract.sourceArchive.url,contract.platforms.linux.releaseAsset.url]) {
    const original=transport.get(url);transport.set(url,Buffer.alloc(original.length));
    await assert.rejects(consumeRelease(root,manifest,['linux'],{read,run}));transport.set(url,original);
  }
  const treeURL=api+`/git/trees/${contract.sourceTree}?recursive=1`, treeBytes=transport.get(treeURL);
  transport.set(treeURL,encode({sha:contract.sourceTree,truncated:true,tree:[]}));
  await assert.rejects(consumeRelease(root,manifest,['linux'],{read,run}));transport.set(treeURL,treeBytes);
  put(join(root,contract.assetIndex.path),Buffer.alloc(index.length));
  await assert.rejects(consumeRelease(root,manifest,['linux'],{read,run}));put(join(root,contract.assetIndex.path),index);
  let reads=0;
  await assert.rejects(consumeRelease(root,manifest,['linux'],{run,read:async url=>{
    if(url===publicURL && ++reads===2){const r=JSON.parse(publicBytes);r.assets[0].digest=null;return encode(r);}
    return read(url);
  }}),/identity changed/);
  // Failed acquisitions never replace the already installed, verified payload.
  verifyPackagedRelease(root,manifest,'linux',join(root,'resources/linux/sing-box'),run,'linux');
  // Reuse this exact original source/archive fixture. Only Go inspection and
  // full-tree subprocess remain its existing mocks; cache does not certify them.
  const cacheParent=mkdtempSync(join(tmpdir(),'published-blob-cache-')), blobCacheDir=join(fs.realpathSync(cacheParent),'store');
  t.after(()=>rmSync(cacheParent,{recursive:true,force:true}));
  const reports=[], options={read,run,host:'linux',blobCacheDir,report:line=>reports.push(line)};
  const admissionStop=Error('private upstream cache admission observation only');
  await assert.rejects(consumeRelease(root,manifest,['linux'],{...options,read:async()=>{throw admissionStop;}}),error=>error===admissionStop);
  const cacheSupported=reports.includes('Release blob cache ready');
  assert.equal(reports.includes('Release blob cache unavailable'),!cacheSupported);reports.length=0;

  const payloadURLs=[contract.sourceArchive.url,...Object.values(contract.platforms).map(row=>row.releaseAsset.url)];
  const payloadCount=()=>calls.filter(url=>payloadURLs.includes(url)).length;
  const sourceCount=()=>sourceChecks.length;
  const bytesAt=()=>Object.fromEntries(Object.entries(contract.platforms).map(([key,row])=>[key,digest(readFileSync(join(root,'resources',key,row.core.member)))]));
  await t.test('raw cache upstream cold/warm still reprove source and both public snapshots',async()=>{
    const n=payloadCount(), checks=sourceCount();
    const cold=await consumeRelease(root,manifest,undefined,options), coldBytes=bytesAt();
    assert.equal(payloadCount()-n,5);assert.equal(sourceCount()-checks,1);
    const at=calls.length, first=payloadCount(), priorChecks=sourceCount();
    const warm=await consumeRelease(root,manifest,undefined,options);
    assert.deepEqual(warm,cold);assert.deepEqual(bytesAt(),coldBytes);
    assert.equal(payloadCount()-first,cacheSupported?0:5);assert.equal(sourceCount()-priorChecks,1);
    assert.equal(calls.slice(at).filter(url=>url===publicURL).length,2);
    assert.equal(calls.slice(at).filter(url=>url===treeURL).length,2);
    assert.equal(reports.filter(line=>line.endsWith(' hit')).length,cacheSupported?5:0);
    for(const key of Object.keys(contract.platforms))verifyPackagedRelease(root,manifest,key,join(root,'resources',key,contract.platforms[key].core.member),run,'linux');
  });
  await t.test('raw cache upstream hit still rejects complete-source failure and fresh source drift',async()=>{
    const installed=bytesAt(), failure=Error('complete source tree validation rejected');
    await assert.rejects(consumeRelease(root,manifest,['linux'],{...options,run:(command,args,settings)=>{
      if(args[1]?.endsWith('release-archive.py') && settings.input && JSON.parse(settings.input).sourceArchive)throw failure;
      return run(command,args,settings);
    }}),error=>error===failure);assert.deepEqual(bytesAt(),installed);
    let snapshots=0;
    await assert.rejects(consumeRelease(root,manifest,['linux'],{...options,read:async url=>{
      const bytes=await read(url);
      if(url===treeURL && ++snapshots===2)return encode({sha:'0'.repeat(40),truncated:false,tree:[]});return bytes;
    }}),/source tree|source observation|source.*differ|Source tree/i);
    assert.equal(snapshots,2);assert.deepEqual(bytesAt(),installed);
  });
  await t.test('raw cache upstream bad source bytes fall back without storing failed downloads',async()=>{
    if(!cacheSupported){await consumeRelease(root,manifest,['linux'],options);assert.equal(fs.existsSync(blobCacheDir)?fs.readdirSync(blobCacheDir).filter(name=>name.endsWith('.blob')).length:0,0);return;}
    const pin=contract.sourceArchive, path=join(blobCacheDir,pin.sha256+'.blob'), original=transport.get(pin.url), before=bytesAt();
    writeFileSync(path,Buffer.alloc(pin.bytes));transport.set(pin.url,Buffer.alloc(pin.bytes));
    try{await assert.rejects(consumeRelease(root,manifest,['linux'],options),/complete source archive differs/);assert.deepEqual(bytesAt(),before);assert.deepEqual(readFileSync(path),Buffer.alloc(pin.bytes));}
    finally{transport.set(pin.url,original);writeFileSync(path,original);}
  });
  await t.test('raw cache same Core regenerates receipts for two actual clean Git adapter commits',async()=>{
    // The archive/source fixture stays identical. Use actual ordinary Git here,
    // so the new adapter's HEAD and changed file closure are read by production.
    const isolateGitEnvironment=(base)=>{
      const env={...base};
      // Repository/index/object selectors, injected -c configuration, identities,
      // dates, templates and executable paths are all invocation-local here.
      for(const key of Object.keys(env))if(/^GIT_/i.test(key))delete env[key];
      const clean={...env,GIT_CONFIG_NOSYSTEM:'1',GIT_CONFIG_GLOBAL:devNull,GIT_OPTIONAL_LOCKS:'0'};
      // Plain-object child env keys must have no Windows case-insensitive alias.
      assert.deepEqual(Object.keys(clean).filter(key=>/^GIT_/i.test(key)).sort(),['GIT_CONFIG_GLOBAL','GIT_CONFIG_NOSYSTEM','GIT_OPTIONAL_LOCKS']);
      return clean;
    };
    const fixtureGit=(directory,args,settings={})=>execFileSync('git',[
      '-c','user.name=Fixture','-c','user.email=fixture@example.invalid','-c','commit.gpgsign=false',...args,
    ],{...settings,cwd:directory,env:isolateGitEnvironment(settings.env??process.env),encoding:'utf8'});
    const outside=mkdtempSync(join(tmpdir(),'cache-adapter-external-'));t.after(()=>rmSync(outside,{recursive:true,force:true}));
    // Even the first controlled repository creation receives case-variant
    // selectors. Every target here is inside this new private fixture only.
    const initialEnvironment={...process.env,GIT_DIR:join(outside,'uppercase.git'),git_dir:join(outside,'lowercase.git'),
      Git_Work_Tree:join(outside,'redirected-worktree'),git_index_file:join(outside,'redirected.index'),
      Git_Config_Count:'1',git_config_key_0:'core.worktree',gIt_CoNfIg_VaLuE_0:outside,
      Git_Author_Name:'Foreign setup author',git_committer_date:'2000-01-01T00:00:00Z'};
    const setup=(...args)=>fixtureGit(outside,args,{env:initialEnvironment});
    setup('init','--quiet');writeFileSync(join(outside,'sentinel'),'controlled external repository must not change\n');
    setup('add','--','sentinel');setup('commit','--quiet','--message','test: controlled external repository');
    assert(fs.existsSync(join(outside,'.git')));
    for(const name of ['uppercase.git','lowercase.git','redirected-worktree','redirected.index'])assert(!fs.existsSync(join(outside,name)));
    const outsideIndex=join(outside,'alternate.index');copyFileSync(join(outside,'.git/index'),outsideIndex);
    const externalBytes=()=>{
      const files={};const walk=(at,prefix='')=>{for(const name of fs.readdirSync(at)){const p=join(at,name),relative=prefix+name;const st=fs.lstatSync(p);if(st.isDirectory())walk(p,relative+'/');else files[relative]={bytes:readFileSync(p),mode:st.mode};}};
      walk(outside);return files;
    };
    const externalBefore=externalBytes(),externalHEAD=fixtureGit(outside,['rev-parse','HEAD']).trim(),sessionEnvironment={...process.env};
    const hostile={...process.env,GIT_DIR:join(outside,'.git'),GIT_WORK_TREE:outside,GIT_INDEX_FILE:outsideIndex,
      GIT_COMMON_DIR:join(outside,'.git'),GIT_OBJECT_DIRECTORY:join(outside,'.git/objects'),GIT_ALTERNATE_OBJECT_DIRECTORIES:join(outside,'.git/objects'),
      GIT_CONFIG_COUNT:'2',GIT_CONFIG_KEY_0:'core.worktree',GIT_CONFIG_VALUE_0:outside,GIT_CONFIG_KEY_1:'commit.gpgsign',GIT_CONFIG_VALUE_1:'true',
      GIT_AUTHOR_NAME:'Foreign Author',GIT_AUTHOR_EMAIL:'foreign@example.invalid',GIT_AUTHOR_DATE:'2000-01-01T00:00:00Z',
      GIT_COMMITTER_NAME:'Foreign Committer',GIT_COMMITTER_EMAIL:'committer@example.invalid',GIT_COMMITTER_DATE:'2000-01-01T00:00:00Z',GIT_TEMPLATE_DIR:outside,
      git_dir:join(outside,'.git'),Git_Work_Tree:outside,git_index_file:outsideIndex,GiT_CoMmOn_DiR:join(outside,'.git'),
      git_object_directory:join(outside,'.git/objects'),Git_Alternate_Object_Directories:join(outside,'.git/objects'),
      git_config_count:'2',Git_Config_Key_0:'core.worktree',git_config_value_0:outside,Git_Config_Key_1:'commit.gpgsign',git_config_value_1:'true',
      Git_Author_Name:'Foreign case author',git_author_email:'case@example.invalid',Git_Author_Date:'2000-01-01T00:00:00Z',
      git_committer_name:'Foreign case committer',Git_Committer_Email:'case@example.invalid',git_committer_date:'2000-01-01T00:00:00Z',Git_Template_Dir:outside};
    const git=(...args)=>fixtureGit(root,args,{env:hostile});
    git('init','--quiet');git('config','core.autocrlf','false');
    git('add','--',...ADAPTER_FILES,manifest.desktopRelease.contractPath,contract.assetIndex.path);
    git('commit','--quiet','--message','test: record first cache adapter closure');
    // Use the same complete environment isolation for production adapter reads,
    // including any selector/config values in its supplied subprocess settings.
    const actualRun=(command,args,settings)=>command==='git'?fixtureGit(root,args,{...settings,env:{...settings?.env,...hostile}}):run(command,args,settings);
    const firstHEAD=git('rev-parse','HEAD').trim(), first=await consumeRelease(root,manifest,undefined,{...options,run:actualRun});
    assert.equal(git('show','-s','--format=%an <%ae>|%cn <%ce>','HEAD').trim(),'Fixture <fixture@example.invalid>|Fixture <fixture@example.invalid>');
    const beforeBytes=bytesAt(), previousPayloads=payloadCount(), previousChecks=sourceCount(), at=calls.length;
    const changed=ADAPTER_FILES.find(name=>name==='scripts/prepare-core-identity.mjs'), path=join(root,changed);
    writeFileSync(path,Buffer.concat([readFileSync(path),Buffer.from('\n// private cache test adapter generation\n')]));
    git('add','--',changed);git('commit','--quiet','--message','test: record second cache adapter closure');
    const nextHEAD=git('rev-parse','HEAD').trim();assert.notEqual(firstHEAD,nextHEAD);
    const second=await consumeRelease(root,manifest,undefined,{...options,run:actualRun});
    assert.deepEqual(bytesAt(),beforeBytes);assert.equal(payloadCount()-previousPayloads,cacheSupported?0:5);assert.equal(sourceCount()-previousChecks,1);
    assert.equal(calls.slice(at).filter(url=>url===publicURL).length,2);assert.equal(calls.slice(at).filter(url=>url===treeURL).length,2);
    for(const key of Object.keys(contract.platforms)){
      assert.equal(first[key].adapter.facts.candidate,firstHEAD);assert.equal(second[key].adapter.facts.candidate,nextHEAD);
      assert.notEqual(first[key].adapter.fingerprint,second[key].adapter.fingerprint);assert.notEqual(first[key].fingerprint,second[key].fingerprint);
      assert.notEqual(first[key].adapter.facts.fileSha256[changed],second[key].adapter.facts.fileSha256[changed]);
      assert.deepEqual(first[key].producer,second[key].producer);assert.deepEqual(first[key].releaseAsset,second[key].releaseAsset);
      verifyPackagedRelease(root,manifest,key,join(root,'resources',key,contract.platforms[key].core.member),actualRun,'linux');
    }
    assert.equal(git('show','-s','--format=%an <%ae>|%cn <%ce>','HEAD').trim(),'Fixture <fixture@example.invalid>|Fixture <fixture@example.invalid>');
    assert.equal(fixtureGit(outside,['rev-parse','HEAD']).trim(),externalHEAD);
    assert.deepEqual(externalBytes(),externalBefore,'malicious Git selectors/config must not change any controlled external repo/index bytes');
    assert.deepEqual({...process.env},sessionEnvironment,'fixture isolation must not mutate the parent environment');
  });
});


const versionCases = JSON.parse(readFileSync(new URL('../lib/polaris-version-fixtures.json', import.meta.url), 'utf8'));
test('shared version corpus retains patch tags and exact original/public archive mapping', () => {
  assert.ok(ADAPTER_FILES.includes('scripts/lib/polaris-version.json'));
  for (const version of versionCases.accepted) {
    const { contract, index } = brandedFixture(version);
    assert.equal(validateReleaseContract(contract), contract, version);
    assert.equal(validateUpstreamIndex(index, contract), index, version);
    for (const row of Object.values(contract.platforms)) {
      assert.equal(row.version, version);
      assert.equal(row.tag, `v${version}`);
      assert.ok(row.archive.root.startsWith(`sing-box-${version}-`));
      assert.ok(row.releaseAsset.name.startsWith(`sing-box-polaris-${version.replace('-polaris-', '-')}-`));
      assert.ok(row.releaseAsset.url.includes(`/v${version}/`));
      assert.deepEqual(JSON.parse(upstreamMetadata(contract, row)['provenance.json']).receipt,
        index.platforms.find(original => original.id === row.id));
    }
    const legacy = fixture(version);
    validateReleaseContract(legacy.contract);
    validateUpstreamIndex(legacy.index, legacy.contract);
  }
  for (const version of versionCases.rejected) {
    assert.throws(() => validateReleaseContract(brandedFixture(version).contract), undefined, JSON.stringify(version));
  }
});
test('alpha.12.4 keeps its complete version in producer and public archive identities', () => {
  const version = '1.15.0-polaris-alpha.12.4';
  const { contract, index } = brandedFixture(version);
  assert.equal(validateReleaseContract(contract), contract);
  assert.equal(validateUpstreamIndex(index, contract), index);
  for (const row of Object.values(contract.platforms)) {
    assert.equal(row.version, version);
    assert.equal(row.tag, `v${version}`);
    assert.ok(row.releaseAsset.url.includes(`/v${version}/`));
  }
  for (const alter of [
    c => { c.tag = 'v1.15.0-polaris-alpha.12'; },
    c => { c.platforms.linux.version = '1.15.0-polaris-alpha.12.3'; },
    c => { c.platforms.linux.sourceCommit = 'f'.repeat(40); },
  ]) { const changed = structuredClone(contract); alter(changed); assert.throws(() => validateReleaseContract(changed)); }
});
test('alpha.12.6 keeps its complete version in producer and public archive identities', () => {
  const version = '1.15.0-polaris-alpha.12.6';
  const { contract, index } = brandedFixture(version);
  assert.equal(validateReleaseContract(contract), contract);
  assert.equal(validateUpstreamIndex(index, contract), index);
  for (const row of Object.values(contract.platforms)) {
    assert.equal(row.version, version);
    assert.equal(row.tag, `v${version}`);
    assert.ok(row.releaseAsset.url.includes(`/v${version}/`));
  }
  for (const alter of [
    c => { c.tag = 'v1.15.0-polaris-alpha.12'; },
    c => { c.platforms.linux.version = '1.15.0-polaris-alpha.12.4'; },
    c => { c.platforms.linux.sourceCommit = 'f'.repeat(40); },
  ]) { const changed = structuredClone(contract); alter(changed); assert.throws(() => validateReleaseContract(changed)); }
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
    const { contract } = brandedFixture(version); mutate(contract);
    assert.throws(() => validateReleaseContract(contract));
  }
  for (const key of ['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree']) {
    const { contract, index } = brandedFixture(version); index[key] += 'changed';
    assert.throws(() => validateUpstreamIndex(index, contract));
  }
});
