// Explicit release acquisition for build resources. Never executes target code.
import { hasTemporaryDesktop, loadTemporaryDesktopSelection, verifyCandidatePayload } from './native-dns-candidate.mjs';
import { execFileSync, spawnSync } from 'node:child_process';
import { chmodSync, closeSync, copyFileSync, cpSync, existsSync, fsyncSync, lstatSync, mkdirSync, mkdtempSync,
  openSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonical, digest, observeMacCodeSignature, validateMacCodeSignature, verifyMacCodeSignatureReceipt,
  kernelRepositoryIdentity, releaseGoVersion, upstreamSourceGoVersion, requireGraph as demand } from './source-graph.mjs';
import { isUpstreamRelease, isUpstreamRow, validateUpstreamContract, validateUpstreamRow, validateUpstreamIndex,
  upstreamMetadata, upstreamPayloadPins, contractPublicAssets, hasCommittedReleaseMaterials, validatePublishedUpstreamReadback, readUpstreamSourceEvidence } from './upstream-release.mjs';
import { inspectCronetBytes } from '../lib/cronet-payload.mjs';

const REPO = 'polaris-arch/polaris-box';
// v1 remains the historical archive/provenance contract. New upstream-layout
// releases require their own validated contract; changing hosting is not enough.
export function releaseRepositoryId(contract) {
  return kernelRepositoryIdentity(contract.repository).id;
}
// Only consumer steps explicitly receive the built-in Actions token. Keep it
// in this reader's closure, not in the environment inherited by native tools.
const RELEASE_API_ENV = Object.freeze({ GITHUB_ACTIONS: process.env.GITHUB_ACTIONS,
  POLARIS_RELEASE_API_TOKEN: process.env.POLARIS_RELEASE_API_TOKEN });
delete process.env.POLARIS_RELEASE_API_TOKEN;
const POLICY = 'preserve measured original bytes; ad-hoc/not-signed observations do not assert trust';
// Local inspection must fail closed before the outer gate deadline; no retries or target execution.
const LOCAL_METADATA_TIMEOUT_MS = 60_000;
const LOCAL_ARCHIVE_TIMEOUT_MS = 120_000;
const CONTRACT_PATH = 'scripts/desktop-core/release-contract.json';
const PENDING_CONTRACT_PATH = 'scripts/desktop-core/pending-release-contract.json';
const PROVIDER_FILES = ['scripts/polaris_production_release.py', 'scripts/polaris_native_candidate.py',
  'scripts/polaris_source_fingerprint.py', 'scripts/polaris_dns_identity.py', 'scripts/polaris_go_environment.py',
  'scripts/polaris_native_distribution.py', 'release/polaris-production-release.json', 'release/polaris-candidate-inputs.json'];
export const RELEASE_TARGETS = Object.freeze({
  linux: { os: 'linux', arch: 'amd64', core: 'sing-box', sidecar: 'libcronet.so', suffix: '.tar.gz', cgo: '0' },
  win: { os: 'windows', arch: 'amd64', core: 'sing-box.exe', sidecar: 'libcronet.dll', suffix: '.zip', cgo: '0' },
  'mac-x64': { os: 'darwin', arch: 'amd64', core: 'sing-box', sidecar: null, suffix: '.tar.gz', cgo: '1' },
  'mac-arm64': { os: 'darwin', arch: 'arm64', core: 'sing-box', sidecar: null, suffix: '.tar.gz', cgo: '1' },
});
export const ADAPTER_FILES = ['scripts/desktop-core/native-dns-candidate.mjs', 'scripts/desktop-core/native-dns-candidate.json',
  'scripts/desktop-core/native-dns-source-binding.json', 'scripts/desktop-core/validation-origin.mjs',
  'scripts/desktop-core/native-dns-gates.mjs',
  'scripts/desktop-core/release-consumer.mjs', 'scripts/desktop-core/upstream-release.mjs', 'scripts/desktop-core/release-archive.py', 'scripts/mobile-core/source_policy.py',
  'scripts/desktop-core/wire-input-handoff.mjs',
  'scripts/lib/polaris-version.json',
  'scripts/prepare-core-identity.mjs', 'scripts/lib/core-identity.mjs',
  'scripts/postprocess-appimage.mjs', 'src-tauri/build.rs',
  'src-tauri/src/runtime/core_identity.rs', 'src-tauri/src/runtime/helper.rs',
  'Cargo.toml', 'Cargo.lock', 'crates/helper/Cargo.toml',
  'crates/helper/src/cli.rs', 'crates/helper/src/platform/unix_log.rs',
  'crates/helper/src/platform/windows/daemon.rs',
  'crates/helper/src/admin_probe.rs',
  'crates/helper/src/lib.rs', 'crates/helper/src/core_cli.rs', 'crates/helper/src/core_hash.rs',
  'crates/helper/src/core_store.rs', 'crates/helper/src/core_objects.rs', 'crates/helper/src/pinned_core.rs',
  'crates/helper/src/user_control.rs', 'crates/helper/src/helper_policy.rs', 'crates/helper/src/legacy_use.rs',
  'crates/helper/src/platform/linux/daemon.rs', 'crates/helper/src/platform/linux/handler.rs',
  'crates/helper/src/platform/linux/server.rs', 'crates/helper/src/platform/linux/state.rs',
  'crates/helper/src/platform/macos/daemon.rs', 'crates/helper/src/platform/macos/handler.rs',
  'crates/helper/src/platform/macos/server.rs', 'crates/helper/src/platform/macos/native_birth.rs',
  'crates/helper/src/platform/windows/helper.rs', 'crates/helper/src/platform/windows/helper/native_birth.rs',
  'crates/helper/src/platform/windows/coreacl.rs', 'crates/helper/src/platform/windows/logic.rs',
  'crates/helper/src/platform/windows/native_caller.rs', 'crates/helper/src/platform/windows/service/win.rs',
  'crates/helper/src/platform/windows/winproc/win.rs',
  'crates/helper-client/src/lib.rs', 'crates/helper-client/src/manager.rs',
  'crates/helper-client/src/manager/install_unix.rs', 'crates/helper-client/src/manager/install_windows.rs',
  '.github/workflows/package.yml',
  'crates/helper-proto/src/lib.rs', 'crates/helper-proto/src/codec.rs', 'crates/helper-proto/src/error.rs',
  'crates/helper-client/src/client.rs', 'crates/helper-client/src/transport.rs',
  'src-tauri/src/runtime/update_install.rs',
  'src-tauri/src/runtime/core_paths.rs', 'src-tauri/src/runtime/proxy/core_binary.rs',
  'src-tauri/src/runtime/proxy/startup.rs', 'src-tauri/src/runtime/proxy/process_supervision.rs',
  'crates/helper-proto/src/core_manifest.rs', 'crates/helper-proto/src/core_protocol.rs',
  'crates/helper-proto/src/request.rs', 'crates/helper-proto/src/response.rs', 'crates/helper-proto/src/command.rs',
  'scripts/macos-nested-code.sh',
  'scripts/desktop-core/release-native-gates.mjs',
  'scripts/fetch-core.mjs', 'scripts/fetch-cronet.mjs', 'scripts/lib/bounded-download.mjs', 'scripts/desktop-core/bundle.mjs',
  'scripts/lib/cronet-payload.mjs', 'scripts/native-payload-input.mjs', 'scripts/verify-packaging.mjs',
  'crates/updater/src/core_release.rs', 'crates/updater/src/lib.rs', 'src-tauri/src/runtime/updater.rs',
  'scripts/desktop-core/source-graph.mjs', 'scripts/lib/cronet-contract.mjs', 'scripts/assert-pc-runtime-release.mjs'];
