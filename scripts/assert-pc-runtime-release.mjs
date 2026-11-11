#!/usr/bin/env node

// Publication requires the selected formal release contract.
// Native acceptance stays unobserved. The signed Release DAG still owns all CI,
// producer/consumer, tag, signature, asset-digest and promotion checks.
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadReleaseContract } from './desktop-core/release-consumer.mjs';
import { requireGraph } from './desktop-core/source-graph.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const readRepository = (relative) => readFileSync(join(root, relative));

export function assertSourceFirstRelease(read = readRepository) {
  const manifest = JSON.parse(read('src-tauri/core-manifest.json'));
  // loadReleaseContract admits only the exact {schema, contractPath, contractSha256} selection.
  const contract = loadReleaseContract(root, manifest);
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

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    requireGraph(process.argv.length === 2, 'Release policy takes no arguments or clearance overrides');
    console.log(JSON.stringify(assertSourceFirstRelease()));
  } catch (error) {
    console.error(`PC_RUNTIME_RELEASE_BLOCKED: ${error.message}`);
    process.exitCode = 1;
  }
}
