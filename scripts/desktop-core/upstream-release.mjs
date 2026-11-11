// Versioned adapter for unchanged upstream archives and their committed legal
// texts. It never runs target binaries. Producer evidence stays with the Core
// release; the App trusts the committed contract digest and archive/member pins.
import { readFileSync } from 'node:fs';
import { canonical, digest, kernelRepositoryIdentity, releaseGoVersion, requireGraph as demand } from './source-graph.mjs';
const REPO = 'polaris-arch/sing-box-polaris';
// Shared JS/Python grammar; Rust admission is checked against the same corpus.
const VERSION_PATTERN = JSON.parse(readFileSync(new URL('../lib/polaris-version.json', import.meta.url), 'utf8')).pattern;
const VERSION = new RegExp(VERSION_PATTERN);
const ARCHIVE_VERSION = new RegExp('^sing-box-' + VERSION_PATTERN.slice(1, -1) + '-(.+)$');
const upstreamVersion = value => typeof value === 'string' && VERSION.exec(value)?.[0] === value;
const hex = (v, n) => typeof v === 'string' && new RegExp(`^[a-f0-9]{${n}}$`).test(v);
const positive = (v) => Number.isSafeInteger(v) && v > 0;
const same = (a, b, reason) => demand(canonical(a) === canonical(b), reason);
export const isUpstreamRelease = (contract) => contract?.schema === 'polaris-upstream-native-release-consumer-v1';
export const isUpstreamRow = (row) => row?.schema === 'polaris-upstream-native-receipt-v1';