const hex = (v, n) => typeof v === 'string' && new RegExp(`^[a-f0-9]{${n}}$`).test(v);
const positive = (v) => Number.isSafeInteger(v) && v > 0;
const same = (a, b, message) => demand(canonical(a) === canonical(b), message);
// Match producer Python json.dumps(sort_keys=True, separators=(',', ':'), ensure_ascii=True).
export const portableCanonical = (v) => canonical(v).replace(/[\u007f-\uffff]/g,
  (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`);
export const hasDesktopSelection = (manifest) => Object.hasOwn(manifest, 'desktopRelease') || Object.hasOwn(manifest, 'desktopTemporary');
export const hasDesktopRelease = (manifest) => Object.hasOwn(manifest, 'desktopRelease');
const json = (path) => JSON.parse(readFileSync(path, 'utf8'));

export function releasePlatforms(argv) {
  const flags = argv.filter((a) => a.startsWith('--platform='));
  demand(flags.length <= 1 && argv.filter((a) => a === '--force').length <= 1
    && argv.every((a) => a === '--force' || a.startsWith('--platform=')), 'Release mode accepts only --platform= and --force');
  const keys = flags.length ? flags[0].slice('--platform='.length).split(',') : Object.keys(RELEASE_TARGETS);
  demand(keys.length && new Set(keys).size === keys.length && keys.every((k) => Object.hasOwn(RELEASE_TARGETS, k)), 'Unknown/duplicate/empty App architecture selection');
  return keys;
}

export function regularPath(path, directory = false) {
  let at = resolve(path);
  demand(directory ? lstatSync(at).isDirectory() : lstatSync(at).isFile(), `Non-regular release path: ${at}`);
  for (at = dirname(at);;) {
    demand(lstatSync(at).isDirectory(), `Linked release ancestor: ${at}`);
    const parent = dirname(at); if (parent === at) break; at = parent;
  }
}
function checkedBytes(path, pin) {
  regularPath(path); const bytes = readFileSync(path);
  demand(bytes.length === pin.bytes && digest(bytes) === pin.sha256, `Release bytes differ: ${path}`);
  return bytes;
}
function pin(pin, name, contract, max = 512 * 1024 * 1024) {
  demand(pin?.name === name && positive(pin.bytes) && pin.bytes <= max && hex(pin.sha256, 64)
    && positive(pin.assetId) && pin.releaseId === contract.releaseId
    && pin.url === `https://github.com/${contract.repository}/releases/download/${contract.tag}/${name}`, 'Release asset pin/URL differs');
}
function identity(value, schema, facts) {
  demand(value?.schema === schema && hex(value.fingerprint, 64), 'Portable identity shape differs');
  same(value.facts, facts, 'Portable identity facts differ');
  demand(value.fingerprint === digest(portableCanonical(facts)), 'Portable identity fingerprint differs');
}
function validateRow(contract, key, row) {
  if (isUpstreamRelease(contract)) return validateUpstreamRow(contract, key, row, RELEASE_TARGETS);
  const target = RELEASE_TARGETS[key];
  const root = `polaris-box-${contract.version}-${target.os}-${target.arch}`;
  demand(row?.schema === 'polaris-production-native-receipt-v1' && row.target === `${target.os}/${target.arch}`
    && row.version === contract.version && row.tag === contract.tag && row.sourceCommit === contract.sourceCommit
    && row.sourceTree === contract.sourceTree && row.providerContract === 'polaris-box-release-v1'
    && row.signaturePolicy === POLICY && row.signatureObservation?.trustValidated === false,
  'Producer row/source/signature policy differs');
  releaseMacCodeSignature(row, key);
  demand(row.sourceFingerprint?.schema === 'fork-source-fingerprint-v1', 'Original source collector missing');
  const facts = Object.fromEntries(['sourceCommit', 'sourceTree', 'sourceOverlay', 'goVersion',
    'goModSha256', 'goSumSha256', 'replacement'].map((k) => [k, row.sourceFingerprint[k]]));
  demand(facts.sourceCommit === contract.sourceCommit && facts.sourceTree === contract.sourceTree
    && facts.sourceOverlay === 0 && facts.goVersion === releaseGoVersion(contract) && hex(facts.goModSha256, 64)
    && hex(facts.goSumSha256, 64) && facts.replacement?.module === 'github.com/sagernet/sing-tun', 'Stable source facts differ');
  identity(row.portableSourceIdentity, 'polaris-production-source-v1', facts);
  const tags = typeof row.tags === 'string' ? row.tags.split(',') : [];
  demand(tags.length && tags.every((tag) => /^[a-z0-9_]+$/.test(tag)) && new Set(tags).size === tags.length
    && tags.includes('with_gvisor') && tags.includes('with_naive_outbound')
    && (target.sidecar ? tags.includes('with_purego') : !tags.includes('with_purego')), 'Release capabilities/tags differ');
  const platform = { sourceFingerprint: row.portableSourceIdentity.fingerprint, os: target.os, arch: target.arch,
    CGO_ENABLED: target.cgo, tags: [...tags].sort(), version: contract.version, tag: contract.tag,
    goVersion: facts.goVersion, sourceOverlay: 0 };
  identity(row.portablePlatformIdentity, 'polaris-production-platform-v1', platform);
  demand(row.sourceIdentityFingerprint === row.portableSourceIdentity.fingerprint
    && row.platformInputFingerprint === row.portablePlatformIdentity.fingerprint
    && row.buildID === `polaris-production-v1-${row.platformInputFingerprint}`
    && row.portablePlatformIdentity.buildID === row.buildID, 'Release BuildID/identity differs');
  const provider = row.providerIdentity?.facts;
  same(Object.keys(provider?.fileSha256 ?? {}).sort(), [...PROVIDER_FILES].sort(), 'Provider file inventory differs');
  demand(provider.sourceCommit === contract.sourceCommit && provider.providerContract === row.providerContract
    && Object.values(provider.fileSha256).every((v) => hex(v, 64)), 'Provider source binding differs');
  identity(row.providerIdentity, 'polaris-production-provider-v1', provider);
  demand(row.providerFingerprint === row.providerIdentity.fingerprint, 'Provider fingerprint differs');
  pin(row.releaseAsset, root + target.suffix, contract);
  same({ name: row.archive?.name, bytes: row.archive?.bytes, sha256: row.archive?.sha256 },
    { name: row.releaseAsset.name, bytes: row.releaseAsset.bytes, sha256: row.releaseAsset.sha256 }, 'Archive pin differs');
  const members = row.archive.members;
  demand(members && Object.keys(members).length <= 1024 && Object.values(members).every((v) => hex(v, 64)), 'Archive inventory invalid');
  demand(Object.keys(members).every((name) => name.startsWith(`${root}/`) && name.slice(root.length + 1).split('/').every((part) =>
    /^[A-Za-z0-9_+.-]+$/.test(part) && !['.', '..'].includes(part) && !part.endsWith('.')
    && !/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/i.test(part.split('.')[0]))), 'Archive member path invalid');
  for (const name of [target.core, 'LICENSE', 'NOTICE', 'licenses/NOTICE', 'provenance.json', 'source/SOURCE-INDEX.json', 'SHA256SUMS'])
    demand(hex(members[`${root}/${name}`], 64), `Required release member absent: ${name}`);
  function payload(p, name) {
    demand(p?.member === name && positive(p.bytes) && p.bytes <= 200 * 1024 * 1024
      && p.sha256 === members[`${root}/${name}`], 'Payload pin/layout differs');
  }
  payload(row.core, target.core);
  if (target.sidecar) payload(row.sidecar, target.sidecar);
  else demand(row.sidecar === null && !Object.keys(members).some((n) => /\.(so|dll|dylib)$/.test(n)), 'Static target contains sidecar');
  // Receipt inputs are source-owned. Actual binary BuildID is checked separately.
  const command = row.buildCommand;
  demand(Array.isArray(command) && command.length === 14 && typeof command[12] === 'string', 'Build command shape differs');
  same(command, ['go', 'build', '-buildvcs=true', '-mod=readonly', '-p', '1', '-trimpath', '-tags', row.tags,
    '-ldflags', row.ldflags, '-o', command[12], './cmd/sing-box'], 'Build command inputs differ');
  demand(command[12].replace(/\\/g, '/').endsWith(`/${root}/${target.core}`)
    && typeof row.ldflags === 'string' && row.ldflags.endsWith(` -s -w -X github.com/sagernet/sing-box/constant.Version=${contract.version} -buildid=${row.buildID}`), 'Linker version/BuildID inputs differ');
  return root;
}

export function validateReleaseContract(contract) {
  if (isUpstreamRelease(contract)) return validateUpstreamContract(contract, RELEASE_TARGETS);
  demand(contract?.schema === 'polaris-box-release-consumer-contract-v1'
    && contract.providerContract === 'polaris-box-release-v1' && contract.repository === REPO
    && positive(contract.releaseId) && hex(contract.sourceCommit, 40) && hex(contract.sourceTree, 40)
    && hex(contract.tagObject, 40) && /^1\.15\.0-alpha\.12(?:\.polaris\.[1-9][0-9]*)?$/.test(contract.version)
    && contract.tag === `v${contract.version}`, 'Unbound or unsupported desktop release contract');
  same(Object.keys(contract.platforms ?? {}).sort(), Object.keys(RELEASE_TARGETS).sort(), 'Exactly four App targets required');
  pin(contract.assetIndex, 'polaris-box-assets.json', contract, 8 * 1024 * 1024);
  const ids = [contract.assetIndex.assetId];
  for (const [key, row] of Object.entries(contract.platforms)) { validateRow(contract, key, row); ids.push(row.releaseAsset.assetId); }
  demand(new Set(ids).size === ids.length, 'Duplicate release asset identity');
  same(new Set(Object.values(contract.platforms).map((r) => r.providerFingerprint)).size, 1, 'Platform providers differ');
  same(new Set(Object.values(contract.platforms).map((r) => r.sourceIdentityFingerprint)).size, 1, 'Platform sources differ');
  return contract;
}

// Shared with the existing Rust embedded dependency gate. V2 preserves all
// Go replacement rows; the sing-tun gate must select exactly its own module.
export function releaseSingTunDependency(row) {
  let dependency;
  if (isUpstreamRow(row)) {
    demand(Array.isArray(row.buildGraph?.localReplacements), 'Upstream replacement inventory missing');
    const selected = row.buildGraph.localReplacements.filter((item) => item.module === 'github.com/sagernet/sing-tun');
    demand(selected.length === 1 && selected[0].replacement?.module === './third_party/sing-tun'
      && selected[0].replacement.version === '(devel)' && selected[0].replacement.h1 === '', 'Unique upstream sing-tun replacement required');
    dependency = selected[0];
  } else {
    demand(row?.schema === 'polaris-production-native-receipt-v1', 'Unsupported dependency receipt');
    dependency = row.buildGraph?.localReplacement;
  }
  demand(dependency?.module === 'github.com/sagernet/sing-tun' && typeof dependency.version === 'string'
    && dependency.version.length > 0 && typeof dependency.h1 === 'string', 'Release sing-tun identity differs');
  return { module: dependency.module, version: dependency.version, h1: dependency.h1 };
}

export function loadReleaseContract(root, manifest) {
  demand(!hasTemporaryDesktop(manifest), 'Formal and temporary desktop selections cannot coexist');
  const selection = manifest.desktopRelease;
  demand(selection?.schema === 'polaris-app-release-selection-v1' && [CONTRACT_PATH, PENDING_CONTRACT_PATH].includes(selection.contractPath)
    && hex(selection.contractSha256, 64), 'Explicit release selection incomplete');
  const file = join(root, selection.contractPath); regularPath(file);
  const bytes = readFileSync(file); demand(bytes.length <= 8 * 1024 * 1024 && digest(bytes) === selection.contractSha256, 'Reviewed consumer contract SHA differs');
  const contract = validateReleaseContract(JSON.parse(bytes));
  if (hasCommittedReleaseMaterials(contract)) indexRows(checkedBytes(join(root, contract.assetIndex.path), contract.assetIndex), contract);
  for (const key of ['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId'])
    same(selection[key], contract[key], `Compiled release selection ${key} differs`);
  same(selection.librarySha256, { linux: contract.platforms.linux.sidecar.sha256,
    win: contract.platforms.win.sidecar.sha256 }, 'Compiled same-package library pins differ');
  return contract;
}

// App installation identity for the upstream layout. This is deliberately a
// separate domain from both the historical producer fingerprint and the
// temporary candidate digest. Original producer rows/index bytes stay intact.
export const UPSTREAM_INSTALLATION_DOMAIN = 'polaris-upstream-native-installation-v1\0';
export function releaseInstallationContract(root, manifest) {
  const contract = loadReleaseContract(root, manifest);
  if (!isUpstreamRelease(contract)) return contract;
  demand(manifest.desktopRelease.contractPath === CONTRACT_PATH, 'Upstream installation requires the compiled canonical contract');
  const raw = readFileSync(join(root, manifest.desktopRelease.contractPath));
  demand(digest(raw) === manifest.desktopRelease.contractSha256, 'Installation contract changed during verification');
  const providerFingerprint = digest(Buffer.concat([Buffer.from(UPSTREAM_INSTALLATION_DOMAIN), raw]));
  return { ...contract, platforms: Object.fromEntries(Object.entries(contract.platforms).map(([key, row]) => {
    demand(!Object.hasOwn(row, 'providerFingerprint') && !Object.hasOwn(row, 'providerIdentity'),
      'Upstream installation cannot borrow an unverified producer identity');
    return [key, { ...row, providerFingerprint }];
  })) };
}

// The producer preserves codesign display stderr; it does not supply App signature state.
// Derive a bound state/CDHash, then use the existing strict native observer on Mac.
function releaseMacCodeSignature(row, key) {
  if (!key.startsWith('mac-')) return null;
  if (isUpstreamRow(row)) {
    const { state, cdHash, trustValidated } = row.signatureObservation ?? {};
    demand(trustValidated === false, 'Upstream signature trust must not be asserted');
    const signature = { state, cdHash }; validateMacCodeSignature(signature, key); return signature;
  }
  const original = row.signatureObservation?.core;
  demand(original?.format === 'Mach-O' && original.signatureTrustValidated === false
    && typeof original.codeSignatureInspection === 'string', 'Original Mac signature observation missing');
  const display = original.codeSignatureInspection;
  const hashes = [...display.matchAll(/^CDHash=([a-fA-F0-9]{40})\r?$/gm)];
  const unsigned = display.includes('code object is not signed at all');
  demand(display.split(/\r?\n/).filter((line) => line.startsWith('CDHash=')).length === hashes.length
    && (unsigned ? hashes.length === 0 : hashes.length === 1), 'Original Mac signature state/CDHash invalid');
  const signature = unsigned ? { state: 'unsigned', cdHash: null }
    : { state: 'signed', cdHash: hashes[0][1].toLowerCase() };
  validateMacCodeSignature(signature, key);
  return signature;
}

export function releasePayloadMembers(contract, key) {
  const row = contract.platforms[key], prefix = validateRow(contract, key, row) + '/';
  if (isUpstreamRelease(contract)) return Object.keys(upstreamPayloadPins(contract, row)).sort().map((name) => `core-release/${name}`);
  return Object.keys(row.archive.members).sort().map((name) => name.slice(prefix.length))
    .filter((name) => name !== row.core.member && name !== row.sidecar?.member)
    .map((name) => `core-release/${name}`);
}

export function validateReleaseNotice(notice, manifest, contract, mobile) {
  function section(label) {
    const lines = notice.split(/\r?\n/), starts = lines.flatMap((line, i) => line.startsWith(`  - ${label} `) ? [i] : []);
    demand(starts.length === 1, `NOTICE: unique ${label} section required`);
    const end = lines.findIndex((line, i) => i > starts[0] && line.startsWith('  - '));
    return lines.slice(starts[0], end < 0 ? lines.length : end).map((line) => line.trim());
  }
  function exact(lines, text) {
    const label = text.slice(0, text.indexOf('：') + 1);
    demand(lines.filter((line) => line.startsWith(label)).length === 1 && lines.includes(text), `NOTICE: release identity missing/duplicated: ${text}`);
  }
  const committed = hasCommittedReleaseMaterials(contract);
  const desktop = section('桌面 sing-box');
  demand(desktop[0] === `- 桌面 sing-box v${contract.version} (GPLv3) — https://github.com/${contract.repository}`, 'NOTICE: release baseline differs');
  for (const text of [`上游基线源码：https://github.com/SagerNet/sing-box/tree/v1.15.0-alpha.12`,
    `桌面发布构建：${contract.version}`, `固定源码：https://github.com/${contract.repository}/tree/${contract.sourceCommit}`,
    `源码 tree：${contract.sourceTree}`, `固定消费标签：https://github.com/${contract.repository}/tree/${contract.tag}`,
    `消费标签对象：${contract.tagObject}`, `消费合同 SHA-256：${manifest.desktopRelease.contractSha256}`,
    `${committed ? '受审原始 index' : '公开资产 index'} SHA-256：${contract.assetIndex.sha256}`, `桌面发布：https://github.com/${contract.repository}/releases/tag/${contract.tag}`]) exact(desktop, text);
  // Source guidance is bilingual. Reject contradictory legacy desktop guidance
  // anywhere in NOTICE, while preserving the independent mobile manifests.
  for (const text of [`Desktop release: ${contract.version} — https://github.com/${contract.repository}/releases/tag/${contract.tag}`,
    `Desktop source: https://github.com/${contract.repository}/tree/${contract.sourceCommit}`,
    `Desktop release contract: ${manifest.desktopRelease.contractPath} (SHA-256 ${manifest.desktopRelease.contractSha256})`,
    `${committed ? 'Committed original index' : 'Published asset index'} SHA-256: ${contract.assetIndex.sha256}`]) {
    const label = text.slice(0, text.indexOf(':') + 1), lines = notice.split(/\r?\n/);
    demand(lines.filter((line) => line.startsWith(label)).length === 1 && lines.includes(text), 'NOTICE: English release identity differs');
  }
  demand(notice.includes('`src-tauri/core-manifest.json` 的 `desktopRelease`')
    && notice.includes(committed ? '`sourceArchive` 指向固定 commit 的 GitHub 自动源码归档，按实际传输 hash 与完整 Git tree 核验。' : isUpstreamRelease(contract) ? '`sourceArchive` 指向同批完整源码归档。' : '`sourceDelivery` 指向固定公开来源。')
    && notice.includes('Desktop selection uses src-tauri/core-manifest.json desktopRelease.')
    && notice.includes(committed ? 'The committed original index retains the producer archive identity; sourceArchive pins the independently verified fixed-commit GitHub source archive,' : isUpstreamRelease(contract) ? 'asset index sourceArchive identifies the same-run complete source archive,' : 'asset index sourceDelivery identifies the automatic product source archive,'), 'NOTICE: bilingual release source guidance missing');
  demand(!/desktopSourceBuild|scripts\/desktop-core\/source-manifest\.json|desktop[^\n]*alpha\.11|Final native kernels, signatures, source archives/i.test(notice),
    'NOTICE: obsolete desktop source guidance');
  for (const [label, source] of [['Android libbox 基线', mobile.android], ['Apple libbox 基线', mobile.apple]]) {
    demand(hex(source?.sourceCommit, 40) && hex(source?.manifestSha256, 64), 'NOTICE: mobile source identity missing');
    const lines = section(label);
    demand(lines[0] === `- ${label} v${manifest.bundledCoreVersion} (GPLv3) — https://github.com/SagerNet/sing-box`, 'NOTICE: mobile baseline differs');
    for (const text of [`上游基线源码：https://github.com/SagerNet/sing-box/tree/v${manifest.bundledCoreVersion}`,
      `固定源码：https://github.com/SagerNet/sing-box/tree/${source.sourceCommit}`, `source manifest SHA-256：${source.manifestSha256}`]) exact(lines, text);
  }
}

// Redirects are permitted only for immutable asset bytes, whose expected hash is already pinned.
// Do not spend a native runner waiting for a shared anonymous IP's hourly reset.
// One short, evidenced recovery remains shared by the whole consumption.
const PRIMARY_QUOTA_WAIT_MS = 2 * 60 * 1000;
const TRANSPORT_WAIT_MS = 1000;
const TRANSPORT_CODES = new Set(['EAI_AGAIN', 'ECONNRESET', 'ETIMEDOUT', 'ECONNREFUSED', 'ENOTFOUND',
  'EACCES', 'EPERM', 'ABORT_ERR', 'ERR_TLS_CERT_ALTNAME_INVALID', 'CERT_HAS_EXPIRED',
  'DEPTH_ZERO_SELF_SIGNED_CERT', 'UNABLE_TO_VERIFY_LEAF_SIGNATURE',
  'UND_ERR_CONNECT_TIMEOUT', 'UND_ERR_HEADERS_TIMEOUT', 'UND_ERR_BODY_TIMEOUT',
  'UND_ERR_SOCKET', 'UND_ERR_ABORTED', 'UND_ERR_DESTROYED']);
const TRANSIENT_TRANSPORT_CODES = new Set(['EAI_AGAIN', 'ECONNRESET', 'ETIMEDOUT',
  'UND_ERR_CONNECT_TIMEOUT', 'UND_ERR_HEADERS_TIMEOUT', 'UND_ERR_SOCKET']);
function canonicalPublicAPI(endpoint, asset, repository = REPO) {
  kernelRepositoryIdentity(repository);
  return !asset && endpoint.protocol === 'https:' && endpoint.hostname === 'api.github.com'
    && !endpoint.port && !endpoint.username && !endpoint.password && !endpoint.hash
    && (!endpoint.search || endpoint.search === '?recursive=1'
      && endpoint.pathname.startsWith(`/repos/${repository}/git/trees/`)
      && /^[a-f0-9]{40}$/.test(endpoint.pathname.slice(`/repos/${repository}/git/trees/`.length)))
    && (endpoint.pathname === `/repos/${repository}` || endpoint.pathname.startsWith(`/repos/${repository}/`));
}
// Installation credentials are opaque Bearer values, not a frozen GitHub token-prefix alphabet.
// RFC 6750 b64token excludes whitespace/control bytes while permitting JWT dots and padding.
function releaseAPIToken(env) {
  if (env.GITHUB_ACTIONS !== 'true' || !env.POLARIS_RELEASE_API_TOKEN) return null;
  demand(typeof env.POLARIS_RELEASE_API_TOKEN === 'string'
    && env.POLARIS_RELEASE_API_TOKEN.length <= 4096
    && /^[A-Za-z0-9._~+/-]+=*$/.test(env.POLARIS_RELEASE_API_TOKEN), 'Invalid dedicated release API credential');
  return env.POLARIS_RELEASE_API_TOKEN;
}
function quotaDetail(response, token) {
  return ['x-github-request-id', 'x-ratelimit-limit', 'x-ratelimit-remaining',
    'x-ratelimit-reset', 'x-ratelimit-resource', 'retry-after'].map((name) => {
    const value = response.headers.get(name);
    const safe = value !== null && value !== token && (name === 'x-github-request-id'
      ? /^[A-Fa-f0-9:.-]{1,128}$/.test(value) : name === 'x-ratelimit-resource'
        ? ['core', 'search'].includes(value) : /^[0-9]{1,20}$/.test(value));
    return `${name}=${safe ? value : 'unknown'}`;
  }).join(', ');
}
const validETag = (value) => typeof value === 'string' && /^(?:W\/)?"[\x21\x23-\x7e]{1,256}"$/.test(value);
// If-None-Match uses weak comparison (RFC 9110 section 13.1.2).
// Header names are case-insensitive; opaque validator bytes are not.
const matchingETag = (left, right) => validETag(left) && validETag(right)
  && left.replace(/^W\//, '') === right.replace(/^W\//, '');
// Keep only a closed code vocabulary. Never copy error messages, addresses,
// URLs, signed queries or a raw cause into the public diagnostic. Mixed,
// unknown, cyclic or clipped cause graphs cannot authorize recovery.
function transportObservation(error) {
  const codes = new Set(), seen = new Set(); let complete = true, cancelled = false, kind = 'Error';
  const visit = (item, depth) => {
    if (!item || typeof item !== 'object' || depth > 4 || seen.size >= 16 || seen.has(item)) {
      complete = false; return;
    }
    seen.add(item);
    try {
      const name = item.name;
      if (depth === 0 && ['AbortError', 'TimeoutError', 'TypeError', 'Error', 'AggregateError'].includes(name)) kind = name;
      if (['AbortError', 'TimeoutError'].includes(name)) cancelled = true;
      const code = item.code, cause = item.cause, errors = item.errors;
      if (code !== undefined) {
        if (TRANSPORT_CODES.has(code)) codes.add(code);
        else complete = false;
      }
      const children = cause === undefined ? [] : [cause];
      if (errors !== undefined) {
        if (!Array.isArray(errors) || errors.length > 16) complete = false;
        else children.push(...errors);
      }
      if (!children.length && code === undefined) complete = false;
      for (const child of children) visit(child, depth + 1);
    } catch { complete = false; }
  };
  visit(error, 0);
  const sorted = [...codes].sort();
  return { kind, codes: sorted, complete,
    retryable: complete && !cancelled
      && sorted.length > 0 && sorted.every((code) => TRANSIENT_TRANSPORT_CODES.has(code)) };
}
const transportDetail = (observation) => `${observation.kind}; codes=${observation.codes.join(',') || 'unknown'}; causeComplete=${observation.complete}`;
// One evidenced primary-quota recovery across the entire consumption, including
// both public snapshots. Asset denials and unknown API denials stay fail-closed.
export function releasePublicReader(options = {}) {
  const quotaBudget = { available: true };
  const transportBudget = { available: true };
  const metadataCache = new Map(), observationBudget = { available: true }, conditionalObservationBudget = { available: true };
  const apiToken = releaseAPIToken(options.env ?? RELEASE_API_ENV);
  const now = options.now ?? Date.now;
  const supplied = options.deadlineMs ?? process.env.POLARIS_RELEASE_JOB_DEADLINE_MS;
  const deadlineMs = supplied === undefined ? now() + 120 * 60 * 1000 : Number(supplied);
  demand((supplied === undefined || /^[0-9]+$/.test(String(supplied)))
    && Number.isSafeInteger(deadlineMs) && deadlineMs > 0 && deadlineMs <= now() + 180 * 60 * 1000,
  'Invalid absolute release-consumer deadline');
  return (url, limit, asset = false) => publicBytes(url, limit, asset,
    { ...options, apiToken, quotaBudget, transportBudget, deadlineMs, metadataCache, observationBudget, conditionalObservationBudget });
}
function primaryQuotaDelay(response, endpoint, asset, now, authenticated, repository) {
  const reset = response.headers.get('x-ratelimit-reset'), retry = response.headers.get('retry-after');
  if (!canonicalPublicAPI(endpoint, asset, repository) || ![403, 429].includes(response.status)
    || !(authenticated ? ['1000', '15000'] : ['60']).includes(response.headers.get('x-ratelimit-limit'))
    || response.headers.get('x-ratelimit-remaining') !== '0'
    || response.headers.get('x-ratelimit-resource') !== 'core' || !/^[0-9]+$/.test(reset ?? '')
    || (retry !== null && !/^[0-9]+$/.test(retry))) return null;
  const resetMs = Number(reset) * 1000;
  if (!Number.isSafeInteger(resetMs) || resetMs <= now) return null;
  const delay = Math.max(resetMs - now + 1000, retry === null ? 0 : Number(retry) * 1000);
  return Number.isSafeInteger(delay) && delay > 0 ? delay : null;
}
export async function publicBytes(url, limit, asset = false, options = {}) {
  const repository = options.repository ?? REPO;
  kernelRepositoryIdentity(repository);
  const request = options.fetch ?? fetch;
  const token = options.apiToken ?? releaseAPIToken(options.env ?? RELEASE_API_ENV);
  const original = new URL(url);
  const sourceMatch = original.origin === 'https://github.com' && !original.search && !original.hash
    ? new RegExp(`^/${repository}/archive/([a-f0-9]{40})\\.tar\\.gz$`).exec(original.pathname) : null;
  for (let redirects = 0; redirects <= 3;) {
    const endpoint = new URL(url);
    const headers = { 'User-Agent': 'Polaris-release-consumer', Accept: 'application/vnd.github+json' };
    const metadata = canonicalPublicAPI(endpoint, asset, repository), authenticated = metadata && token !== null;
    if (authenticated) headers.Authorization = `Bearer ${token}`;
    const cached = metadata ? options.metadataCache?.get(url) : null;
    if (cached) headers['If-None-Match'] = cached.etag;
    // Assets/CDN redirects remain anonymous; API redirects fail closed. Never
    // put a credential, response body, signed query or raw error in diagnostics.
    const stage = asset ? (redirects ? 'asset-redirect' : 'asset') : 'public-api';
    const location = `stage=${stage} host=${endpoint.hostname} path=${endpoint.pathname}`;
    const now = (options.now ?? Date.now)();
    const remaining = options.deadlineMs === undefined ? 60000 : options.deadlineMs - now;
    demand(remaining > 0, `Release consumer job deadline exceeded (${location})`);
    let response;
    try {
      response = await request(url, { redirect: 'manual', signal: AbortSignal.timeout(Math.min(60000, remaining)), headers });
    } catch (error) {
      const observation = transportObservation(error);
      if (options.transportBudget?.available && observation.retryable && canonicalPublicAPI(endpoint, asset, repository)
        && (options.now ?? Date.now)() + TRANSPORT_WAIT_MS < options.deadlineMs) {
        options.transportBudget.available = false;
        (options.report ?? console.error)(`Release transport retry ${TRANSPORT_WAIT_MS}ms (${location}; ${transportDetail(observation)})`);
        await (options.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms))))(TRANSPORT_WAIT_MS);
        continue;
      }
      throw new Error(`Release request failed (${location}; ${transportDetail(observation)})`);
    }
    const cancelBody = async () => {
      try { await response.body?.cancel(); }
      catch (error) { throw new Error(`Release body cancellation failed (${location}; ${transportDetail(transportObservation(error))})`); }
    };
    if (response.status === 304) {
      await cancelBody();
      const etag = response.headers.get('etag');
      const intact = Boolean(cached && cached.bytes.length === cached.size && digest(cached.bytes) === cached.sha256);
      const withinLimit = Boolean(cached && cached.size <= limit);
      const validator = etag === null ? 'absent' : !validETag(etag) ? 'invalid'
        : !cached || !matchingETag(etag, cached.etag) ? 'different'
          : etag === cached.etag ? 'exact' : 'weak-equivalent';
      // Categorical diagnostics never expose validators, credentials or bodies.
      demand(metadata && cached && ['absent', 'exact', 'weak-equivalent'].includes(validator)
        && intact && withinLimit,
      `Release conditional response has no intact matching metadata (${location}; status=304; cached=${Boolean(cached)}; validator=${validator}; intact=${intact}; withinLimit=${withinLimit})`);
      if (validator === 'weak-equivalent' && options.conditionalObservationBudget?.available) {
        options.conditionalObservationBudget.available = false;
        (options.report ?? console.error)(`Release conditional observation (${location}; status=304; validator=weak-equivalent; intact=true; withinLimit=true)`);
      }
      return Buffer.from(cached.bytes);
    }
    if ([301, 302, 303, 307, 308].includes(response.status)) {
      await cancelBody(); const next = new URL(response.headers.get('location') ?? '', url);
      const sourceRedirect = sourceMatch && redirects === 0 && next.hostname === 'codeload.github.com'
        && !next.search && !next.hash && next.pathname === `/${repository}/tar.gz/${sourceMatch[1]}`;
      demand(asset && next.protocol === 'https:' && !next.username && !next.password && !next.port
        && (sourceMatch ? sourceRedirect : ['release-assets.githubusercontent.com', 'objects.githubusercontent.com'].includes(next.hostname)),
      'Unapproved release redirect');
      url = next.href; redirects++; continue;
    }
    if (!response.ok) {
      await cancelBody();
      const quota = `apiAuth=${authenticated ? 'actions' : 'anonymous'}; ${quotaDetail(response, token)}`;
      const delay = options.quotaBudget?.available
        ? primaryQuotaDelay(response, endpoint, asset, (options.now ?? Date.now)(), authenticated, repository) : null;
      if (delay !== null && delay <= PRIMARY_QUOTA_WAIT_MS
        && (options.now ?? Date.now)() + delay < options.deadlineMs) {
        options.quotaBudget.available = false;
        (options.report ?? console.error)(`Release primary quota wait ${delay}ms (${location}; ${quota})`);
        await (options.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms))))(delay);
        continue;
      }
      const budget = delay !== null && delay > PRIMARY_QUOTA_WAIT_MS ? '; primary-quota-wait-budget-exceeded' : '';
      throw new Error(`Release HTTP ${response.status} (${location}; ${quota}${budget})`);
    }
    const parts = []; let size = 0;
    let oversized = false;
    try {
      for await (const part of response.body) {
        size += part.length;
        if (size > limit) { oversized = true; break; }
        parts.push(part);
      }
    } catch (error) {
      throw new Error(`Release body read failed (${location}; ${transportDetail(transportObservation(error))})`);
    }
    demand(!oversized, 'Release response exceeds pinned size');
    const bytes = Buffer.concat(parts);
    // Cache at most five metadata endpoints under the existing 8 MiB limit. The
    // second snapshot still performs fresh HTTP validation; no stale/disk cache.
    const etag = response.headers.get('etag');
    if (metadata && options.metadataCache) {
      options.metadataCache.delete(url);
      if (validETag(etag) && bytes.length <= 8 * 1024 * 1024 && options.metadataCache.size < 5)
        options.metadataCache.set(url, { etag, bytes: Buffer.from(bytes), size: bytes.length, sha256: digest(bytes) });
    }
    if (metadata && options.observationBudget?.available) {
      options.observationBudget.available = false;
      (options.report ?? console.error)(`Release API observation (${location}; status=${response.status}; apiAuth=${authenticated ? 'actions' : 'anonymous'}; ${quotaDetail(response, token)})`);
    }
    return bytes;
  }
  throw new Error('Release redirect limit');
}
async function publicSnapshot(contract, read) {
  const request = async (path) => JSON.parse(await read(`https://api.github.com/repos/${contract.repository}${path}`, 8 * 1024 * 1024, false));
  const repo = await request(''); demand(repo.id === releaseRepositoryId(contract) && repo.full_name === contract.repository && repo.private === false, 'Repository identity differs');
  const release = await request(`/releases/${contract.releaseId}`);
  demand(release.id === contract.releaseId && release.tag_name === contract.tag && release.draft === false
    && release.html_url === `https://github.com/${contract.repository}/releases/tag/${contract.tag}`, 'Published release identity differs');
  const ref = await request(`/git/ref/tags/${contract.tag}`);
  demand(ref.ref === `refs/tags/${contract.tag}` && ref.object?.type === 'tag' && ref.object.sha === contract.tagObject, 'Annotated release tag differs');
  const tag = await request(`/git/tags/${contract.tagObject}`);
  demand(tag.sha === contract.tagObject && tag.tag === contract.tag && tag.object?.type === 'commit'
    && tag.object.sha === contract.sourceCommit, 'Release tag source differs');
  const commit = await request(`/git/commits/${contract.sourceCommit}`);
  demand(commit.sha === contract.sourceCommit && commit.tree?.sha === contract.sourceTree, 'Published source tree differs');
  demand(Array.isArray(release.assets) && new Set(release.assets.map((a) => a.id)).size === release.assets.length
    && new Set(release.assets.map((a) => a.name)).size === release.assets.length, 'Duplicate public assets');
  const assets = contractPublicAssets(contract).map((pin) => {
    const rows = release.assets.filter((a) => a.name === pin.name);
    demand(rows.length === 1, 'Pinned public asset missing'); const asset = rows[0];
    demand(asset.id === pin.assetId && asset.size === pin.bytes && asset.state === 'uploaded'
      && asset.browser_download_url === pin.url && (asset.digest == null || asset.digest === `sha256:${pin.sha256}`), 'Public asset identity differs');
    return { name: asset.name, id: asset.id, bytes: asset.size, url: asset.browser_download_url, digest: asset.digest ?? null };
  });
  const upstream = isUpstreamRelease(contract) ? await readUpstreamSourceEvidence(contract, request) : undefined;
  return { repositoryId: repo.id, releaseId: release.id, tag: tag.sha, sourceCommit: commit.sha, sourceTree: commit.tree.sha, assets, ...upstream };
}
function indexRows(bytes, contract) {
  demand(bytes.length === contract.assetIndex.bytes && digest(bytes) === contract.assetIndex.sha256, 'Published asset index SHA differs');
  const index = JSON.parse(bytes);
  if (isUpstreamRelease(contract)) return validateUpstreamIndex(index, contract);
  demand(index.schema === 'polaris-production-assets-v1' && index.repository === contract.repository && Array.isArray(index.platforms), 'Published asset index schema differs');
  for (const name of ['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree']) same(index[name], contract[name], `Index ${name} differs`);
  same(index.platforms.map((r) => r.target).sort(), ['linux/amd64', 'linux/arm64', 'windows/amd64', 'windows/arm64', 'darwin/amd64', 'darwin/arm64'].sort(), 'Six CLI index targets differ');
  for (const row of Object.values(contract.platforms)) {
    const { releaseAsset, ...producer } = row;
    same(index.platforms.find((r) => r.target === row.target), producer, 'Public index producer row differs');
  }
  return index;
}

