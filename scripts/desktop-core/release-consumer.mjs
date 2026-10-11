// Explicit release acquisition for build resources. Never executes target code.
import { execFileSync } from 'node:child_process';
import { chmodSync, closeSync, copyFileSync, cpSync, existsSync, fsyncSync, lstatSync, mkdirSync, mkdtempSync,
  openSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonical, digest, requireGraph as demand } from './source-graph.mjs';
import { inspectCronetBytes } from '../lib/cronet-payload.mjs';

const REPO = 'polaris-arch/polaris-box';
const API = `https://api.github.com/repos/${REPO}`;
const POLICY = 'preserve measured original bytes; ad-hoc/not-signed observations do not assert trust';
const CONTRACT_PATH = 'scripts/desktop-core/release-contract.json';
const PROVIDER_FILES = ['scripts/polaris_production_release.py', 'scripts/polaris_native_candidate.py',
  'scripts/polaris_source_fingerprint.py', 'scripts/polaris_dns_identity.py', 'scripts/polaris_go_environment.py',
  'scripts/polaris_native_distribution.py', 'release/polaris-production-release.json', 'release/polaris-candidate-inputs.json'];
export const RELEASE_TARGETS = Object.freeze({
  linux: { os: 'linux', arch: 'amd64', core: 'sing-box', sidecar: 'libcronet.so', suffix: '.tar.gz', cgo: '0' },
  win: { os: 'windows', arch: 'amd64', core: 'sing-box.exe', sidecar: 'libcronet.dll', suffix: '.zip', cgo: '0' },
  'mac-x64': { os: 'darwin', arch: 'amd64', core: 'sing-box', sidecar: null, suffix: '.tar.gz', cgo: '1' },
  'mac-arm64': { os: 'darwin', arch: 'arm64', core: 'sing-box', sidecar: null, suffix: '.tar.gz', cgo: '1' },
});
export const ADAPTER_FILES = ['scripts/desktop-core/release-consumer.mjs', 'scripts/desktop-core/release-archive.py',
  'scripts/fetch-core.mjs', 'scripts/fetch-cronet.mjs', 'scripts/desktop-core/bundle.mjs',
  'scripts/lib/cronet-payload.mjs', 'scripts/native-payload-input.mjs', 'scripts/verify-packaging.mjs',
  'crates/updater/src/core_release.rs', 'crates/updater/src/lib.rs', 'src-tauri/src/runtime/updater.rs',
  'scripts/desktop-core/source-graph.mjs', 'scripts/lib/cronet-contract.mjs', 'scripts/assert-pc-runtime-release.mjs'];
