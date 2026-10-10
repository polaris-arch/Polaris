// Inspect current source bindings and actual payload metadata before native use.
import { execFileSync } from 'node:child_process';
import { lstatSync, readFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { verifyPackagedSource } from './desktop-core/bundle.mjs';
import { validationContext, VALIDATION_SCHEMA } from './desktop-core/validation-origin.mjs';
import { buildInfoFingerprint, canonical, desktopSourceManifestPath, digest, requireGraph,
  validateBuildInfo, validateSourcePins, verifyHash } from './desktop-core/source-graph.mjs';
import { verifyCronetPayload } from './lib/cronet-payload.mjs';

function regular(path) {
  let cursor = resolve(path);
  for (;;) {
    const info = lstatSync(cursor);
    requireGraph(cursor === resolve(path) ? info.isFile() : info.isDirectory(), `Payload link/non-regular ancestor: ${cursor}`);
    const parent = dirname(cursor); if (parent === cursor) break; cursor = parent;
  }
}

export function inspectNativePayload(root, key, core, library, run = execFileSync) {
  requireGraph(key === 'linux' || key === 'win', 'Dynamic payload requires Linux/Windows');
  core = resolve(core); library = resolve(library); regular(core); regular(library);
  requireGraph(basename(core) === (key === 'win' ? 'sing-box.exe' : 'sing-box')
    && basename(library) === (key === 'win' ? 'libcronet.dll' : 'libcronet.so')
    && dirname(core) === dirname(library), 'Canonical colocated payload required');
  const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json')));
  const spec = validateSourcePins(manifest, key, false);
  const receipt = JSON.parse(readFileSync(join(root, `resources/.source-receipts/${key}.json`)));
  const inventory = JSON.parse(readFileSync(join(root, 'resources/.source-receipts/bundle.json')));
  requireGraph(inventory.schema === VALIDATION_SCHEMA, 'Native validation requires explicit origin envelope');
  validationContext(inventory.candidate);
  const options = { cwd: root, encoding: 'utf8', env: { ...process.env } };
  for (const key of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE']) delete options.env[key];
  requireGraph(String(run('git', ['rev-parse', 'HEAD'], options)).trim() === inventory.candidate
    && String(run('git', ['status', '--porcelain', '--untracked-files=no'], options)).trim() === '', 'Native input checkout differs');
  verifyPackagedSource(root, manifest, key, core, run);
  const sourcePath = join(root, desktopSourceManifestPath(spec)); verifyHash(sourcePath, spec.sourceManifestSha256);
  const source = JSON.parse(readFileSync(sourcePath));
  const goOptions = { encoding: 'utf8', env: { ...process.env, GOTOOLCHAIN: 'local', GOWORK: 'off' } };
  const raw = String(run('go', ['version', '-m', core], goOptions)).trim();
  const actual = validateBuildInfo(raw, source, key, spec);
  requireGraph(buildInfoFingerprint(raw) === receipt.buildInfoSha256
    && canonical(Object.fromEntries(actual.modules)) === canonical(receipt.linkedModules)
    && String(run('go', ['tool', 'buildid', core], goOptions)).trim() === receipt.buildID, 'Native actual core metadata differs');
  const sourceCore = join(root, 'resources', key, basename(core)); regular(sourceCore);
  requireGraph((lstatSync(core).mode & 0o7777) === (lstatSync(sourceCore).mode & 0o7777), 'Final core permissions differ');
  const cronet = verifyCronetPayload(join(root, 'resources', key, basename(library)), library, key, manifest);
  const module = `github.com/sagernet/cronet-go/lib/${key === 'win' ? 'windows' : 'linux'}_amd64`;
  requireGraph(actual.modules.has(module), 'Actual core lacks platform Cronet module');
  return { schema: 'polaris-native-payload-input-v1', candidate: inventory.candidate, platform: key,
    core: { path: core, sha256: digest(readFileSync(core)), mode: lstatSync(core).mode & 0o7777,
      buildID: receipt.buildID, buildInfoSha256: receipt.buildInfoSha256, goVersion: actual.goVersion },
    library: { path: library, ...cronet, module, moduleIdentity: actual.modules.get(module) },
    source: { commit: source.sourceCommit, tag: source.sourceTag, tagObject: source.sourceTagObject,
      tree: spec.buildTree, manifestSha256: spec.sourceManifestSha256,
      mainGoModSha256: receipt.mainGoModSha256, mainGoSumSha256: receipt.mainGoSumSha256,
      sourceFingerprint: receipt.sourceFingerprint, platformInputFingerprint: receipt.platformInputFingerprint,
      version: receipt.version, goos: receipt.goos, goarch: receipt.goarch, cgo: receipt.cgo, tags: receipt.tags },
    receiptSha256: inventory.platforms[key].receiptSha256, origin: inventory.origins[key] };
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const [key, core, library] = process.argv.slice(2);
  requireGraph(process.argv.length === 5, 'Usage: native-payload-input.mjs linux|win CORE LIBRARY');
  const root = join(dirname(fileURLToPath(import.meta.url)), '..');
  console.log(JSON.stringify(inspectNativePayload(root, key, core, library)));
}
