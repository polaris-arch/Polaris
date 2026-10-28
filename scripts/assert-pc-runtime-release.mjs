#!/usr/bin/env node

// Source-first publication accepts this reviewed source closure only. Native
// acceptance remains unobserved. The signed Release DAG still owns all CI,
// producer/consumer, tag, signature, asset-digest and promotion checks.
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonical, DESKTOP_TARGETS, digest, requireGraph, validateSourceManifest,
  validateSourcePins } from './desktop-core/source-graph.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
// Historical reviewed backend anchor, not a commit claiming the later Go inputs.
const reviewedCandidate = '123259cb4ee0eef484368e34d8ea7211d39964b6';
// This closure includes the previously accepted shared Go TS writer delta and
// the accepted validation retirement and target-membership deltas. The latest
// implementation/review hashes below identify the final Membership record.
// Retirement implementation 6ee4c9f0fa982dc5cc691e6fc7babd3c78b174a799d62b64ce6cce296c7eb9e3,
// review f09f2f6a6a602d3e650980881abba1fb7ed85d6c072419f8a13936b2895ef846.
// Earlier shared Go review 3579c1d6e782ff53142a60ac5c279fa9ed263bdc9147eaf406b8a7088eaa27ba
// remains the base anchor. None of these records is a new candidate commit,
// native acceptance or global ownership clearance.
const reviewedSourceIncrement = Object.freeze({
  scope: 'finite-ts-store-writer-retirement-source-only',
  implementationSha256: '317f0dbe99b89bc018cfe9af7dfc0a0d2536d166566b1ff6a512d5285c2f9c67',
  independentReviewSha256: '26e53101a2510edaf9038e3e5d75b1e982ffcada2a540ede0c38a57971005553',
  sourcePatchSha256: '839cfff957264aec8d4a85bb9e4efd272c4cfc6ad08de5eeb33d4c8f8c6e55b5',
  sourceManifestSha256: '9f269083f002cc7dea49e5151c4e150e6ba1d26dc173d4601614dd2cb557b7fd',
  sourceReceiptFingerprint: '287b3d7a85b777e1a3f379f05171f26ee1e4f96803c1f4aaabe3e8116dc21b54',
});
// Canonical inputs bind these accepted finite deltas and the unchanged backend graph,
// plus the original Windows overlay replay and internal .polaris.3 version.
// Source-only pins do not claim newly compiled output. Only five binary
// output pins are excluded: the existing all-four consumer verifies actual
// producer outputs for the current candidate/run/attempt, rather than old pins.
const reviewedInputsSha256 = '09ebeea39925eb302f70d1cafbbd4eb6b6e5a63c1ebb3e4d05b8125e26ba53dc';
const readRepository = (relative) => readFileSync(join(root, relative));

export function assertSourceFirstRelease(read = readRepository) {
  const manifest = JSON.parse(read('src-tauri/core-manifest.json'));
  for (const key of Object.keys(DESKTOP_TARGETS)) validateSourcePins(manifest, key, false);
  const inputs = structuredClone(manifest);
  delete inputs.windowsBuild.binarySha256;
  for (const platform of Object.values(inputs.sourceBuild.platforms)) delete platform.binarySha256;
  requireGraph(digest(canonical(inputs)) === reviewedInputsSha256,
    'Source inputs differ from the reviewed release policy; source review must be renewed');

  const spec = manifest.sourceBuild;
  const check = (relative, expected) => {
    const bytes = read(relative);
    requireGraph(digest(bytes) === expected, `Reviewed source SHA-256 mismatch: ${relative}`);
    return bytes;
  };
  const sourcePath = 'scripts/libbox-patches/source-manifest.json';
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
