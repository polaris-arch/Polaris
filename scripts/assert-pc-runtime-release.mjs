#!/usr/bin/env node

// Source-first publication accepts this reviewed source closure only. Native
// acceptance remains unobserved. The signed Release DAG still owns all CI,
// producer/consumer, tag, signature, asset-digest and promotion checks.
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonical, DESKTOP_TARGETS, digest, requireGraph, validateSourceManifest,
  validateSourcePins, hasDesktopSource, desktopSourceManifestPath } from './desktop-core/source-graph.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
// Historical reviewed backend anchor, not a commit claiming the later Go inputs.
const reviewedCandidate = '123259cb4ee0eef484368e34d8ea7211d39964b6';
// This closure removes the Linux ownership chain from the core source: one
// core patch and both dependency patches leave the build, so the dependency
// graph is empty and the core is internal version .polaris.4. The latest
// implementation/review hashes identify that removal's source record.
// Earlier closures: unstarted validation implementation
// daaa1ca96138b3df13ccafae33bc6183ba483eeafcb5371921e1f4d07b2c88c7,
// review 156d8c345c73ae929a77112f566ad3dbe72b76be354a6b7e7dd736520246c841.
// Membership implementation 317f0dbe99b89bc018cfe9af7dfc0a0d2536d166566b1ff6a512d5285c2f9c67,
// review 26e53101a2510edaf9038e3e5d75b1e982ffcada2a540ede0c38a57971005553.
// Retirement implementation 6ee4c9f0fa982dc5cc691e6fc7babd3c78b174a799d62b64ce6cce296c7eb9e3,
// review f09f2f6a6a602d3e650980881abba1fb7ed85d6c072419f8a13936b2895ef846.
// Earlier shared Go review 3579c1d6e782ff53142a60ac5c279fa9ed263bdc9147eaf406b8a7088eaa27ba
// remains the base anchor. These records do not claim a new candidate commit,
// native acceptance or global ownership clearance.
const reviewedSourceIncrement = Object.freeze({
  scope: 'ownership-chain-removal-source-only',
  implementationSha256: '8219f717e33b2f561cf7671bb7abbab59ca38acd36d9389f2787acced5a6c27a',
  independentReviewSha256: 'b89812708d740515aea83369777889da2f98edc3e6cb111544889a9fe9f063a1',
  sourcePatchSha256: '839cfff957264aec8d4a85bb9e4efd272c4cfc6ad08de5eeb33d4c8f8c6e55b5',
  sourceManifestSha256: 'd7d8441334f1b2c3a42b15312e26fb40af453b2bb1755c289b5bd4827afaadec',
  sourceReceiptFingerprint: '151aa5ab53084a25a80f571bf6281b7cd18eb429a1548ad9cafaeba044eecafe',
});
// Canonical inputs bind the accepted deltas and the backend graph without the
// ownership chain, plus the original Windows overlay replay and internal
// .polaris.4 version.
// Source-only pins do not claim newly compiled output. Only five binary
// output pins are excluded: the existing all-four consumer verifies actual
// producer outputs for the current candidate/run/attempt, rather than old pins.
const reviewedInputsSha256 = 'ffcee458004b621e531917a23338bca978981406cb2087c22fd55bdd39b3a31a';
// Projection of the same reviewed legacy closure: retain the entire graph
// consumed by Android/iOS, not just a display version. Output bytes still bind
// at their native consumer gates. This projection grants no D release authority.
const reviewedMobileInputsSha256 = 'd5ecd0fd5c1131b5ecd1a2842fa8d3cf35b3a940aecdce03fdfa8ebabb3f77b7';
const reviewedAppleSourceSha256 = '1468d594ca51c4ae82823fae0a4d363453962b1c3ab0a15a8c14963d6589f942';

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

