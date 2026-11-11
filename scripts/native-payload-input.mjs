// Inspect the committed release binding and actual payload metadata before native use.
import { execFileSync } from 'node:child_process';
import { lstatSync, readFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { releaseGoVersion, requireGraph } from './desktop-core/source-graph.mjs';
import { verifyCronetPayload } from './lib/cronet-payload.mjs';
import { loadReleaseContract, verifyPackagedRelease } from './desktop-core/release-consumer.mjs';

function regular(path) {
  let cursor = resolve(path);
  for (;;) {
    const info = lstatSync(cursor);
    requireGraph(cursor === resolve(path) ? info.isFile() : info.isDirectory(), `Payload link/non-regular ancestor: ${cursor}`);
    const parent = dirname(cursor); if (parent === cursor) break; cursor = parent;
  }
}

export function nativeReleaseGoVersion(contract, row) {
  requireGraph(row?.schema === 'polaris-upstream-native-receipt-v1', 'Native release receipt schema differs');
  requireGraph(row.goVersion === releaseGoVersion(contract), 'Native release receipt Go version differs');
  return row.goVersion;
}

export function inspectNativePayload(root, key, core, library, run = execFileSync) {
  requireGraph(key === 'linux' || key === 'win', 'Dynamic payload requires Linux/Windows');
  core = resolve(core); library = resolve(library); regular(core); regular(library);
  requireGraph(basename(core) === (key === 'win' ? 'sing-box.exe' : 'sing-box')
    && basename(library) === (key === 'win' ? 'libcronet.dll' : 'libcronet.so')
    && dirname(core) === dirname(library), 'Canonical colocated payload required');
  requireGraph(((lstatSync(core).mode | lstatSync(library).mode) & 0o7000) === 0, 'Special payload permissions forbidden');
  const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json')));
  const contract = loadReleaseContract(root, manifest);
  const receipt = verifyPackagedRelease(root, manifest, key, core, run);
  const row = contract.platforms[key];
  const cronet = verifyCronetPayload(join(root, 'resources', key, basename(library)), library, key, contract);
  const module = `github.com/sagernet/cronet-go/lib/${key === 'win' ? 'windows' : 'linux'}_amd64`;
  const acquisition = row.buildGraph.prebuiltAcquisitionModules.filter((d) => d.module === module);
  requireGraph(acquisition.length === 1, 'Release core lacks exact platform Cronet acquisition module');
  const candidate = String(run('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' })).trim();
  return { schema: 'polaris-native-release-payload-input-v1', candidate, platform: key,
    core: { path: core, sha256: row.core.sha256, mode: lstatSync(core).mode & 0o7777,
      buildID: row.buildID, goVersion: nativeReleaseGoVersion(contract, row) },
    library: { path: library, ...cronet, module, moduleIdentity: acquisition[0] },
    producer: row, consumerReceiptFingerprint: receipt.fingerprint,
    origin: { kind: 'published-fork-release', contractSha256: receipt.contractSha256,
      releaseId: contract.releaseId, tagObject: contract.tagObject, releaseAsset: row.releaseAsset },
    actualAppAcceptance: receipt.actualAppAcceptance };
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const [key, core, library] = process.argv.slice(2);
  requireGraph(process.argv.length === 5, 'Usage: native-payload-input.mjs linux|win CORE LIBRARY');
  const root = join(dirname(fileURLToPath(import.meta.url)), '..');
  console.log(JSON.stringify(inspectNativePayload(root, key, core, library)));
}
