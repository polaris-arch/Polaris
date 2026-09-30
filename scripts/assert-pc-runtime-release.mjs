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
const reviewedCandidate = '123259cb4ee0eef484368e34d8ea7211d39964b6';
// Canonical core-manifest inputs at the reviewed candidate. Only five binary
// output pins are excluded: the existing all-four consumer verifies actual
// producer outputs for the current candidate/run/attempt, rather than old pins.
const reviewedInputsSha256 = 'c7dbacbebf895c6e340de34839103a4e5f48f2c08b9ec234f1c78b647f406cc7';
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
  check('scripts/core-source-provision.py', spec.provisionerSha256);
  for (const patch of source.patches) check(`scripts/libbox-patches/${patch.file}`, patch.sha256);
  for (const dep of dependencies) check(`scripts/libbox-patches/${dep.patchFile}`, dep.patchSha256);
  check('scripts/core-patches/windows-dns-refresh.patch', manifest.windowsBuild.patchSha256);

  return {
    policyState: 'SOURCE_FIRST_RELEASE_ELIGIBLE',
    reviewedCandidate,
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