const hex = (v, n) => typeof v === 'string' && new RegExp(`^[a-f0-9]{${n}}$`).test(v);
const positive = (v) => Number.isSafeInteger(v) && v > 0;
const same = (a, b, message) => demand(canonical(a) === canonical(b), message);
// Match producer Python json.dumps(sort_keys=True, separators=(',', ':'), ensure_ascii=True).
export const portableCanonical = (v) => canonical(v).replace(/[\u007f-\uffff]/g,
  (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`);
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
    && pin.url === `https://github.com/${REPO}/releases/download/${contract.tag}/${name}`, 'Release asset pin/URL differs');
}
function identity(value, schema, facts) {
  demand(value?.schema === schema && hex(value.fingerprint, 64), 'Portable identity shape differs');
  same(value.facts, facts, 'Portable identity facts differ');
  demand(value.fingerprint === digest(portableCanonical(facts)), 'Portable identity fingerprint differs');
}
function validateRow(contract, key, row) {
  const target = RELEASE_TARGETS[key];
  const root = `polaris-box-${contract.version}-${target.os}-${target.arch}`;
  demand(row?.schema === 'polaris-production-native-receipt-v1' && row.target === `${target.os}/${target.arch}`
    && row.version === contract.version && row.tag === contract.tag && row.sourceCommit === contract.sourceCommit
    && row.sourceTree === contract.sourceTree && row.providerContract === 'polaris-box-release-v1'
    && row.signaturePolicy === POLICY && row.signatureObservation?.trustValidated === false,
  'Producer row/source/signature policy differs');
  demand(row.sourceFingerprint?.schema === 'fork-source-fingerprint-v1', 'Original source collector missing');
  const facts = Object.fromEntries(['sourceCommit', 'sourceTree', 'sourceOverlay', 'goVersion',
    'goModSha256', 'goSumSha256', 'replacement'].map((k) => [k, row.sourceFingerprint[k]]));
  demand(facts.sourceCommit === contract.sourceCommit && facts.sourceTree === contract.sourceTree
    && facts.sourceOverlay === 0 && facts.goVersion === 'go1.25.5' && hex(facts.goModSha256, 64)
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
  demand(contract?.schema === 'polaris-box-release-consumer-contract-v1'
    && contract.providerContract === 'polaris-box-release-v1' && contract.repository === REPO
    && positive(contract.releaseId) && hex(contract.sourceCommit, 40) && hex(contract.sourceTree, 40)
    && hex(contract.tagObject, 40) && /^1\.15\.0-alpha\.12\.polaris\.[1-9][0-9]*$/.test(contract.version)
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

export function loadReleaseContract(root, manifest) {
  const selection = manifest.desktopRelease;
  demand(selection?.schema === 'polaris-app-release-selection-v1' && selection.contractPath === CONTRACT_PATH
    && hex(selection.contractSha256, 64), 'Explicit release selection incomplete');
  const file = join(root, CONTRACT_PATH); regularPath(file);
  const bytes = readFileSync(file); demand(bytes.length <= 8 * 1024 * 1024 && digest(bytes) === selection.contractSha256, 'Reviewed consumer contract SHA differs');
  const contract = validateReleaseContract(JSON.parse(bytes));
  for (const key of ['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId'])
    same(selection[key], contract[key], `Compiled release selection ${key} differs`);
  same(selection.librarySha256, { linux: contract.platforms.linux.sidecar.sha256,
    win: contract.platforms.win.sidecar.sha256 }, 'Compiled same-package library pins differ');
  return contract;
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
  const desktop = section('桌面 sing-box');
  demand(desktop[0] === `- 桌面 sing-box v1.15.0-alpha.12 (GPLv3) — https://github.com/${REPO}`, 'NOTICE: release baseline differs');
  for (const text of [`上游基线源码：https://github.com/SagerNet/sing-box/tree/v1.15.0-alpha.12`,
    `桌面发布构建：${contract.version}`, `固定源码：https://github.com/${REPO}/tree/${contract.sourceCommit}`,
    `源码 tree：${contract.sourceTree}`, `固定消费标签：https://github.com/${REPO}/tree/${contract.tag}`,
    `消费标签对象：${contract.tagObject}`, `消费合同 SHA-256：${manifest.desktopRelease.contractSha256}`,
    `公开资产 index SHA-256：${contract.assetIndex.sha256}`, `桌面发布：https://github.com/${REPO}/releases/tag/${contract.tag}`]) exact(desktop, text);
  for (const [label, source] of [['Android libbox 基线', mobile.android], ['Apple libbox 基线', mobile.apple]]) {
    demand(hex(source?.sourceCommit, 40) && hex(source?.manifestSha256, 64), 'NOTICE: mobile source identity missing');
    const lines = section(label);
    demand(lines[0] === `- ${label} v${manifest.bundledCoreVersion} (GPLv3) — https://github.com/SagerNet/sing-box`, 'NOTICE: mobile baseline differs');
    for (const text of [`上游基线源码：https://github.com/SagerNet/sing-box/tree/v${manifest.bundledCoreVersion}`,
      `固定源码：https://github.com/SagerNet/sing-box/tree/${source.sourceCommit}`, `source manifest SHA-256：${source.manifestSha256}`]) exact(lines, text);
  }
}

// Redirects are permitted only for immutable asset bytes, whose expected hash is already pinned.
export async function publicBytes(url, limit, asset = false) {
  for (let redirects = 0; redirects <= 3; redirects++) {
    const response = await fetch(url, { redirect: 'manual', signal: AbortSignal.timeout(60000),
      headers: { 'User-Agent': 'Polaris-release-consumer', Accept: 'application/vnd.github+json' } });
    if ([301, 302, 303, 307, 308].includes(response.status)) {
      await response.body?.cancel(); const next = new URL(response.headers.get('location') ?? '', url);
      demand(asset && next.protocol === 'https:' && !next.username && !next.password && !next.port
        && ['release-assets.githubusercontent.com', 'objects.githubusercontent.com'].includes(next.hostname), 'Unapproved release redirect');
      url = next.href; continue;
    }
    demand(response.ok, `Release HTTP ${response.status}`);
    const parts = []; let size = 0;
    for await (const part of response.body) {
      size += part.length;
      if (size > limit) throw new Error('Release response exceeds pinned size');
      parts.push(part);
    }
    return Buffer.concat(parts);
  }
  throw new Error('Release redirect limit');
}
async function publicSnapshot(contract, read) {
  const request = async (path) => JSON.parse(await read(`${API}${path}`, 8 * 1024 * 1024, false));
  const repo = await request(''); demand(repo.id === 1407705262 && repo.full_name === REPO, 'Repository identity differs');
  const release = await request(`/releases/${contract.releaseId}`);
  demand(release.id === contract.releaseId && release.tag_name === contract.tag && release.draft === false
    && release.html_url === `https://github.com/${REPO}/releases/tag/${contract.tag}`, 'Published release identity differs');
  const ref = await request(`/git/ref/tags/${contract.tag}`);
  demand(ref.ref === `refs/tags/${contract.tag}` && ref.object?.type === 'tag' && ref.object.sha === contract.tagObject, 'Annotated release tag differs');
  const tag = await request(`/git/tags/${contract.tagObject}`);
  demand(tag.sha === contract.tagObject && tag.tag === contract.tag && tag.object?.type === 'commit'
    && tag.object.sha === contract.sourceCommit, 'Release tag source differs');
  const commit = await request(`/git/commits/${contract.sourceCommit}`);
  demand(commit.sha === contract.sourceCommit && commit.tree?.sha === contract.sourceTree, 'Published source tree differs');
  demand(Array.isArray(release.assets) && new Set(release.assets.map((a) => a.id)).size === release.assets.length
    && new Set(release.assets.map((a) => a.name)).size === release.assets.length, 'Duplicate public assets');
  const assets = [contract.assetIndex, ...Object.values(contract.platforms).map((r) => r.releaseAsset)].map((pin) => {
    const rows = release.assets.filter((a) => a.name === pin.name);
    demand(rows.length === 1, 'Pinned public asset missing'); const asset = rows[0];
    demand(asset.id === pin.assetId && asset.size === pin.bytes && asset.state === 'uploaded'
      && asset.browser_download_url === pin.url && (asset.digest == null || asset.digest === `sha256:${pin.sha256}`), 'Public asset identity differs');
    return { name: asset.name, id: asset.id, bytes: asset.size, url: asset.browser_download_url, digest: asset.digest ?? null };
  });
  return { repositoryId: repo.id, releaseId: release.id, tag: tag.sha, sourceCommit: commit.sha, sourceTree: commit.tree.sha, assets };
}
function indexRows(bytes, contract) {
  demand(bytes.length === contract.assetIndex.bytes && digest(bytes) === contract.assetIndex.sha256, 'Published asset index SHA differs');
  const index = JSON.parse(bytes);
  demand(index.schema === 'polaris-production-assets-v1' && index.repository === REPO && Array.isArray(index.platforms), 'Published asset index schema differs');
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
function actualBuildInfo(raw) {
  const settings = {}, deps = [], replacements = [];
  demand(raw.split('\n')[0].endsWith('go1.25.5'), 'Actual release Go version differs');
  demand(raw.includes('\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n')
    && raw.includes('\tmod\tgithub.com/sagernet/sing-box\t'), 'Actual Go module identity differs');
  for (const line of raw.split('\n')) {
    const parts = line.split('\t');
    if (parts[1] === 'build') {
      const at = parts[2].indexOf('='); demand(at > 0 && !Object.hasOwn(settings, parts[2].slice(0, at)), 'Duplicate/invalid BuildInfo setting');
      settings[parts[2].slice(0, at)] = parts[2].slice(at + 1);
    } else if (parts[1] === 'dep') deps.push({ module: parts[2], version: parts[3], h1: parts[4] ?? '' });
    else if (parts[1] === '=>') {
      demand(deps.at(-1)?.module === 'github.com/sagernet/sing-tun' && parts[2] === './third_party/sing-tun'
        && parts[3] === '(devel)', 'Actual local replacement differs'); replacements.push(deps.pop());
    }
  }
  demand(replacements.length === 1 && deps.length && deps.every((d) => /^h1:[A-Za-z0-9+/]+=$/.test(d.h1))
    && new Set([...deps, ...replacements].map((d) => d.module)).size === deps.length + replacements.length, 'Actual module graph incomplete');
  const acquisition = deps.filter((d) => d.module.startsWith('github.com/sagernet/cronet-go/lib/'));
  return { settings, normalModules: deps.filter((d) => !acquisition.includes(d)), prebuiltAcquisitionModules: acquisition, localReplacement: replacements[0] };
}
export function verifyReleaseCore(path, row, key, run = execFileSync) {
  const bytes = checkedBytes(path, row.core); const target = RELEASE_TARGETS[key]; coreHeader(bytes, target);
  const options = { encoding: 'utf8', env: { ...process.env, GOTOOLCHAIN: 'local', GOWORK: 'off' }, maxBuffer: 8 * 1024 * 1024 };
  const graph = actualBuildInfo(String(run('go', ['version', '-m', path], options)).trimEnd() + '\n');
  same(graph, row.buildGraph, 'Actual release BuildInfo/module graph differs');
  const settings = graph.settings;
  demand(settings.GOOS === target.os && settings.GOARCH === target.arch && settings.CGO_ENABLED === target.cgo
    && settings['vcs.revision'] === row.sourceCommit && settings['vcs.modified'] === 'false'
    && settings['-trimpath'] === 'true' && !Object.hasOwn(settings, '-ldflags'), 'Actual release build settings differ');
  same(settings['-tags']?.replaceAll('"', '').split(',').sort(), row.tags.split(',').sort(), 'Actual release feature tags differ');
  demand(String(run('go', ['tool', 'buildid', path], options)).trim() === row.buildID, 'Actual release BuildID differs');
  return graph;
}
function verifyExtracted(directory, rootName, row, key, run) {
  const root = join(directory, rootName);
  const { archive, core, sidecar, releaseAsset, ...producer } = row;
  same(json(join(root, 'provenance.json')), producer, 'Inner release provenance differs');
  const sums = Object.keys(archive.members).filter((n) => n !== `${rootName}/SHA256SUMS`).sort()
    .map((n) => `${archive.members[n]}  ${n.slice(rootName.length + 1)}\n`).join('');
  demand(readFileSync(join(root, 'SHA256SUMS'), 'utf8') === sums, 'Inner checksum inventory differs');
  verifyReleaseCore(join(root, core.member), row, key, run);
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
  const options = { cwd: root, encoding: 'utf8', env: { ...process.env } };
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
  demand(keys.length && new Set(keys).size === keys.length && keys.every((k) => Object.hasOwn(RELEASE_TARGETS, k)), 'Unknown/duplicate App architecture selection');
  const contract = loadReleaseContract(root, manifest), run = options.run ?? execFileSync, read = options.read ?? publicBytes;
  const adapter = adapterIdentity(root, run);
  const resources = join(root, 'resources'); mkdirSync(resources, { recursive: true }); regularPath(resources, true);
  const lock = join(resources, '.release-consumer.lock'); const fd = openSync(lock, 'wx');
  let work, preserve = false, replaced = false;
  try {
    work = mkdtempSync(join(resources, '.release-consumer-'));
    const journal = { schema: 'polaris-release-consumer-lock-v1', pid: process.pid, work: basename(work), phase: 'acquiring' };
    saveJournal(lock, work, journal);
    const before = await publicSnapshot(contract, read);
    const index = await read(contract.assetIndex.url, contract.assetIndex.bytes, true); indexRows(index, contract);
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
        { input: JSON.stringify({ archive: row.archive, root: rootName }), encoding: 'utf8', maxBuffer: 1024 * 1024 });
      const source = verifyExtracted(extracted, rootName, row, key, run);
      const dest = join(stage, key); mkdirSync(dest);
      const existing = join(resources, key);
      if (existsSync(existing)) { directoryTree(existing); cpSync(existing, dest, { recursive: true }); }
      demand(!readdirSync(dest).some((name) => /^libcronet\.(so|dll|dylib)$/.test(name) && name !== target.sidecar), 'Existing resource contains a different Cronet sidecar');
      for (const name of [target.core, target.sidecar].filter(Boolean)) { copyFileSync(join(source, name), join(dest, name)); chmodSync(join(dest, name), lstatSync(join(source, name)).mode & 0o777); }
      const notices = join(dest, 'core-release'); rmSync(notices, { recursive: true, force: true }); mkdirSync(notices);
      for (const name of readdirSync(source).filter((n) => n !== target.core && n !== target.sidecar)) cpSync(join(source, name), join(notices, name), { recursive: true });
      const facts = { platform: key, contractSha256: manifest.desktopRelease.contractSha256,
        assetIndexSha256: contract.assetIndex.sha256, releaseAsset: row.releaseAsset, adapter, producer: row,
        publicReadback: before,
        signaturePolicy: POLICY, actualAppAcceptance: 'pending native load, gvisor/mixed, feature and final carrier gates' };
      const receipt = { schema: 'polaris-app-release-consumption-v1', ...facts, fingerprint: digest(portableCanonical(facts)) };
      writeFileSync(join(metadata, `${key}.json`), JSON.stringify(receipt, null, 2) + '\n'); receipts[key] = receipt;
    }
    const after = await publicSnapshot(contract, read); same(after, before, 'Release identity changed during acquisition');
    writeFileSync(join(metadata, 'public-readback.json'), JSON.stringify({ schema: 'polaris-app-release-readback-v1', contractSha256: manifest.desktopRelease.contractSha256, before, after }, null, 2) + '\n');
    writeFileSync(join(metadata, 'polaris-box-assets.json'), index);
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
    closeSync(fd);
    if (!preserve) rmSync(lock);
    if (work && !preserve) rmSync(work, { recursive: true, force: true });
  }
}

export function verifyPackagedRelease(root, manifest, key, core, run = execFileSync) {
  demand(Object.hasOwn(RELEASE_TARGETS, key), 'Unknown App target');
  demand(!existsSync(join(root, 'resources/.release-consumer.lock')), 'Release acquisition/recovery still pending');
  const contract = loadReleaseContract(root, manifest), row = contract.platforms[key];
  const metadata = join(root, 'resources/.release-receipts'), file = join(metadata, `${key}.json`); regularPath(file);
  const receipt = json(file); const { fingerprint, schema, ...facts } = receipt;
  demand(schema === 'polaris-app-release-consumption-v1' && fingerprint === digest(portableCanonical(facts))
    && receipt.platform === key && receipt.contractSha256 === manifest.desktopRelease.contractSha256
    && receipt.assetIndexSha256 === contract.assetIndex.sha256 && receipt.signaturePolicy === POLICY, 'Release consumption receipt differs');
  same(receipt.producer, row, 'Saved original producer differs'); same(receipt.releaseAsset, row.releaseAsset, 'Saved asset identity differs');
  same(receipt.adapter, adapterIdentity(root, run), 'App adapter identity differs');
  indexRows(checkedBytes(join(metadata, 'polaris-box-assets.json'), contract.assetIndex), contract);
  regularPath(join(metadata, 'public-readback.json'));
  const readback = json(join(metadata, 'public-readback.json'));
  demand(readback.schema === 'polaris-app-release-readback-v1' && readback.contractSha256 === receipt.contractSha256, 'Saved public readback differs');
  same(readback.before, readback.after, 'Saved readback snapshots differ');
  same(readback.before, receipt.publicReadback, 'Consumption public snapshot differs');
  const snapshot = receipt.publicReadback;
  demand(snapshot?.repositoryId === 1407705262 && snapshot.releaseId === contract.releaseId
    && snapshot.tag === contract.tagObject && snapshot.sourceCommit === contract.sourceCommit
    && snapshot.sourceTree === contract.sourceTree, 'Saved readback identity differs');
  same(snapshot.assets?.map(({ name, id, bytes, url }) => ({ name, id, bytes, url })),
    [contract.assetIndex, ...Object.values(contract.platforms).map((r) => r.releaseAsset)]
      .map((p) => ({ name: p.name, id: p.assetId, bytes: p.bytes, url: p.url })), 'Saved public assets differ');
  // Both source and final carrier must retain the producer's bytes and permissions.
  const source = join(root, 'resources', key, row.core.member); checkedBytes(source, row.core); verifyReleaseCore(core, row, key, run);
  demand((lstatSync(core).mode & 0o777) === (lstatSync(source).mode & 0o777), 'Final release core permissions differ');
  if (row.sidecar) {
    const lib = join(dirname(core), row.sidecar.member), original = join(root, 'resources', key, row.sidecar.member);
    checkedBytes(original, row.sidecar); inspectCronetBytes(checkedBytes(lib, row.sidecar), key);
    demand((lstatSync(lib).mode & 0o777) === (lstatSync(original).mode & 0o777), 'Final release library permissions differ');
  } else demand(!readdirSync(dirname(core)).some((n) => /^libcronet\.(so|dll|dylib)$/.test(n)), 'Static carrier contains Cronet sidecar');
  const prefix = `polaris-box-${contract.version}-${RELEASE_TARGETS[key].os}-${RELEASE_TARGETS[key].arch}/`;
  for (const [name, sha256] of Object.entries(row.archive.members)) {
    const relative = name.slice(prefix.length);
    if (relative === row.core.member || relative === row.sidecar?.member) continue;
    const original = join(dirname(source), 'core-release', relative), final = join(dirname(core), 'core-release', relative);
    regularPath(original); regularPath(final);
    demand(digest(readFileSync(original)) === sha256 && digest(readFileSync(final)) === sha256,
      'Release notice/source/provenance bytes differ in final carrier');
    demand((lstatSync(original).mode & 0o777) === (lstatSync(final).mode & 0o777), 'Final release notice permissions differ');
  }
  return receipt;
}