function coreHeader(bytes, target) {
  let ok = false;
  if (target.os === 'linux') ok = bytes.length >= 20 && bytes.subarray(0, 6).equals(Buffer.from([127, 69, 76, 70, 2, 1])) && bytes.readUInt16LE(18) === 62;
  else if (target.os === 'darwin') ok = bytes.length >= 32 && bytes.readUInt32LE(0) === 0xfeedfacf && bytes.readUInt32LE(4) === (target.arch === 'amd64' ? 0x1000007 : 0x100000c);
  else if (bytes.length >= 64 && bytes.toString('ascii', 0, 2) === 'MZ') {
    const offset = bytes.readUInt32LE(60);
    ok = offset + 6 <= bytes.length && bytes.toString('ascii', offset, offset + 4) === 'PE\0\0' && bytes.readUInt16LE(offset + 4) === 0x8664;
  }
  demand(ok, 'Actual release core architecture differs');
}
function actualBuildInfo(raw, upstream, goVersion) {
  const settings = {}, deps = [], replacements = [];
  demand(raw.split('\n')[0].endsWith(`: ${goVersion}`), 'Actual release Go version differs');
  demand(raw.includes('\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n')
    && raw.includes('\tmod\tgithub.com/sagernet/sing-box\t'), 'Actual Go module identity differs');
  for (const line of raw.split('\n')) {
    const parts = line.split('\t');
    if (parts[1] === 'build') {
      const at = parts[2].indexOf('='); demand(at > 0 && !Object.hasOwn(settings, parts[2].slice(0, at)), 'Duplicate/invalid BuildInfo setting');
      let value = parts[2].slice(at + 1);
      if (upstream && value.startsWith('\"')) value = JSON.parse(value);
      demand(typeof value === 'string', 'Actual upstream BuildInfo setting differs');
      settings[parts[2].slice(0, at)] = value;
    } else if (parts[1] === 'dep') deps.push({ module: parts[2], version: parts[3], h1: parts[4] ?? '' });
    else if (parts[1] === '=>') {
      if (upstream) {
        demand(deps.length > 0 && typeof parts[2] === 'string' && parts[2].length > 0 && typeof parts[3] === 'string', 'Actual upstream replacement differs');
        replacements.push({ ...deps.pop(), replacement: { module: parts[2], version: parts[3], h1: parts[4] ?? '' } });
        continue;
      }
      demand(deps.at(-1)?.module === 'github.com/sagernet/sing-tun' && parts[2] === './third_party/sing-tun'
        && parts[3] === '(devel)', 'Actual local replacement differs'); replacements.push(deps.pop());
    }
  }
  demand((upstream ? replacements.length > 0 : replacements.length === 1) && deps.length && deps.every((d) => /^h1:[A-Za-z0-9+/]+=$/.test(d.h1))
    && new Set([...deps, ...replacements].map((d) => d.module)).size === deps.length + replacements.length, 'Actual module graph incomplete');
  const acquisition = deps.filter((d) => d.module.startsWith('github.com/sagernet/cronet-go/lib/'));
  return { settings, normalModules: deps.filter((d) => !acquisition.includes(d)), prebuiltAcquisitionModules: acquisition, ...(upstream ? { localReplacements: replacements } : { localReplacement: replacements[0] }) };
}
export function verifyReleaseCore(path, row, key, run = execFileSync, host = process.platform,
  signatureRun = run === execFileSync ? undefined : run, contract = undefined) {
  const bytes = checkedBytes(path, row.core); const target = RELEASE_TARGETS[key]; coreHeader(bytes, target);
  const options = { encoding: 'utf8', env: { ...process.env, GOTOOLCHAIN: 'local', GOWORK: 'off' },
    maxBuffer: 8 * 1024 * 1024, timeout: LOCAL_METADATA_TIMEOUT_MS, killSignal: 'SIGKILL' };
  const raw = String(run('go', ['version', '-m', path], options)).trimEnd() + '\n';
  const upstream = isUpstreamRow(row);
  if (contract) demand(row.sourceCommit === contract.sourceCommit && row.sourceTree === contract.sourceTree,
    'Actual core/contract source pair differs');
  const goVersion = contract ? releaseGoVersion(contract) : upstream ? upstreamSourceGoVersion(row)
    : releaseGoVersion({ schema: 'polaris-box-release-consumer-contract-v1' });
  const graph = actualBuildInfo(raw, upstream, goVersion);
  if (upstream) {
    const normalized = (value) => { const lines = value.trimEnd().split(/\r?\n/);
      demand(lines[0].endsWith(row.goVersion), 'Original upstream BuildInfo Go version differs');
      return [row.goVersion, ...lines.slice(1)].join('\n'); };
    same(normalized(raw), normalized(row.rawBuildInfo), 'Actual upstream raw BuildInfo differs');
  }
  same(graph, row.buildGraph, 'Actual release BuildInfo/module graph differs');
  const settings = graph.settings;
  demand(settings.GOOS === target.os && settings.GOARCH === target.arch && settings.CGO_ENABLED === target.cgo
    && (upstream
      ? (!Object.hasOwn(settings, 'vcs.revision') || settings['vcs.revision'] === row.sourceCommit)
        && (!Object.hasOwn(settings, 'vcs.modified') || settings['vcs.modified'] === 'false')
      : settings['vcs.revision'] === row.sourceCommit && settings['vcs.modified'] === 'false')
    && settings['-trimpath'] === 'true' && (upstream || !Object.hasOwn(settings, '-ldflags')), 'Actual release build settings differ');
  same(settings['-tags']?.replaceAll('"', '').split(',').sort(), row.tags.split(',').sort(), 'Actual release feature tags differ');
  demand(String(run('go', ['tool', 'buildid', path], options)).trim() === row.buildID, 'Actual release BuildID differs');
  const signature = releaseMacCodeSignature(row, key);
  if (host === 'darwin' && signature) {
    same(observeMacCodeSignature(path, key, signatureRun), signature, 'Actual release Mac signature state/CDHash differs');
    checkedBytes(path, row.core); // Inspection must preserve the original downloaded bytes.
  }
  return graph;
}
export function verifyReleaseProvenance(bytes, row, key) {
  const target = RELEASE_TARGETS[key], rootName = `polaris-box-${row.version}-${target.os}-${target.arch}`;
  demand(digest(bytes) === row.archive.members[`${rootName}/provenance.json`], 'Inner provenance bytes differ');
  const { archive: nativeArchive, ...nativeProducer } = JSON.parse(bytes);
  const { archive, core, sidecar, releaseAsset, finalPackageCronetSmoke, ...producer } = row;
  // The producer's original receipt contains its first archive inventory. The
  // final assembly adds legal/source inputs and a final-package smoke record.
  // Both records retain their original identities; the pinned member hash above
  // binds the complete original receipt, including its earlier archive facts.
  same(nativeProducer, producer, 'Inner release provenance differs');
  demand(nativeArchive && equalKeys(nativeArchive, ['name', 'bytes', 'sha256', 'members'])
    && nativeArchive.name === archive.name && positive(nativeArchive.bytes)
    && nativeArchive.bytes <= archive.bytes && hex(nativeArchive.sha256, 64)
    && nativeArchive.members && Object.keys(nativeArchive.members).length > 0
    && Object.entries(nativeArchive.members).every(([name, hash]) => Object.hasOwn(archive.members, name) && hex(hash, 64))
    && nativeArchive.members[`${rootName}/${core.member}`] === core.sha256
    && (!sidecar || nativeArchive.members[`${rootName}/${sidecar.member}`] === sidecar.sha256),
  'Original native archive lineage differs');
  const smoke = finalPackageCronetSmoke;
  demand(smoke && smoke.os === target.os && smoke.arch === target.arch
    && smoke.go === row.sourceFingerprint.goVersion && smoke.gvisorCompiled === true
    && smoke.linkage === (sidecar ? 'shared-purego' : 'static-cgo')
    && /^\d+\.\d+\.\d+\.\d+$/.test(smoke.version ?? '')
    && (sidecar ? smoke.sha256 === sidecar.sha256
      && typeof smoke.library === 'string' && smoke.library.replaceAll('\\', '/').endsWith(`/${rootName}/${sidecar.member}`)
      : smoke.library === undefined && smoke.sha256 === undefined), 'Final package Cronet smoke binding differs');
  return nativeProducer;
}
function equalKeys(value, keys) { return canonical(Object.keys(value).sort()) === canonical([...keys].sort()); }
function verifyExtracted(directory, rootName, row, key, run, host, signatureRun, contract) {
  const root = join(directory, rootName);
  const { archive, core, sidecar } = row;
  if (!isUpstreamRow(row)) {
  verifyReleaseProvenance(readFileSync(join(root, 'provenance.json')), row, key);
  const sums = Object.keys(archive.members).filter((n) => n !== `${rootName}/SHA256SUMS`).sort()
    .map((n) => `${archive.members[n]}  ${n.slice(rootName.length + 1)}\n`).join('');
  demand(readFileSync(join(root, 'SHA256SUMS'), 'utf8') === sums, 'Inner checksum inventory differs');
  }
  verifyReleaseCore(join(root, core.member), row, key, run, host, signatureRun, contract);
  if (sidecar) inspectCronetBytes(checkedBytes(join(root, sidecar.member), sidecar), key);
  return root;
}
function directoryTree(path) {
  regularPath(path, true);
  for (const name of readdirSync(path)) {
    const child = join(path, name), st = lstatSync(child);
    if (st.isDirectory()) directoryTree(child); else regularPath(child);
  }
}
function directoryFingerprint(path) {
  directoryTree(path); const records = [];
  function walk(at, prefix = '') {
    records.push({ path: prefix, mode: lstatSync(at).mode & 0o777 });
    for (const name of readdirSync(at).sort()) {
      const child = join(at, name), relative = prefix + name, st = lstatSync(child);
      if (st.isDirectory()) walk(child, relative + '/');
      else records.push({ path: relative, mode: st.mode & 0o777, sha256: digest(readFileSync(child)) });
    }
  }
  walk(path); return digest(portableCanonical(records));
}
function saveJournal(lock, work, value) {
  const temporary = join(work, 'journal-next');
  const file = openSync(temporary, 'wx');
  try { writeFileSync(file, JSON.stringify(value) + '\n'); fsyncSync(file); } finally { closeSync(file); }
  renameSync(temporary, lock);
}
function processAlive(pid) {
  try { process.kill(pid, 0); return true; } catch (error) { return error.code !== 'ESRCH'; }
}
// Explicit recovery only. A live owner or changed resource tree is never removed.
export function recoverRelease(root, options = {}) {
  const resources = join(root, 'resources'), lock = join(resources, '.release-consumer.lock'); regularPath(lock);
  const journal = json(lock);
  demand(journal.schema === 'polaris-release-consumer-lock-v1' && positive(journal.pid)
    && !(options.isAlive ?? processAlive)(journal.pid), 'Release owner still active or recovery journal invalid');
  demand(typeof journal.work === 'string' && /^\.release-consumer-[A-Za-z0-9]+$/.test(journal.work), 'Recovery work path invalid');
  const work = join(resources, journal.work); regularPath(work, true);
  if (journal.phase !== 'acquiring') {
    demand(['replacing', 'committed'].includes(journal.phase) && Array.isArray(journal.directories)
      && journal.directories.length >= 2 && journal.directories.length <= 5
      && new Set(journal.directories.map((r) => r.key)).size === journal.directories.length
      && journal.directories.at(-1).key === '.release-receipts', 'Recovery directory inventory invalid');
    for (const row of journal.directories) {
      demand((Object.hasOwn(RELEASE_TARGETS, row.key) || row.key === '.release-receipts')
        && hex(row.staged, 64) && (row.original === null || hex(row.original, 64)), 'Recovery fingerprint invalid');
      const dest = join(resources, row.key), backup = join(work, `old-${row.key}`);
      if (journal.phase === 'committed') {
        demand(existsSync(dest) && directoryFingerprint(dest) === row.staged, 'Committed resources changed; recovery retained');
        if (existsSync(backup)) demand(row.original && directoryFingerprint(backup) === row.original, 'Original backup changed');
      } else if (existsSync(backup)) {
        demand(row.original && directoryFingerprint(backup) === row.original, 'Original backup changed');
        if (existsSync(dest)) demand(directoryFingerprint(dest) === row.staged, 'Partial resources changed; recovery retained');
      } else if (row.original) demand(existsSync(dest) && directoryFingerprint(dest) === row.original, 'Original resources unavailable');
      else if (existsSync(dest)) demand(directoryFingerprint(dest) === row.staged, 'New resources changed; recovery retained');
    }
    // Validate the entire recovery plan before restoring any directory.
    if (journal.phase === 'replacing') for (const row of [...journal.directories].reverse()) {
      const dest = join(resources, row.key), backup = join(work, `old-${row.key}`);
      if (existsSync(backup)) {
        if (existsSync(dest)) rmSync(dest, { recursive: true });
        (options.rename ?? renameSync)(backup, dest);
      } else if (row.original === null && existsSync(dest)) rmSync(dest, { recursive: true });
    }
  }
  rmSync(work, { recursive: true }); rmSync(lock);
  return { schema: 'polaris-release-consumer-recovery-v1', phase: journal.phase, restoredPreviousResources: journal.phase === 'replacing' };
}
function adapterIdentity(root, run) {
  const options = { cwd: root, encoding: 'utf8', env: { ...process.env },
    timeout: LOCAL_METADATA_TIMEOUT_MS, killSignal: 'SIGKILL' };
  for (const name of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE']) delete options.env[name];
  const candidate = String(run('git', ['rev-parse', 'HEAD'], options)).trim();
  demand(hex(candidate, 40) && !String(run('git', ['status', '--porcelain', '--untracked-files=no'], options)).trim(), 'Release adapter checkout differs');
  const files = Object.fromEntries(ADAPTER_FILES.map((name) => { regularPath(join(root, name)); return [name, digest(readFileSync(join(root, name)))]; }));
  const facts = { candidate, fileSha256: files };
  return { schema: 'polaris-app-release-adapter-v1', facts, fingerprint: digest(portableCanonical(facts)) };
}
function replaceDirectories(resources, work, keys, rename) {
  const completed = [];
  try {
    for (const key of [...keys, '.release-receipts']) {
      const dest = join(resources, key), backup = join(work, `old-${key}`), stage = join(work, 'stage', key);
      if (existsSync(dest)) { directoryTree(dest); rename(dest, backup); }
      completed.push({ dest, backup, installed: false });
      rename(stage, dest); completed.at(-1).installed = true;
    }
  } catch (error) {
    try {
      for (const { dest, backup, installed } of completed.reverse()) {
        if (installed) rmSync(dest, { recursive: true });
        if (existsSync(backup)) rename(backup, dest);
      }
    } catch (rollback) { throw new Error(`Release rollback failed; recovery retained at ${work}: ${rollback.message}`, { cause: error }); }
    throw error;
  }
}

export async function consumeRelease(root, manifest, keys = Object.keys(RELEASE_TARGETS), options = {}) {
  if (Object.hasOwn(manifest, 'desktopTemporary')) {
    const { consumeTemporaryDesktop } = await import('./native-dns-candidate.mjs');
    demand(keys.length && new Set(keys).size === keys.length && keys.every((key) => Object.hasOwn(RELEASE_TARGETS, key)), 'Unknown/duplicate App architecture selection');
    return consumeTemporaryDesktop(root, manifest, options);
  }
  demand(keys.length && new Set(keys).size === keys.length && keys.every((k) => Object.hasOwn(RELEASE_TARGETS, k)), 'Unknown/duplicate App architecture selection');
  const contract = loadReleaseContract(root, manifest), invoke = options.run ?? execFileSync, read = options.read ?? releasePublicReader({ repository: contract.repository });
  const withoutAPIToken = (execute) => (command, args, settings = {}) => {
    const env = { ...(settings.env ?? process.env) };
    delete env.POLARIS_RELEASE_API_TOKEN;
    return execute(command, args, { ...settings, env });
  };
  const run = withoutAPIToken(invoke);
  // codesign --display writes CDHash to stderr even on success. execFileSync
  // returns stdout only; preserve the observer's status/stdout/stderr contract.
  const signatureRun = invoke === execFileSync ? withoutAPIToken(spawnSync) : run;
  const adapter = adapterIdentity(root, run);
  const resources = join(root, 'resources'); mkdirSync(resources, { recursive: true }); regularPath(resources, true);
  const lock = join(resources, '.release-consumer.lock');
  // Exclusivity is the continuously present wx-created path, not an open
  // descriptor: journal replacement changes its inode already. Windows refuses
  // replacing an open destination, so release this handle without unlinking.
  closeSync(openSync(lock, 'wx'));
  let work, preserve = false, replaced = false;
  try {
    work = mkdtempSync(join(resources, '.release-consumer-'));
    const journal = { schema: 'polaris-release-consumer-lock-v1', pid: process.pid, work: basename(work), phase: 'acquiring' };
    saveJournal(lock, work, journal);
    const before = await publicSnapshot(contract, read);
    const index = hasCommittedReleaseMaterials(contract)
      ? checkedBytes(join(root, contract.assetIndex.path), contract.assetIndex)
      : await read(contract.assetIndex.url, contract.assetIndex.bytes, true);
    indexRows(index, contract);
    let sourceArchive;
    if (isUpstreamRelease(contract)) {
      sourceArchive = await read(contract.sourceArchive.url, contract.sourceArchive.bytes, true);
      demand(sourceArchive.length === contract.sourceArchive.bytes && digest(sourceArchive) === contract.sourceArchive.sha256, 'Downloaded complete source archive differs');
      // loadReleaseContract already checks legal content; send pins, then reprove tar bytes.
      const sourceFile = join(work, contract.sourceArchive.name); writeFileSync(sourceFile, sourceArchive, { flag: 'wx' });
      run(process.platform === 'win32' ? 'python' : 'python3', ['-B', join(dirname(fileURLToPath(import.meta.url)), 'release-archive.py'), sourceFile, '--source-legal-pins'],
        { input: JSON.stringify({ sourceArchive: contract.sourceArchive, sourceCommit: contract.sourceCommit,
          sourceTree: contract.sourceTree, sourceGitlinks: before.sourceGitlinks, legalFiles: contract.legalFiles.map(({ path, bytes, sha256 }) => ({ path, bytes, sha256 })), sourceIdentity: contract.platforms.linux.sourceIdentity,
          buildReplacements: Object.values(contract.platforms).flatMap((r) => r.buildGraph.localReplacements),
          ...(contract.sourcePolicy ? { sourcePolicy: contract.sourcePolicy } : {}) }), encoding: 'utf8', maxBuffer: 1024 * 1024 });
    }
    const stage = join(work, 'stage'); mkdirSync(stage);
    const metadata = join(stage, '.release-receipts'); mkdirSync(metadata);
    const oldMetadata = join(resources, '.release-receipts');
    if (existsSync(oldMetadata)) { directoryTree(oldMetadata); cpSync(oldMetadata, metadata, { recursive: true }); }
    const receipts = {};
    for (const key of keys) {
      const row = contract.platforms[key], target = RELEASE_TARGETS[key];
      const archive = join(work, row.archive.name);
      const bytes = await read(row.releaseAsset.url, row.releaseAsset.bytes, true);
      demand(bytes.length === row.releaseAsset.bytes && digest(bytes) === row.releaseAsset.sha256, 'Downloaded release archive SHA differs');
      writeFileSync(archive, bytes, { flag: 'wx' });
      const extracted = join(work, `unpacked-${key}`), rootName = validateRow(contract, key, row);
      run(process.platform === 'win32' ? 'python' : 'python3', ['-B', join(dirname(fileURLToPath(import.meta.url)), 'release-archive.py'), archive, extracted],
        { input: JSON.stringify({ archive: row.archive, root: rootName, target: row.target }), encoding: 'utf8', maxBuffer: 1024 * 1024 });
      const source = verifyExtracted(extracted, rootName, row, key, run, options.host ?? process.platform, signatureRun, contract);
      const dest = join(stage, key); mkdirSync(dest);
      const existing = join(resources, key);
      if (existsSync(existing)) { directoryTree(existing); cpSync(existing, dest, { recursive: true }); }
      demand(!readdirSync(dest).some((name) => /^libcronet\.(so|dll|dylib)$/.test(name) && name !== target.sidecar), 'Existing resource contains a different Cronet sidecar');
      for (const name of [target.core, target.sidecar].filter(Boolean)) { copyFileSync(join(source, name), join(dest, name)); chmodSync(join(dest, name), lstatSync(join(source, name)).mode & 0o777); }
      const notices = join(dest, 'core-release'); rmSync(notices, { recursive: true, force: true }); mkdirSync(notices);
      for (const name of readdirSync(source).filter((n) => n !== target.core && n !== target.sidecar)) cpSync(join(source, name), join(notices, name), { recursive: true });
      if (isUpstreamRelease(contract)) {
        const metadata = { ...upstreamMetadata(contract, row), [`source/${contract.sourceArchive.name}`]: sourceArchive };
        for (const [name, content] of Object.entries(metadata)) {
          const path = join(notices, name); mkdirSync(dirname(path), { recursive: true });
          writeFileSync(path, content, { flag: 'wx', mode: 0o644 });
        }
      }
      const facts = { platform: key, contractSha256: manifest.desktopRelease.contractSha256,
        assetIndexSha256: contract.assetIndex.sha256, releaseAsset: row.releaseAsset, adapter, producer: row,
        publicReadback: before,
        signaturePolicy: POLICY, macCodeSignature: releaseMacCodeSignature(row, key),
        actualAppAcceptance: 'pending native load, gvisor/mixed, feature and final carrier gates' };
      const receipt = { schema: 'polaris-app-release-consumption-v1', ...facts, fingerprint: digest(portableCanonical(facts)) };
      writeFileSync(join(metadata, `${key}.json`), JSON.stringify(receipt, null, 2) + '\n', { mode: 0o600 }); receipts[key] = receipt;
    }
    const after = await publicSnapshot(contract, read); same(after, before, 'Release identity changed during acquisition');
    writeFileSync(join(metadata, 'public-readback.json'), JSON.stringify({ schema: 'polaris-app-release-readback-v1', contractSha256: manifest.desktopRelease.contractSha256, before, after }, null, 2) + '\n', { mode: 0o600 });
    writeFileSync(join(metadata, contract.assetIndex.name), index, { mode: 0o600 });
    journal.phase = 'replacing'; journal.directories = [...keys, '.release-receipts'].map((key) => ({ key,
      original: existsSync(join(resources, key)) ? directoryFingerprint(join(resources, key)) : null,
      staged: directoryFingerprint(join(stage, key)) }));
    saveJournal(lock, work, journal);
    try {
      replaceDirectories(resources, work, keys, options.rename ?? renameSync); replaced = true;
      journal.phase = 'committed'; saveJournal(lock, work, journal);
    } catch (e) { if (replaced || e.message.startsWith('Release rollback failed;')) preserve = true; throw e; }
    return receipts;
  } finally {
    if (!preserve) rmSync(lock);
    if (work && !preserve) rmSync(work, { recursive: true, force: true });
  }
}

export function verifyPackagedRelease(root, manifest, key, core, run = execFileSync, host = process.platform) {
  demand(Object.hasOwn(RELEASE_TARGETS, key), 'Unknown App target');
  demand(!existsSync(join(root, 'resources/.release-consumer.lock')), 'Release acquisition/recovery still pending');
  const contract = loadReleaseContract(root, manifest), row = contract.platforms[key];
  const metadata = join(root, 'resources/.release-receipts'), file = join(metadata, `${key}.json`); regularPath(file);
  const receipt = json(file); const { fingerprint, schema, ...facts } = receipt;
  demand(schema === 'polaris-app-release-consumption-v1' && fingerprint === digest(portableCanonical(facts))
    && receipt.platform === key && receipt.contractSha256 === manifest.desktopRelease.contractSha256
    && receipt.assetIndexSha256 === contract.assetIndex.sha256 && receipt.signaturePolicy === POLICY, 'Release consumption receipt differs');
  same(receipt.producer, row, 'Saved original producer differs'); same(receipt.releaseAsset, row.releaseAsset, 'Saved asset identity differs');
  same(receipt.macCodeSignature, releaseMacCodeSignature(row, key), 'Saved Mac signature state/CDHash differs');
  same(receipt.adapter, adapterIdentity(root, run), 'App adapter identity differs');
  indexRows(checkedBytes(join(metadata, contract.assetIndex.name), contract.assetIndex), contract);
  regularPath(join(metadata, 'public-readback.json'));
  const readback = json(join(metadata, 'public-readback.json'));
  demand(readback.schema === 'polaris-app-release-readback-v1' && readback.contractSha256 === receipt.contractSha256, 'Saved public readback differs');
  same(readback.before, readback.after, 'Saved readback snapshots differ');
  same(readback.before, receipt.publicReadback, 'Consumption public snapshot differs');
  const snapshot = receipt.publicReadback;
  validatePublishedUpstreamReadback(snapshot, contract);
  demand(snapshot?.repositoryId === releaseRepositoryId(contract) && snapshot.releaseId === contract.releaseId
    && snapshot.tag === contract.tagObject && snapshot.sourceCommit === contract.sourceCommit
    && snapshot.sourceTree === contract.sourceTree, 'Saved readback identity differs');
  same(snapshot.assets?.map(({ name, id, bytes, url }) => ({ name, id, bytes, url })),
    contractPublicAssets(contract)
      .map((p) => ({ name: p.name, id: p.assetId, bytes: p.bytes, url: p.url })), 'Saved public assets differ');
  // Both source and final carrier must retain the producer's bytes and permissions.
  const source = join(root, 'resources', key, row.core.member); checkedBytes(source, row.core);
  demand(((lstatSync(core).mode | lstatSync(source).mode) & 0o7000) === 0
      && (lstatSync(core).mode & 0o7777) === (lstatSync(source).mode & 0o7777), 'Final release core permissions differ');
  verifyReleaseCore(core, row, key, run, host, undefined, contract);
  if (row.sidecar) {
    const lib = join(dirname(core), row.sidecar.member), original = join(root, 'resources', key, row.sidecar.member);
    checkedBytes(original, row.sidecar); inspectCronetBytes(checkedBytes(lib, row.sidecar), key);
    demand(((lstatSync(lib).mode | lstatSync(original).mode) & 0o7000) === 0
      && (lstatSync(lib).mode & 0o7777) === (lstatSync(original).mode & 0o7777), 'Final release library permissions differ');
  } else demand(!readdirSync(dirname(core)).some((n) => /^libcronet\.(so|dll|dylib)$/.test(n)), 'Static carrier contains Cronet sidecar');
  const prefix = validateRow(contract, key, row) + '/';
  const payloadPins = isUpstreamRelease(contract) ? upstreamPayloadPins(contract, row)
    : Object.fromEntries(Object.entries(row.archive.members).map(([name, hash]) => [name.slice(prefix.length), hash]));
  for (const [relative, sha256] of Object.entries(payloadPins)) {
    if (relative === row.core.member || relative === row.sidecar?.member) continue;
    const original = join(dirname(source), 'core-release', relative), final = join(dirname(core), 'core-release', relative);
    regularPath(original); regularPath(final);
    demand(digest(readFileSync(original)) === sha256 && digest(readFileSync(final)) === sha256,
      'Release notice/source/provenance bytes differ in final carrier');
    demand(((lstatSync(original).mode | lstatSync(final).mode) & 0o7000) === 0
      && (lstatSync(original).mode & 0o7777) === (lstatSync(final).mode & 0o7777), 'Final release notice permissions differ');
  }
  if (isUpstreamRelease(contract)) {
    // The validated contract binds legal content to these pins; Python still checks actual tar bytes.
    const started = performance.now();
    try {
      run(process.platform === 'win32' ? 'python' : 'python3',
        ['-B', join(dirname(fileURLToPath(import.meta.url)), 'release-archive.py'), join(dirname(core), 'core-release/source', contract.sourceArchive.name), '--source-legal-pins'],
        { input: JSON.stringify({ sourceArchive: contract.sourceArchive, sourceCommit: contract.sourceCommit,
          sourceTree: contract.sourceTree, sourceGitlinks: snapshot.sourceGitlinks, legalFiles: contract.legalFiles.map(({ path, bytes, sha256 }) => ({ path, bytes, sha256 })), sourceIdentity: row.sourceIdentity,
          buildReplacements: row.buildGraph.localReplacements, ...(contract.sourcePolicy ? { sourcePolicy: contract.sourcePolicy } : {}) }),
          encoding: 'utf8', maxBuffer: 1024 * 1024, timeout: LOCAL_ARCHIVE_TIMEOUT_MS, killSignal: 'SIGKILL' });
    } catch (error) {
      // Identify the exact local proof that failed without logging its input,
      // changing the verification budget, retrying, or accepting older evidence.
      const elapsedMs = Math.round(performance.now() - started);
      throw new Error(`Release proof failed (platform=${key}; archive=${contract.sourceArchive.name}; stage=source-archive-reproof; elapsedMs=${elapsedMs}; timeoutMs=${LOCAL_ARCHIVE_TIMEOUT_MS}; code=${error?.code ?? 'unknown'}; status=${error?.status ?? 'unknown'}; signal=${error?.signal ?? 'unknown'}): ${error?.message ?? error}`, { cause: error });
    }
  }
  return receipt;
}

// Final Mac bundle validation selects the explicit release contract before any
// legacy source receipt. It never aliases/relabels the two receipt schemas.
export function verifyMacPackagedCore(root, manifest, key, binary, run = execFileSync) {
  demand(key === 'mac-x64' || key === 'mac-arm64', 'Mac packaged core requires a Mac target');
  if (hasTemporaryDesktop(manifest)) { loadTemporaryDesktopSelection(root, manifest); return verifyCandidatePayload(root, key, binary, run); }
  if (hasDesktopRelease(manifest)) return verifyPackagedRelease(root, manifest, key, binary, run, 'darwin');
  const path = join(root, 'resources/.source-receipts', `${key}.json`); regularPath(path);
  const receipt = json(path); verifyMacCodeSignatureReceipt(binary, receipt, key, run === execFileSync ? undefined : run);
  return receipt;
}
