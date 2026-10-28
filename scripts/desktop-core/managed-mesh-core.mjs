// Verify the currently selected published core before the managed-mesh tests
// execute its version/check/format commands. The production consumer owns all
// archive, provider, module, BuildID, receipt and App candidate checks.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadReleaseContract, releaseInstallationContract, verifyPackagedRelease } from './release-consumer.mjs';
import { isUpstreamRelease } from './upstream-release.mjs';
import { requireGraph as demand } from './source-graph.mjs';
import { candidateContext, hasTemporaryDesktop, loadTemporaryDesktopSelection, candidateVersionOutput, verifyCandidatePayload } from './native-dns-candidate.mjs';

export function managedCheckPlatform(host) {
  const key = { 'linux/x64': 'linux', 'win32/x64': 'win',
    'darwin/x64': 'mac-x64', 'darwin/arm64': 'mac-arm64' }[`${host.os}/${host.arch}`];
  demand(key, 'Managed config check requires a supported native desktop host');
  return key;
}

export function managedCheckSelection(context, manual) {
  // A manual acquisition has the original context without a mode field, even
  // when the same checkout also commits a temporary desktop default.
  demand(context && typeof context === 'object'
    && (manual ? !Object.hasOwn(context, 'mode') : context.mode === 'temporary-default'),
  'Managed check acquisition mode differs from admitted selection');
  return context.mode === 'temporary-default' ? 'temporary-desktop-default' : 'native-dns-candidate';
}

export function managedFormalSelection(root, manifest, host) {
  const contract = loadReleaseContract(root, manifest), upstream = isUpstreamRelease(contract);
  if (!upstream) demand(host.os === 'linux' && host.arch === 'x64', 'Managed mesh core requires native Linux amd64');
  const key = upstream ? managedCheckPlatform(host) : 'linux', row = contract.platforms[key];
  const expected = upstream ? candidateVersionOutput(row) : row.actualVersionOutput;
  demand(typeof expected === 'string'
    && expected.split('\n').includes(`sing-box version ${contract.version}`)
    && expected.split('\n').includes(`Revision: ${contract.sourceCommit}`), 'Producer version/Revision binding differs');
  return { contract, upstream, key, row, expected };
}

export function verifyManagedMeshCore(root, manifest, core, run = execFileSync, host = { os: process.platform, arch: process.arch }, env = process.env) {
  if (Object.hasOwn(env, 'POLARIS_NATIVE_DNS_CANDIDATE') || hasTemporaryDesktop(manifest)) {
    const key = managedCheckPlatform(host);
    const manual = Object.hasOwn(env, 'POLARIS_NATIVE_DNS_CANDIDATE');
    if (manual) candidateContext(env.CORE_CANDIDATE, env, root, run);
    else loadTemporaryDesktopSelection(root, manifest);
    const { binding, row, receipt } = verifyCandidatePayload(root, key, core, run);
    const selection = managedCheckSelection(receipt.context, manual);
    const output = run(core, ['version'], { encoding: 'utf8', timeout: 15_000, maxBuffer: 1024 * 1024 });
    demand(output === candidateVersionOutput(row), 'Candidate managed mesh version/Revision output differs');
    return { selection, platform: key, version: row.version,
      sourceCommit: binding.sourceCommit, sourceTree: binding.sourceTree,
      bindingSha256: receipt.bindingSha256, buildID: row.buildID, coreSha256: row.core.sha256 };
  }
  // Historical v1 evidence retains its original scope. A fully verified
  // upstream formal contract selects its own native row on all four hosts.
  const { upstream, key, row, expected } = managedFormalSelection(root, manifest, host);
  verifyPackagedRelease(root, manifest, key, core, run, host.os);
  // execFileSync throws for every nonzero status; wrong bytes are rejected above
  // before even version is executed. Match the full original observation.
  const output = run(core, ['version'], { encoding: 'utf8' });
  demand(output === expected, 'Managed mesh core version output differs');
  return { ...(upstream ? { selection: 'formal-desktop-default', platform: key } : {}),
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