export function safeNativePath(name) {
  return typeof name === 'string' && name.length <= 512 && name.split('/').every((part) =>
    /^[A-Za-z0-9_+.-]+$/.test(part) && !['.', '..'].includes(part) && !part.endsWith('.')
    && !/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/i.test(part.split('.')[0]));
}
function file(value, max = 512 * 1024 * 1024) {
  demand(value && safeNativePath(value.name) && !value.name.includes('/')
    && positive(value.bytes) && value.bytes <= max && hex(value.sha256, 64), 'Upstream file pin differs');
}
// Public transport names use the upstream version spelling; immutable archives
// keep the Polaris build version. Explicit invalid aliases never fall back.
function nativeArchiveName(archive) {
  demand(archive && typeof archive.name === 'string' && safeNativePath(archive.name)
    && !archive.name.includes('/') && archive.name.startsWith('sing-box-')
    && !archive.name.startsWith('sing-box-polaris-'), 'Upstream native archive name differs');
  demand(!Object.hasOwn(archive, 'publicName'), 'Unsupported upstream public-name alias');
  return archive.name;
}
export function upstreamReleaseName(archive) {
  const name = nativeArchiveName(archive);
  const parts = ARCHIVE_VERSION.exec(name);
  demand(parts && parts[0] === name && archive.releaseName === `sing-box-polaris-${parts[1]}-${parts[2]}-${parts[4]}`,
    'Upstream release archive name differs');
  return archive.releaseName;
}
// Reconstruct only the producer's historical public-name annotation. Every
// other original receipt field is compared byte-for-byte by canonical value.
function originalProducerRow(row) {
  const { releaseAsset, ...producer } = row;
  upstreamReleaseName(row.archive);
  return { ...producer, archive: { ...row.archive,
    releaseName: 'sing-box-polaris-' + row.archive.name.slice('sing-box-'.length) } };
}
export function upstreamAsset(pin, contract, max) {
  file(pin, max);
  demand(positive(pin.assetId) && pin.releaseId === contract.releaseId
    && pin.url === `https://github.com/${REPO}/releases/download/${contract.tag}/${pin.name}`, 'Upstream asset identity differs');
}
function graph(graph) {
  demand(graph && graph.settings && !Array.isArray(graph.settings)
    && Object.values(graph.settings).every((v) => typeof v === 'string'), 'Upstream BuildInfo settings missing');
  const modules = [...(graph.normalModules ?? []), ...(graph.prebuiltAcquisitionModules ?? []), ...(graph.localReplacements ?? [])];
  demand(Array.isArray(graph.normalModules) && graph.normalModules.length > 0
    && Array.isArray(graph.prebuiltAcquisitionModules) && Array.isArray(graph.localReplacements)
    && modules.every((m) => typeof m.module === 'string' && m.module.length && typeof m.version === 'string' && typeof m.h1 === 'string')
    && new Set(modules.map((m) => m.module)).size === modules.length, 'Upstream module graph incomplete');
}
export function validateNativeArchiveRow(contract, key, row, targets) {
  const target = targets[key];
  demand(isUpstreamRow(row) && target && row.target === `${target.os}/${target.arch}`
    && row.version === contract.version && row.tag === contract.tag && row.sourceCommit === contract.sourceCommit
    && row.sourceTree === contract.sourceTree && typeof row.id === 'string' && /^[A-Za-z0-9._+-]+$/.test(row.id), 'Upstream target/source differs');
  const linkage = target.sidecar ? 'shared-purego' : 'static-cgo';
  demand(row.variant && row.variant.linkage === linkage && row.variant.legacy === false
    && ['libc', 'goarm', 'gomips', 'go386'].every((field) => row.variant[field] === ''), 'Upstream App recipe differs');
  demand(row.goVersion === releaseGoVersion(contract) && typeof row.buildID === 'string'
    && row.buildID.length <= 512 && typeof row.rawBuildInfo === 'string' && row.rawBuildInfo.length > 0 && row.rawBuildInfo.length <= 8 * 1024 * 1024 && !/[\r\n\0]/.test(row.buildID), 'Upstream toolchain/BuildID differs');
  const tags = typeof row.tags === 'string' ? row.tags.split(',') : [];
  demand(tags.length && tags.every((tag) => /^[a-z0-9_]+$/.test(tag)) && new Set(tags).size === tags.length
    && tags.includes('with_gvisor') && tags.includes('with_naive_outbound')
    && tags.includes('with_purego') === Boolean(target.sidecar), 'Upstream required capabilities differ');
  graph(row.buildGraph);
  const root = `sing-box-${contract.version}-${target.os}-${target.arch}`;
  file(row.archive);
  demand(row.archive.root === root && row.id === root && row.archive.name === root + target.suffix,
    'Upstream native archive recipe/layout differs');
  const members = row.archive.members;
  demand(members && Object.keys(members).length > 0 && Object.keys(members).length <= 1024
    && Object.entries(members).every(([name, hash]) => name.startsWith(`${root}/`) && safeNativePath(name) && hex(hash, 64)), 'Upstream archive members differ');
  for (const [payload, member] of [[row.core, target.core], ...(target.sidecar ? [[row.sidecar, target.sidecar]] : [])]) {
    demand(payload?.member === member && positive(payload.bytes) && payload.bytes <= 200 * 1024 * 1024
      && payload.sha256 === members[`${root}/${member}`], 'Upstream payload pin differs');
  }
  demand(hex(members[`${root}/LICENSE`], 64), 'Upstream native LICENSE absent');
  if (!target.sidecar) demand(row.sidecar === null && !Object.keys(members).some((name) => /\.(so|dll|dylib)$/.test(name)), 'Upstream static archive has shared payload');
  if (key.startsWith('mac-')) {
    const signature = row.signatureObservation;
    demand(signature?.trustValidated === false && (signature.state === 'unsigned' && signature.cdHash === null
      || signature.state === 'signed' && hex(signature.cdHash, 40)), 'Upstream Mac observation missing');
  } else demand(row.signatureObservation === null, 'Unexpected upstream Mac signature observation');
  return root;
}
export function validateUpstreamRow(contract, key, row, targets) {
  const root = validateNativeArchiveRow(contract, key, row, targets), members = row.archive.members;
  upstreamAsset(row.releaseAsset, contract);
  same({ name: upstreamReleaseName(row.archive), bytes: row.archive.bytes, sha256: row.archive.sha256 },
    { name: row.releaseAsset.name, bytes: row.releaseAsset.bytes, sha256: row.releaseAsset.sha256 }, 'Upstream archive/public bytes differ');
  const reserved = Object.keys(upstreamMetadata(contract, row));
  const combined = [...Object.keys(members).map((name) => name.slice(root.length + 1)), ...reserved];
  const folded = new Set(combined.map((name) => name.toLowerCase()));
  demand(folded.size === combined.length && combined.every((name) => { const parts = name.toLowerCase().split('/');
    return parts.every((_, index) => index === 0 || !folded.has(parts.slice(0, index).join('/'))); }),
  'Upstream sidecar collides with original archive or another sidecar');
  return root;
}
export function validateUpstreamContract(contract, targets) {
  demand(isUpstreamRelease(contract) && contract.providerContract === 'polaris-upstream-native-release-v1'
    && contract.repository === REPO && contract.repositoryId === kernelRepositoryIdentity(REPO).id
    && positive(contract.releaseId) && hex(contract.sourceCommit, 40) && hex(contract.sourceTree, 40)
    && hex(contract.tagObject, 40) && upstreamVersion(contract.version)
    && contract.tag === `v${contract.version}`, 'Upstream release contract identity differs');
  // Corresponding source is offered by public link (NOTICE); installers do not carry it.
  file(contract.sourceArchive);
  demand(contract.sourceArchive.name === `sing-box-polaris-${contract.sourceCommit}.tar.gz`
    && contract.sourceArchive.url === `https://github.com/${REPO}/archive/${contract.sourceCommit}.tar.gz`, 'Fixed commit source archive differs');
  demand(Array.isArray(contract.legalFiles) && contract.legalFiles.length > 0 && contract.legalFiles.length <= 1024
    && new Set(contract.legalFiles.map((f) => typeof f.path === 'string' ? f.path.toLowerCase() : f.path)).size === contract.legalFiles.length, 'Upstream legal inventory differs');
  for (const legal of contract.legalFiles) demand(safeNativePath(legal.path) && legal.path.startsWith('licenses/')
    && typeof legal.content === 'string' && Buffer.byteLength(legal.content) === legal.bytes && legal.bytes <= 2 * 1024 * 1024
    && digest(Buffer.from(legal.content)) === legal.sha256, 'Upstream original legal bytes differ');
  demand(contract.legalFiles.some((f) => f.path === 'licenses/LICENSE') && contract.legalFiles.some((f) => f.path === 'licenses/NOTICE'), 'Upstream source license/NOTICE missing');
  same(Object.keys(contract.platforms ?? {}).sort(), Object.keys(targets).sort(), 'Four explicit upstream App recipes required');
  const ids = [];
  for (const [key, row] of Object.entries(contract.platforms)) { validateUpstreamRow(contract, key, row, targets); ids.push(row.releaseAsset.assetId); }
  demand(new Set(ids).size === ids.length && new Set(Object.values(contract.platforms).map((r) => r.id)).size === 4, 'Duplicate upstream release identities');
  return contract;
}
// The Core release legal set covers every Core product. Desktop installers carry
// only texts for what they distribute: the desktop sing-box binary, its linked
// modules and Go toolchain, and libcronet. Core GUI clients, OCI images, USB/IP
// drivers, gomobile and other Go toolchains are not distributed by the App.
const UNDISTRIBUTED_LEGAL = ['licenses/clients/', 'licenses/oci/', 'licenses/usbip-original/', 'licenses/gomobile-', 'licenses/historical/'];
export function distributedLegalFiles(contract) {
  const go = `licenses/go-${releaseGoVersion(contract).slice(2)}/`;
  const kept = contract.legalFiles.filter(({ path }) => !UNDISTRIBUTED_LEGAL.some((prefix) => path.startsWith(prefix))
    && (!path.startsWith('licenses/go-') || path.startsWith(go)));
  demand(kept.some(({ path }) => path.startsWith(go)), 'Go toolchain license missing');
  return kept;
}
export function upstreamMetadata(contract, row) {
  const producer = originalProducerRow(row);
  const source = { schema: 'polaris-upstream-source-delivery-v1', repository: contract.repository,
    sourceCommit: contract.sourceCommit, sourceTree: contract.sourceTree, tag: contract.tag, tagObject: contract.tagObject,
    sourceArchive: contract.sourceArchive };
  return { ...Object.fromEntries(distributedLegalFiles(contract).map((f) => [f.path, Buffer.from(f.content)])),
    'provenance.json': Buffer.from(JSON.stringify({ schema: 'polaris-upstream-public-provenance-v1', producer: contract.producer, receipt: producer }, null, 2) + '\n'),
    'source/SOURCE-INDEX.json': Buffer.from(JSON.stringify(source, null, 2) + '\n') };
}
export function upstreamPayloadPins(contract, row) {
  const pins = Object.fromEntries(Object.entries(row.archive.members).map(([name, hash]) => [name.slice(row.archive.root.length + 1), hash])
    .filter(([name]) => name !== row.core.member && name !== row.sidecar?.member));
  for (const [name, bytes] of Object.entries(upstreamMetadata(contract, row))) pins[name] = digest(bytes);
  return pins;
}
export function contractPublicAssets(contract) {
  return Object.values(contract.platforms).map((r) => r.releaseAsset);
}
