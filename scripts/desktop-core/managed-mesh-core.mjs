// Verify the currently selected published core before the managed-mesh tests
// execute its version/check/format commands. The production consumer owns all
// archive, provider, module, BuildID, receipt and App candidate checks.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadReleaseContract, verifyPackagedRelease } from './release-consumer.mjs';
import { requireGraph as demand } from './source-graph.mjs';

export function verifyManagedMeshCore(root, manifest, core, run = execFileSync, host = { os: process.platform, arch: process.arch }) {
  demand(host.os === 'linux' && host.arch === 'x64', 'Managed mesh core requires native Linux amd64');
  const contract = loadReleaseContract(root, manifest), row = contract.platforms.linux;
  verifyPackagedRelease(root, manifest, 'linux', core, run, host.os);
  demand(typeof row.actualVersionOutput === 'string'
    && row.actualVersionOutput.split('\n').includes(`sing-box version ${contract.version}`)
    && row.actualVersionOutput.split('\n').includes(`Revision: ${contract.sourceCommit}`), 'Producer version/Revision binding differs');
  // execFileSync throws for every nonzero status; wrong bytes are rejected above
  // before even version is executed. Match the full original observation.
  const output = run(core, ['version'], { encoding: 'utf8' });
  demand(output === row.actualVersionOutput, 'Managed mesh core version output differs');
  return { version: row.version, sourceCommit: row.sourceCommit, sourceTree: row.sourceTree,
    providerFingerprint: row.providerFingerprint, buildID: row.buildID, coreSha256: row.core.sha256 };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    demand(process.argv.length === 3, 'Exactly one managed mesh core path required');
    const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
    const manifest = JSON.parse(readFileSync(resolve(root, 'src-tauri/core-manifest.json'), 'utf8'));
    console.log(JSON.stringify(verifyManagedMeshCore(root, manifest, resolve(process.argv[2]))));
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
