// Verify the currently selected published core before the managed-mesh tests
// execute its version/check/format commands. The production consumer owns all
// archive, provider, module, BuildID and receipt checks.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadReleaseContract, releaseInstallationContract, verifyPackagedRelease } from './release-consumer.mjs';
import { requireGraph as demand } from './source-graph.mjs';

export function managedCheckPlatform(host) {
  const key = { 'linux/x64': 'linux', 'win32/x64': 'win',
    'darwin/x64': 'mac-x64', 'darwin/arm64': 'mac-arm64' }[`${host.os}/${host.arch}`];
  demand(key, 'Managed config check requires a supported native desktop host');
  return key;
}

function releaseVersionOutput(row) {
  demand(typeof row?.version === 'string' && typeof row.goVersion === 'string'
    && typeof row.target === 'string' && typeof row.tags === 'string'
    && ['0', '1'].includes(row.buildGraph?.settings?.CGO_ENABLED), 'Release version inputs differ');
  return `sing-box version ${row.version}\n\nEnvironment: ${row.goVersion} ${row.target}\n`
    + `Tags: ${row.tags}\nRevision: ${row.sourceCommit}\nCGO: ${row.buildGraph.settings.CGO_ENABLED === '1' ? 'enabled' : 'disabled'}\n`;
}

export function managedFormalSelection(root, manifest, host) {
  const contract = loadReleaseContract(root, manifest), key = managedCheckPlatform(host), row = contract.platforms[key];
  const expected = releaseVersionOutput(row);
  demand(expected.split('\n').includes(`sing-box version ${contract.version}`)
    && expected.split('\n').includes(`Revision: ${contract.sourceCommit}`), 'Producer version/Revision binding differs');
  return { contract, key, row, expected };
}

export function verifyManagedMeshCore(root, manifest, core, run = execFileSync, host = { os: process.platform, arch: process.arch }) {
  const { key, row, expected } = managedFormalSelection(root, manifest, host);
  verifyPackagedRelease(root, manifest, key, core, run, host.os);
  // execFileSync throws for every nonzero status; wrong bytes are rejected above
  // before even version is executed. Match the full original observation.
  const output = run(core, ['version'], { encoding: 'utf8' });
  demand(output === expected, 'Managed mesh core version output differs');
  return { selection: 'formal-desktop-default', platform: key,
    version: row.version, sourceCommit: row.sourceCommit, sourceTree: row.sourceTree,
    providerFingerprint: releaseInstallationContract(root, manifest).platforms[key].providerFingerprint,
    buildID: row.buildID, coreSha256: row.core.sha256 };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    demand(process.argv.length === 3, 'Exactly one managed mesh core path required');
    const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
    const manifest = JSON.parse(readFileSync(resolve(root, 'src-tauri/core-manifest.json'), 'utf8'));
    console.log(JSON.stringify(verifyManagedMeshCore(root, manifest, resolve(process.argv[2]))));
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