export function assertSourceFirstRelease(read = readRepository) {
  const manifest = JSON.parse(read('src-tauri/core-manifest.json'));
  for (const key of Object.keys(DESKTOP_TARGETS)) validateSourcePins(manifest, key, false);
  const spec = validateSourcePins(manifest, 'linux', false);
  const sourcePath = desktopSourceManifestPath(spec);
  // Named before the digests so a reintroduction is not reported as a hash drift.
  assertOwnershipChainAbsent(spec, JSON.parse(read(sourcePath)));
  const inputs = structuredClone(manifest);
  if (hasDesktopSource(manifest)) {
    // Structural admission is not release authority. Bind D bytes and retain
    // the complete old mobile graph; the new reviewed closure needs real pins.
    requireGraph(digest(read(sourcePath)) === spec.sourceManifestSha256,
      'Reviewed desktop source SHA-256 differs');
    requireGraph(digest(read('scripts/core-source-provision.py')) === spec.provisionerSha256,
      'Reviewed desktop provisioner SHA-256 differs');
    validateSourceManifest(JSON.parse(read(sourcePath)), spec);
    const mobileInputs = { bundledCoreVersion: manifest.bundledCoreVersion, sourceBuild: structuredClone(manifest.sourceBuild) };
    const mobileManifest = { ...mobileInputs };
    for (const key of Object.keys(DESKTOP_TARGETS)) validateSourcePins(mobileManifest, key, false);
    for (const platform of Object.values(mobileInputs.sourceBuild.platforms)) delete platform.binarySha256;
    requireGraph(digest(canonical(mobileInputs)) === reviewedMobileInputsSha256,
      'Mobile source inputs differ from the reviewed release policy');
    const check = (path, sha) => {
      const bytes = read(path);
      requireGraph(digest(bytes) === sha, `Reviewed mobile source SHA-256 mismatch: ${path}`);
      return bytes;
    };
    const mobileSpec = mobileManifest.sourceBuild;
    const mobileSource = JSON.parse(check('scripts/libbox-patches/source-manifest.json', mobileSpec.sourceManifestSha256));
    const mobileDependencies = validateSourceManifest(mobileSource, mobileSpec);
    assertOwnershipChainAbsent(mobileSpec, mobileSource);
    check('scripts/core-source-provision.py', mobileSpec.provisionerSha256);
    for (const patch of mobileSource.patches) check(`scripts/libbox-patches/${patch.file}`, patch.sha256);
    for (const dep of mobileDependencies) check(`scripts/libbox-patches/${dep.patchFile}`, dep.patchSha256);
    check('scripts/libbox-ios-patches/source-manifest.json', reviewedAppleSourceSha256);
    throw new Error('Desktop fork source review must be renewed: exact D release input closure not recorded');
  }
  delete inputs.windowsBuild.binarySha256;
  for (const platform of Object.values(inputs.sourceBuild.platforms)) delete platform.binarySha256;
  requireGraph(digest(canonical(inputs)) === reviewedInputsSha256,
    'Source inputs differ from the reviewed release policy; source review must be renewed');

  const check = (relative, expected) => {
    const bytes = read(relative);
    requireGraph(digest(bytes) === expected, `Reviewed source SHA-256 mismatch: ${relative}`);
    return bytes;
  };
  const source = JSON.parse(check(sourcePath, spec.sourceManifestSha256));
  const dependencies = validateSourceManifest(source, spec);
  requireGraph(spec.sourceManifestSha256 === reviewedSourceIncrement.sourceManifestSha256
    && spec.sourceReceiptFingerprint === reviewedSourceIncrement.sourceReceiptFingerprint
    && source.patches.find((patch) => patch.file === 'stopped-speed-strict-cleanup.patch')?.sha256
      === reviewedSourceIncrement.sourcePatchSha256, 'Finite reviewed source increment binding differs');
  check('scripts/core-source-provision.py', spec.provisionerSha256);
  for (const patch of source.patches) check(`scripts/libbox-patches/${patch.file}`, patch.sha256);
  for (const dep of dependencies) check(`scripts/libbox-patches/${dep.patchFile}`, dep.patchSha256);
  check('scripts/core-patches/windows-dns-refresh.patch', manifest.windowsBuild.patchSha256);

  return {
    policyState: 'SOURCE_FIRST_RELEASE_ELIGIBLE',
    reviewedCandidate,
    reviewedSourceIncrement,
    reviewedInputsSha256,
    sourceReceiptFingerprint: spec.sourceReceiptFingerprint,
    publicationRequirements: 'existing-signed-release-dag',
    nativeAcceptance: 'NotObserved',
    deviceAcceptance: 'NotObserved',
    Exact: 'Unknown',
    NoOwner: 'Unknown',
    managed: 'NotGranted',
  };
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
