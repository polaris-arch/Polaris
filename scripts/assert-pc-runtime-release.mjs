#!/usr/bin/env node

// Publication requires the selected formal release contract and committed index.
// Source provenance is validated here; native acceptance stays unobserved.
// The signed Release DAG still owns all CI,
// producer/consumer, tag, signature, asset-digest and promotion checks.
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadReleaseContract } from './desktop-core/release-consumer.mjs';
import { hasCommittedReleaseMaterials, isUpstreamRelease } from './desktop-core/upstream-release.mjs';
import { canonical, DESKTOP_TARGETS, digest, requireGraph, validateSourceManifest,
  validateSourcePins, hasDesktopSource, desktopSourceManifestPath } from './desktop-core/source-graph.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
// Product policy of this closure: the Linux ownership chain is not built. The
// generic validators accept an empty dependency graph; this requires it.
export const OWNERSHIP_CHAIN_PATCHES = Object.freeze(['tun-owner-consumer.patch',
  'sing-tun-owned-native.patch', 'nftables-owned-transactions.patch']);
const isEmptyList = (value) => Array.isArray(value) && value.length === 0;
export function assertOwnershipChainAbsent(spec, source) {
  requireGraph(isEmptyList(source.dependencyPatches) && isEmptyList(spec.dependencyModules),
    'Ownership chain policy: dependency patches must be empty');
  requireGraph(Array.isArray(source.patches)
    && !source.patches.some((patch) => OWNERSHIP_CHAIN_PATCHES.includes(patch?.file)),
  'Ownership chain policy: retired patch listed');
  requireGraph(Object.values(spec.platforms).every((platform) =>
    isEmptyList(platform.patchedModules.requiredLinked) && isEmptyList(platform.patchedModules.allowedAbsent)),
  'Ownership chain policy: platform patched modules must be empty');
  requireGraph(spec.graphScope === 'core-source-only', 'Ownership chain policy: graph scope must be core-source-only');
}
const readRepository = (relative) => readFileSync(join(root, relative));

// Validate the selected committed release and its full index. Runtime/signature/
// publication checks remain the responsibility of the existing Release DAG.
function assertFormalConsumer(manifest) {
  requireGraph(manifest.desktopRelease && canonical(Object.keys(manifest.desktopRelease).sort())
    === canonical(['schema', 'contractPath', 'contractSha256', 'version', 'tag', 'tagObject',
      'sourceCommit', 'sourceTree', 'releaseId', 'librarySha256'].sort()),
  'Formal consumer selection fields differ; claimed acceptance is not source authority');
  const contract = loadReleaseContract(root, manifest);
  requireGraph(isUpstreamRelease(contract) && hasCommittedReleaseMaterials(contract),
    'Formal publication requires a committed upstream release contract and index');
  return {
    policyState: 'FORMAL_CONSUMER_SOURCE_ELIGIBLE',
    contractSha256: manifest.desktopRelease.contractSha256,
    sourceCommit: contract.sourceCommit,
    sourceTree: contract.sourceTree,
    tagObject: contract.tagObject,
    releaseId: contract.releaseId,
    publicationRequirements: 'existing-signed-release-dag',
    nativeAcceptance: 'NotObserved',
    deviceAcceptance: 'NotObserved',
  };
}

export function assertSourceFirstRelease(read = readRepository) {
  const manifest = JSON.parse(read('src-tauri/core-manifest.json'));
  requireGraph(!Object.hasOwn(manifest, 'desktopTemporary'),
    'Temporary desktop default is not formal App release approval; packaged acceptance remains unobserved');
  if (Object.hasOwn(manifest, 'desktopRelease')) return assertFormalConsumer(manifest);
  for (const key of Object.keys(DESKTOP_TARGETS)) validateSourcePins(manifest, key, false);
  const spec = validateSourcePins(manifest, 'linux', false);
  const sourcePath = desktopSourceManifestPath(spec);
  assertOwnershipChainAbsent(spec, JSON.parse(read(sourcePath)));
  const check = (path, sha) => {
    const bytes = read(path);
    requireGraph(digest(bytes) === sha, `Source SHA-256 mismatch: ${path}`);
    return bytes;
  };
  const checkSource = (path, sourceSpec) => {
    const source = JSON.parse(check(path, sourceSpec.sourceManifestSha256));
    const dependencies = validateSourceManifest(source, sourceSpec);
    assertOwnershipChainAbsent(sourceSpec, source);
    check('scripts/core-source-provision.py', sourceSpec.provisionerSha256);
    for (const patch of source.patches) check(`scripts/libbox-patches/${patch.file}`, patch.sha256);
    for (const dep of dependencies) check(`scripts/libbox-patches/${dep.patchFile}`, dep.patchSha256);
  };
  checkSource(sourcePath, spec);
  if (hasDesktopSource(manifest)) {
    const mobile = { bundledCoreVersion: manifest.bundledCoreVersion, sourceBuild: manifest.sourceBuild };
    for (const key of Object.keys(DESKTOP_TARGETS)) validateSourcePins(mobile, key, false);
    checkSource('scripts/libbox-patches/source-manifest.json', mobile.sourceBuild);
  } else {
    check('scripts/core-patches/windows-dns-refresh.patch', manifest.windowsBuild.patchSha256);
  }
  // The retired App-owned source producer is not a formal release provider.
  // Valid source inputs alone never authorize publishing its compiled outputs.
  throw new Error('Desktop publication requires a formal upstream release contract');
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    requireGraph(process.argv.length === 2, 'Release policy takes no arguments or clearance overrides');
    console.log(JSON.stringify(assertSourceFirstRelease()));
  } catch (error) {
    console.error(`PC_RUNTIME_RELEASE_BLOCKED: ${error.message}`);
    process.exitCode = 1;
  }
}
