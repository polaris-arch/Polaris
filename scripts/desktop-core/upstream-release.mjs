// Versioned adapter for unchanged upstream archives and same-run source/legal
// sidecars. It never upgrades historical v1 evidence or runs target binaries.
import { readFileSync } from 'node:fs';
import { canonical, digest, kernelRepositoryIdentity, upstreamSourceGoVersion, reviewedSourcePolicy, requireGraph as demand } from './source-graph.mjs';
const REPO = 'polaris-arch/sing-box-polaris';
// Shared JS/Python grammar; Rust admission is checked against the same corpus.
const VERSION_PATTERN = JSON.parse(readFileSync(new URL('../lib/polaris-version.json', import.meta.url), 'utf8')).pattern;
const VERSION = new RegExp(VERSION_PATTERN);
const ARCHIVE_VERSION = new RegExp('^sing-box-' + VERSION_PATTERN.slice(1, -1) + '-(.+)$');
const upstreamVersion = value => typeof value === 'string' && VERSION.exec(value)?.[0] === value;
const hex = (v, n) => typeof v === 'string' && new RegExp(`^[a-f0-9]{${n}}$`).test(v);
const positive = (v) => Number.isSafeInteger(v) && v > 0;
const same = (a, b, reason) => demand(canonical(a) === canonical(b), reason);
export const UPSTREAM_SMOKE_JOBS = Object.freeze({ linux: 'linux-amd64-purego', win: 'windows-amd64', 'mac-x64': 'darwin-amd64', 'mac-arm64': 'darwin-arm64' });
export const isUpstreamRelease = (contract) => contract?.schema === 'polaris-upstream-native-release-consumer-v1';
export const COMMITTED_INDEX_PATH = 'scripts/desktop-core/release-asset-index.json';
export const hasCommittedReleaseMaterials = contract => contract?.materialTransport === 'committed-index-github-source-v1';
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
// and their producer index keep the Polaris build version. Only absence retains
// the historical unbranded transport; explicit invalid aliases never fall back.
function nativeArchiveName(archive) {
  demand(archive && typeof archive.name === 'string' && safeNativePath(archive.name)
    && !archive.name.includes('/') && archive.name.startsWith('sing-box-')
    && !archive.name.startsWith('sing-box-polaris-'), 'Upstream native archive name differs');
  demand(!Object.hasOwn(archive, 'publicName'), 'Unsupported upstream public-name alias');
  return archive.name;
}
function producerReleaseName(archive) {
  const name = nativeArchiveName(archive);
  if (!Object.hasOwn(archive, 'releaseName')) return name;
  const expected = 'sing-box-polaris-' + name.slice('sing-box-'.length);
  demand(archive.releaseName === expected, 'Upstream producer release archive name differs');
  return expected;
}
export function upstreamReleaseName(archive) {
  const name = nativeArchiveName(archive);
  if (!Object.hasOwn(archive, 'releaseName')) return name;
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
  return Object.hasOwn(row.archive, 'releaseName')
    ? { ...producer, archive: { ...row.archive,
      releaseName: 'sing-box-polaris-' + row.archive.name.slice('sing-box-'.length) } }
    : producer;
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
  demand([...graph.normalModules, ...graph.prebuiltAcquisitionModules].every((m) => /^h1:[A-Za-z0-9+/]+=$/.test(m.h1))
    && graph.prebuiltAcquisitionModules.every((m) => m.module.startsWith('github.com/sagernet/cronet-go/lib/'))
    && graph.normalModules.every((m) => !m.module.startsWith('github.com/sagernet/cronet-go/lib/')),
  'Upstream module acquisition identities differ');
  demand(graph.localReplacements.every((m) => m.replacement && typeof m.replacement.module === 'string'
    && typeof m.replacement.version === 'string' && typeof m.replacement.h1 === 'string'
    && (m.replacement.module.startsWith('./')
      ? m.h1 === '' && m.replacement.version === '(devel)' && m.replacement.h1 === ''
      : /^v[0-9]/.test(m.replacement.version) && /^h1:[A-Za-z0-9+/]+=$/.test(m.replacement.h1))), 'Upstream replacement identities missing');
}
// Transport-independent strict native receipt validation. Candidate transport must
// separately prove its public identity; formal release validation below is unchanged.
export function validateNativeArchiveRow(contract, key, row, targets) {
  const target = targets[key];
  demand(isUpstreamRow(row) && target && row.target === `${target.os}/${target.arch}`
    && row.version === contract.version && row.tag === contract.tag && row.sourceCommit === contract.sourceCommit
    && row.sourceTree === contract.sourceTree && typeof row.id === 'string' && /^[A-Za-z0-9._+-]+$/.test(row.id), 'Upstream target/source differs');
  const linkage = target.sidecar ? 'shared-purego' : 'static-cgo';
  demand(row.variant && row.variant.linkage === linkage && row.variant.legacy === false
    && ['libc', 'goarm', 'gomips', 'go386'].every((field) => row.variant[field] === ''), 'Upstream App recipe differs');
  demand(row.goVersion === upstreamSourceGoVersion(contract) && typeof row.buildID === 'string'
    && row.buildID.length <= 512 && typeof row.rawBuildInfo === 'string' && row.rawBuildInfo.length > 0 && row.rawBuildInfo.length <= 8 * 1024 * 1024 && !/[\r\n\0]/.test(row.buildID), 'Upstream toolchain/BuildID differs');
  const tags = typeof row.tags === 'string' ? row.tags.split(',') : [];
  demand(tags.length && tags.every((tag) => /^[a-z0-9_]+$/.test(tag)) && new Set(tags).size === tags.length
    && tags.includes('with_gvisor') && tags.includes('with_naive_outbound')
    && tags.includes('with_purego') === Boolean(target.sidecar), 'Upstream required capabilities differ');
  const source = row.sourceIdentity;
  demand(source?.sourceOverlay === 0 && hex(source.goModSha256, 64) && hex(source.goSumSha256, 64)
    && Array.isArray(source.replacements) && source.replacements.length > 0
    && source.replacements.every((r) => typeof r.module === 'string' && r.module.length
      && typeof r.path === 'string' && r.path.startsWith('./') && safeNativePath(r.path.slice(2)) && hex(r.tree, 40))
    && new Set(source.replacements.map((r) => r.module)).size === source.replacements.length,
  'Upstream source/replacement proof differs');
  graph(row.buildGraph);
  const policy = reviewedSourcePolicy(contract);
  if (policy) {
    const meta = policy.metacubex;
    demand(source.goModSha256 === policy.goModSha256 && source.goSumSha256 === policy.goSumSha256,
      'Reviewed source module pins differ');
    const vendors = policy.schema === 'polaris-reviewed-kernel-source-v2' ? policy.vendors : undefined;
    const expected = (vendors ? Object.keys(vendors) : ['github.com/metacubex/utls', 'github.com/sagernet/sing-tun']).sort();
    same(source.replacements.map((r) => r.module).sort(), expected, 'Reviewed local source inventory differs');
    demand(source.replacements.some((r) => r.module === meta.module && r.path === meta.replacementPath
      && r.tree === meta.replacementTree), 'Reviewed Meta source subtree differs');
    const local = row.buildGraph.localReplacements.filter((r) => r.replacement.module.startsWith('./'));
    same(local.map((r) => r.module).sort(), expected, 'Reviewed embedded local inventory differs');
    // Desktop retains complete source/linked equality. A source-policy schema
    // upgrade alone never authorizes platform pruning or a different profile.
    for (const [name, pin] of Object.entries(vendors ?? {})) {
      demand(source.replacements.some((r) => r.module === name && r.path === pin.replacementPath
        && r.tree === pin.replacementTree), 'Reviewed vendor source subtree differs');
      const linked = local.find((r) => r.module === name);
      demand(linked.version === pin.version && linked.h1 === ''
        && linked.replacement.module === pin.replacementPath && linked.replacement.version === '(devel)'
        && linked.replacement.h1 === '', 'Actual reviewed vendor local replacement differs');
    }
    const actual = local.find((r) => r.module === meta.module);
    demand(actual.version === meta.version && actual.h1 === ''
      && actual.replacement.module === meta.replacementPath && actual.replacement.version === '(devel)'
      && actual.replacement.h1 === '', 'Actual Meta local replacement differs; baseline h1 is not a local sum');
  }

  // The inherited field name records every Go => entry, including remote
  // module replacements. Only repository-local entries have a Git subtree.
  const local = row.buildGraph.localReplacements.filter((r) => r.replacement.module.startsWith('./'));
  demand(local.some((r) => r.module === 'github.com/sagernet/sing-tun'),
    'Source and binary local replacements differ');
  same(local.map((r) => ({ module: r.module, path: r.replacement.module }))
    .sort((a, b) => a.module.localeCompare(b.module)),
  source.replacements.map(({ module, path }) => ({ module, path }))
    .sort((a, b) => a.module.localeCompare(b.module)),
  'Source and binary local replacements differ');
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
  const smoke = row.finalPackageCronetSmoke;
  demand(smoke?.status === 'passed' && smoke.os === target.os && smoke.arch === target.arch && smoke.go === row.goVersion
    && smoke.gvisorCompiled === true && smoke.naiveOutbound === true && smoke.linkage === linkage
    && /^\d+\.\d+\.\d+\.\d+$/.test(smoke.version ?? '') && smoke.coreSha256 === row.core.sha256
    && smoke.librarySha256 === (row.sidecar?.sha256 ?? null), 'Actual native capability smoke missing or unbound');
  return root;
}
export function validateUpstreamRow(contract, key, row, targets) {
  const root = validateNativeArchiveRow(contract, key, row, targets), members = row.archive.members;
  upstreamAsset(row.releaseAsset, contract);
  same({ name: upstreamReleaseName(row.archive), bytes: row.archive.bytes, sha256: row.archive.sha256 },
    { name: row.releaseAsset.name, bytes: row.releaseAsset.bytes, sha256: row.releaseAsset.sha256 }, 'Upstream archive/public bytes differ');
  const reserved = Object.keys(upstreamMetadata(contract, row));
  const combined = [...Object.keys(members).map((name) => name.slice(root.length + 1)), ...reserved, `source/${contract.sourceArchive.name}`];
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
  const producer = contract.producer;
  demand(producer?.repository === REPO && producer.repositoryId === contract.repositoryId
    && /^\.github\/workflows\/[^/]+\.yml$/.test(producer.workflow ?? '') && hex(producer.workflowCommit, 40)
    && producer.workflowCommit === contract.sourceCommit && positive(producer.runId) && positive(producer.runAttempt), 'Upstream same-run producer identity differs');
  same(Object.keys(producer.nativeJobs ?? {}).sort(), Object.keys(targets).sort(), 'Upstream native job inventory differs');
  for (const [key, job] of Object.entries(producer.nativeJobs)) demand(positive(job.id)
    && job.name === `Native final-package Cronet and gVisor smoke (${UPSTREAM_SMOKE_JOBS[key]})`, 'Upstream native job identity differs');
  demand(new Set(Object.values(producer.nativeJobs).map((job) => job.id)).size === 4, 'Duplicate upstream native job');
  if (Object.hasOwn(contract, 'materialTransport')) {
    demand(hasCommittedReleaseMaterials(contract), 'Unsupported release material transport');
    file(contract.assetIndex, 8 * 1024 * 1024);
    same(Object.keys(contract.assetIndex).sort(), ['name', 'bytes', 'sha256', 'path'].sort(), 'Committed index fields differ');
    demand(contract.assetIndex.path === COMMITTED_INDEX_PATH, 'Committed index path differs');
    file(contract.sourceArchive);
    same(Object.keys(contract.sourceArchive).sort(), ['transport', 'name', 'bytes', 'sha256', 'url'].sort(), 'Commit archive fields differ');
    demand(contract.sourceArchive.transport === 'github-commit-archive-v1'
      && contract.sourceArchive.name === `sing-box-polaris-${contract.sourceCommit}.tar.gz`
      && contract.sourceArchive.url === `https://github.com/${REPO}/archive/${contract.sourceCommit}.tar.gz`, 'Fixed commit source archive differs');
    file(contract.producerSourceArchive);
    same(Object.keys(contract.producerSourceArchive).sort(), ['name', 'bytes', 'sha256'].sort(), 'Original source archive fields differ');
    demand(contract.producerSourceArchive.name === `sing-box-polaris-source-${contract.sourceCommit}.tar.gz`, 'Original producer source archive differs');
  } else {
    demand(!Object.hasOwn(contract, 'producerSourceArchive') && !Object.hasOwn(contract.assetIndex ?? {}, 'path')
      && !Object.hasOwn(contract.sourceArchive ?? {}, 'transport'), 'Mixed release material transport');
    upstreamAsset(contract.assetIndex, contract, 8 * 1024 * 1024);
    upstreamAsset(contract.sourceArchive, contract);
    demand(contract.sourceArchive.name === `sing-box-polaris-source-${contract.sourceCommit}.tar.gz`, 'Upstream complete source archive differs');
  }
  demand(contract.assetIndex.name === 'polaris-upstream-native-assets.json', 'Upstream index name differs');
  demand(Array.isArray(contract.legalFiles) && contract.legalFiles.length > 0 && contract.legalFiles.length <= 1024
    && new Set(contract.legalFiles.map((f) => typeof f.path === 'string' ? f.path.toLowerCase() : f.path)).size === contract.legalFiles.length, 'Upstream legal inventory differs');
  for (const legal of contract.legalFiles) demand(safeNativePath(legal.path) && legal.path.startsWith('licenses/')
    && typeof legal.content === 'string' && Buffer.byteLength(legal.content) === legal.bytes && legal.bytes <= 2 * 1024 * 1024
    && digest(Buffer.from(legal.content)) === legal.sha256, 'Upstream original legal bytes differ');
  demand(contract.legalFiles.some((f) => f.path === 'licenses/LICENSE') && contract.legalFiles.some((f) => f.path === 'licenses/NOTICE'), 'Upstream source license/NOTICE missing');
  same(Object.keys(contract.platforms ?? {}).sort(), Object.keys(targets).sort(), 'Four explicit upstream App recipes required');
  const ids = hasCommittedReleaseMaterials(contract) ? [] : [contract.assetIndex.assetId, contract.sourceArchive.assetId];
  for (const [key, row] of Object.entries(contract.platforms)) { validateUpstreamRow(contract, key, row, targets); ids.push(row.releaseAsset.assetId); }
  demand(new Set(ids).size === ids.length && new Set(Object.values(contract.platforms).map((r) => r.id)).size === 4, 'Duplicate upstream release identities');
  const rows = Object.values(contract.platforms);
  for (const row of rows.slice(1)) same(row.sourceIdentity, rows[0].sourceIdentity, 'Upstream platform sources differ');
  return contract;
}
export function validateUpstreamIndex(index, contract) {
  demand(index.schema === 'polaris-upstream-native-assets-v1' && Array.isArray(index.platforms)
    && index.platforms.length >= 4 && index.platforms.length <= 512
    && index.platforms.every((r) => typeof r.id === 'string' && /^[A-Za-z0-9._+-]+$/.test(r.id))
    && new Set(index.platforms.map((r) => r.id)).size === index.platforms.length, 'Upstream complete index identities differ');
  // Additional producer recipes are not downloaded by this App selection, but
  // an explicit producer name must still obey its original closed mapping.
  for (const row of index.platforms) {
    if (row.archive && (Object.hasOwn(row.archive, 'releaseName') || Object.hasOwn(row.archive, 'publicName'))) producerReleaseName(row.archive);
  }
  for (const name of ['repository', 'repositoryId', 'version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'producer', 'legalFiles'])
    same(index[name], contract[name], `Upstream index ${name} differs`);
  const { name, bytes, sha256 } = hasCommittedReleaseMaterials(contract) ? contract.producerSourceArchive : contract.sourceArchive;
  same(index.sourceArchive, { name, bytes, sha256 }, 'Upstream source archive index differs');
  for (const row of Object.values(contract.platforms)) {
    const producer = originalProducerRow(row);
    same(index.platforms.find((r) => r.id === row.id), producer, 'Upstream indexed recipe differs');
  }
  return index;
}
export function upstreamMetadata(contract, row) {
  const producer = originalProducerRow(row);
  const source = { schema: 'polaris-upstream-source-delivery-v1', repository: contract.repository,
    sourceCommit: contract.sourceCommit, sourceTree: contract.sourceTree, tag: contract.tag, tagObject: contract.tagObject,
    sourceArchive: contract.sourceArchive, producer: contract.producer,
    ...(contract.sourcePolicy ? { sourcePolicy: contract.sourcePolicy } : {}) };
  return { ...Object.fromEntries(contract.legalFiles.map((f) => [f.path, Buffer.from(f.content)])),
    'provenance.json': Buffer.from(JSON.stringify({ schema: 'polaris-upstream-public-provenance-v1', producer: contract.producer, receipt: producer }, null, 2) + '\n'),
    'source/SOURCE-INDEX.json': Buffer.from(JSON.stringify(source, null, 2) + '\n') };
}
export function upstreamPayloadPins(contract, row) {
  const pins = Object.fromEntries(Object.entries(row.archive.members).map(([name, hash]) => [name.slice(row.archive.root.length + 1), hash])
    .filter(([name]) => name !== row.core.member && name !== row.sidecar?.member));
  for (const [name, bytes] of Object.entries(upstreamMetadata(contract, row))) pins[name] = digest(bytes);
  pins[`source/${contract.sourceArchive.name}`] = contract.sourceArchive.sha256;
  return pins;
}
export function contractPublicAssets(contract) {
  return [...(hasCommittedReleaseMaterials(contract) ? [] : [contract.assetIndex, ...(isUpstreamRelease(contract) ? [contract.sourceArchive] : [])]),
    ...Object.values(contract.platforms).map((r) => r.releaseAsset)];
}
export function validateUpstreamReadback(snapshot, contract) {
  if (!isUpstreamRelease(contract)) return;
  const readback = snapshot.producerReadback, producer = contract.producer;
  demand(readback?.runId === producer.runId && readback.runAttempt === producer.runAttempt
    && readback.sourceCommit === contract.sourceCommit && readback.workflow === producer.workflow
    && positive(readback.workflowId) && readback.status === 'completed' && readback.conclusion === 'success',
  'Saved upstream producer readback differs');
  same(Object.keys(readback.jobs ?? {}).sort(), Object.keys(producer.nativeJobs).sort(), 'Saved upstream jobs differ');
  for (const [key, expected] of Object.entries(producer.nativeJobs)) same(readback.jobs[key], {
    id: expected.id, name: expected.name, runId: producer.runId, runAttempt: producer.runAttempt,
    sourceCommit: contract.sourceCommit, status: 'completed', conclusion: 'success' }, 'Saved native job identity differs');
  validateUpstreamSourceReadback(snapshot, contract);
}
// Published consumption trusts the reviewed contract/index in this App commit.
// It revalidates public source and bytes, not the producer's historical Actions
// status. Never label these observations as a fresh producer-success readback.
export function validatePublishedUpstreamReadback(snapshot, contract) {
  if (!isUpstreamRelease(contract)) return;
  demand(!Object.hasOwn(snapshot, 'producerReadback'), 'Published readback must not claim live producer verification');
  validateUpstreamSourceReadback(snapshot, contract);
}
function validateUpstreamSourceReadback(snapshot, contract) {
  demand(Array.isArray(snapshot.sourceGitlinks) && snapshot.sourceGitlinks.length <= 1024
    && snapshot.sourceGitlinks.every((entry) => typeof entry.path === 'string' && entry.path.length > 0
      && entry.path.split('/').every((part) => part && !['.', '..'].includes(part) && !part.includes('\0')) && hex(entry.sha, 40))
    && new Set(snapshot.sourceGitlinks.map((entry) => entry.path)).size === snapshot.sourceGitlinks.length,
  'Saved public gitlinks differ');
}

export async function readUpstreamPublicEvidence(contract, request) {
    const producer = contract.producer;
    const run = await request(`/actions/runs/${producer.runId}/attempts/${producer.runAttempt}`);
    demand(run.id === producer.runId && run.run_attempt === producer.runAttempt
      && run.status === 'completed' && run.conclusion === 'success' && run.head_sha === contract.sourceCommit
      && run.path === producer.workflow && positive(run.workflow_id)
      && ['workflow_dispatch', 'push', 'release'].includes(run.event)
      && run.repository?.id === contract.repositoryId && run.repository.full_name === contract.repository
      && run.head_repository?.id === contract.repositoryId && run.head_repository.full_name === contract.repository,
    'Published upstream producer run differs');
    const workflow = await request(`/actions/workflows/${run.workflow_id}`);
    demand(workflow.id === run.workflow_id && workflow.path === producer.workflow, 'Published upstream workflow differs');
    const jobs = {};
    for (const [key, expected] of Object.entries(producer.nativeJobs)) {
      const job = await request(`/actions/jobs/${expected.id}`);
      demand(job.id === expected.id && job.name === expected.name && job.run_id === producer.runId
        && job.run_attempt === producer.runAttempt && job.head_sha === contract.sourceCommit
        && job.status === 'completed' && job.conclusion === 'success', 'Published upstream native job differs');
      jobs[key] = { id: job.id, name: job.name, runId: job.run_id, runAttempt: job.run_attempt,
        sourceCommit: job.head_sha, status: job.status, conclusion: job.conclusion };
    }
    const source = await readUpstreamSourceEvidence(contract, request);
    return { producerReadback: { runId: run.id, runAttempt: run.run_attempt, sourceCommit: run.head_sha,
      workflow: workflow.path, workflowId: workflow.id, status: run.status, conclusion: run.conclusion, jobs }, ...source };
}

// Used by both published consumption and the strict candidate/publication path.
export async function readUpstreamSourceEvidence(contract, request) {
    const tree = await request(`/git/trees/${contract.sourceTree}?recursive=1`);
    demand(tree.sha === contract.sourceTree && tree.truncated === false && Array.isArray(tree.tree), 'Complete public source tree unavailable');
    const sourceGitlinks = tree.tree.filter((entry) => entry.mode === '160000' || entry.type === 'commit')
      .map((entry) => { demand(entry.mode === '160000' && entry.type === 'commit' && hex(entry.sha, 40)
        && typeof entry.path === 'string' && entry.path.split('/').every((part) => part && !['.', '..'].includes(part)),
      'Public source gitlink differs'); return { path: entry.path, sha: entry.sha }; }).sort((a,b) => a.path.localeCompare(b.path));
    const evidence = { sourceGitlinks };
    validateUpstreamSourceReadback(evidence, contract);
    return evidence;
}
