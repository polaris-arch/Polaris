// Deliberately synthetic contract fixtures. Never publication or smoke evidence.
import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, writeFileSync, rmSync, mkdirSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { nativeReleaseGoVersion } from '../native-payload-input.mjs';
import { digest, desktopSourceGoVersion, upstreamSourceGoVersion } from './source-graph.mjs';
import { RELEASE_TARGETS, validateReleaseContract, releasePayloadMembers, publicBytes, verifyReleaseCore, releaseSingTunDependency, validateReleaseNotice } from './release-consumer.mjs';
import { validateUpstreamIndex, upstreamPayloadPins, contractPublicAssets, readUpstreamPublicEvidence, validateUpstreamReadback } from './upstream-release.mjs';
// Frozen G4 source coordinates, with synthetic payloads only. These tests do
// not assert that a release exists or that any native artifact has passed.
const hash = 'a'.repeat(64), commit = '55300c55560bcc604fd9675c5526b5713db81cc1',
  tree = '7767ba9a9901a061ff07db7716baed7896cfefbc';
function fixture() {
  let id = 1;
  const contract = { schema: 'polaris-upstream-native-release-consumer-v1', providerContract: 'polaris-upstream-native-release-v1',
    repository: 'polaris-arch/sing-box-polaris', repositoryId: 1421657509, releaseId: 100,
    version: '1.15.0-polaris-alpha.12', tag: 'v1.15.0-polaris-alpha.12', tagObject: 'd'.repeat(40), sourceCommit: commit, sourceTree: tree,
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
    const selectedContract = JSON.parse(readFileSync(join(actualRoot, selectedManifest.desktopRelease.contractPath)));
    assert.equal(desktopSourceGoVersion(actualRoot, selectedManifest),
      selectedContract.schema === 'polaris-upstream-native-release-consumer-v1' ? upstreamSourceGoVersion(selectedContract).slice(2) : '1.25.5');
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
function reviewedFixture() {
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

function brandedFixture() {
  const value = fixture();
  for (const row of Object.values(value.contract.platforms)) {
    row.archive.releaseName = 'sing-box-polaris-' + row.archive.name.slice('sing-box-'.length);
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
    name.replace('1.15.0', '1.16.0'), name.replace('sing-box-polaris-', 'sing-box-'),
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
  for (const change of [row => { delete row.archive.releaseName; }, row => { row.archive.releaseName += '-other'; }]) {
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
