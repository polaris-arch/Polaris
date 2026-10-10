// Transport receipts are checked for consistency, not cryptographic provenance.
// Ordinary CI retrieves four exact-candidate native artifacts; only the reviewed
// validation-v2 envelope may preserve three explicitly pinned original origins.
import { execFileSync } from 'node:child_process';
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync,
  writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { buildInfoFingerprint, canonical, DESKTOP_TARGETS, digest, expectedTags, platformSourceIdentity, requireGraph, validateBuildInfo,
  desktopSourceManifestPath, desktopOverlays, validateMacCodeSignature, verifyMacCodeSignatureReceipt, validateSourceManifest, validateSourcePins, validateSourceReceipt, verifyHash } from './source-graph.mjs';

import { VALIDATION_SCHEMA, validateBundleOrigin } from './validation-origin.mjs';

export const coreFilename = (key) => key === 'win' ? 'sing-box.exe' : 'sing-box';

const receiptPath = (directory, key) => join(directory, key, `${coreFilename(key)}.source-receipt.json`);
const readJson = (path) => JSON.parse(readFileSync(path, 'utf8'));

export function writeBundleInventory(directory, candidate) {
  requireGraph(/^[a-f0-9]{40}$/.test(candidate ?? ''), 'Explicit candidate source SHA required');
  const platforms = {};
  for (const key of Object.keys(DESKTOP_TARGETS)) {
    const receipt = readJson(receiptPath(directory, key));
    requireGraph(receipt.candidate === candidate && receipt.platform === key, 'Producer candidate/platform differs');
    validateMacCodeSignature(receipt.macCodeSignature, key);
    const { fingerprint, ...facts } = receipt;
    requireGraph(fingerprint === digest(canonical(facts)), 'Producer receipt fingerprint differs');
    verifyHash(join(directory, key, coreFilename(key)), receipt.binarySha256);
    platforms[key] = { binarySha256: receipt.binarySha256, receiptSha256: digest(readFileSync(receiptPath(directory, key))) };
  }
  const inventory = { schema: 'polaris-desktop-bundle-v1', candidate, platforms };
  writeFileSync(join(directory, 'bundle.json'), `${JSON.stringify(inventory, null, 2)}\n`);
  return inventory;
}

export function consumeDesktopBundle(root, manifest, directory, candidate, keys = Object.keys(DESKTOP_TARGETS),
  run = execFileSync, host = process.platform) {
  const spec = validateSourcePins(manifest, 'linux', false);
  requireGraph(/^[a-f0-9]{40}$/.test(candidate ?? ''), 'Explicit candidate source SHA required');
  const gitOptions = { cwd: root, encoding: 'utf8', stdio: 'pipe', env: { ...process.env } };
  for (const name of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE']) delete gitOptions.env[name];
  requireGraph(String(run('git', ['rev-parse', 'HEAD'], gitOptions)).trim() === candidate
    && String(run('git', ['status', '--porcelain', '--untracked-files=no'], gitOptions)).trim() === '',
  'Consumer candidate checkout differs or has tracked modifications');
  requireGraph(keys.length > 0 && new Set(keys).size === keys.length
    && keys.every((key) => Object.hasOwn(DESKTOP_TARGETS, key)), 'Invalid selected desktop targets');
  const sourceManifest = join(root, desktopSourceManifestPath(spec));
  verifyHash(sourceManifest, spec.sourceManifestSha256);
  verifyHash(join(root, 'scripts/core-source-provision.py'), spec.provisionerSha256);
  const source = readJson(sourceManifest);
  validateSourceManifest(source, spec);
  const inventory = readJson(join(directory, 'bundle.json'));
  requireGraph(['polaris-desktop-bundle-v1', VALIDATION_SCHEMA].includes(inventory.schema) && inventory.candidate === candidate
    && canonical(Object.keys(inventory.platforms ?? {}).sort()) === canonical(Object.keys(DESKTOP_TARGETS).sort()),
  'Candidate bundle is missing an exact platform inventory');
  // Every selected subset still validates all four. No missing-platform skip.
  for (const key of Object.keys(DESKTOP_TARGETS)) {
    const binary = join(directory, key, coreFilename(key));
    const receiptFile = receiptPath(directory, key);
    const item = inventory.platforms[key];
    verifyHash(binary, item.binarySha256);
    verifyHash(receiptFile, item.receiptSha256);
    const receipt = readJson(receiptFile);
    const { fingerprint, ...facts } = receipt;
    validateBundleOrigin(root, inventory, candidate, key, receipt);
    const target = DESKTOP_TARGETS[key];
    const overlays = desktopOverlays(spec, key, manifest.windowsBuild?.patchSha256);
    if (overlays.length) verifyHash(join(root, 'scripts/core-patches/windows-dns-refresh.patch'), overlays[0].sha256);
    requireGraph(receipt.schema === 'polaris-desktop-core-v1'
      && receipt.platform === key && receipt.version === spec.version && receipt.binarySha256 === item.binarySha256
      && receipt.buildTree === spec.platforms[key].buildTree && fingerprint === digest(canonical(facts))
      && receipt.goos === target.goos && receipt.goarch === target.goarch && receipt.cgo === target.cgo
      && receipt.mainGoModSha256 === receipt.sourceReceipt?.mainGoModSha256
      && receipt.mainGoSumSha256 === receipt.sourceReceipt?.mainGoSumSha256
      && canonical(receipt.tags) === canonical(expectedTags(key))
      && canonical(receipt.overlays) === canonical(overlays), 'Platform receipt binding differs');
    validateMacCodeSignature(receipt.macCodeSignature, key);
    if (host === 'darwin' && key.startsWith('mac-')) {
      verifyMacCodeSignatureReceipt(binary, receipt, key, run === execFileSync ? undefined : run);
    }
    validateSourceReceipt(receipt.sourceReceipt, source, spec);
    const identity = platformSourceIdentity(receipt.sourceReceipt, source, spec, key, manifest.windowsBuild?.patchSha256);
    requireGraph(receipt.sourceFingerprint === identity.sourceFingerprint
      && receipt.platformInputFingerprint === identity.platformInputFingerprint && receipt.buildID === identity.buildID,
      'Platform source fingerprint differs');
    const buildInfo = String(run('go', ['version', '-m', binary], {
      encoding: 'utf8', stdio: 'pipe', env: { ...process.env, GOTOOLCHAIN: 'local', GOWORK: 'off' },
    })).trim();
    const linked = validateBuildInfo(buildInfo, source, key, spec);
    requireGraph(canonical(Object.fromEntries(linked.modules)) === canonical(receipt.linkedModules), 'Actual linked modules differ from receipt');
    requireGraph(String(run('go', ['tool', 'buildid', binary], {
      encoding: 'utf8', stdio: 'pipe', env: { ...process.env, GOTOOLCHAIN: 'local', GOWORK: 'off' },
    })).trim() === identity.buildID, 'Actual artifact source buildID differs');
    requireGraph(buildInfoFingerprint(buildInfo) === receipt.buildInfoSha256, 'Actual embedded build information differs from receipt');
  }
  // Only after all four are checked, publish selected resources atomically.
  for (const key of keys) {
    const dest = join(root, 'resources', key, coreFilename(key));
    mkdirSync(dirname(dest), { recursive: true });
    const staging = mkdtempSync(join(dirname(dest), '.polaris-core-'));
    try {
      const staged = join(staging, 'binary');
      copyFileSync(join(directory, key, coreFilename(key)), staged);
      verifyHash(staged, inventory.platforms[key].binarySha256);
      if (key !== 'win') chmodSync(staged, 0o755);
      renameSync(staged, dest);
    } finally { rmSync(staging, { recursive: true, force: true }); }
  }
  // Metadata stays outside platform payload directories. Producer artifacts use
  // visible filenames; CI transport must preserve them and the complete bundle.
  const metadata = join(root, 'resources/.source-receipts');
  mkdirSync(metadata, { recursive: true });
  for (const key of Object.keys(DESKTOP_TARGETS)) copyFileSync(receiptPath(directory, key), join(metadata, `${key}.json`));
  copyFileSync(join(directory, 'bundle.json'), join(metadata, 'bundle.json'));
  return inventory;
}

export function verifyPackagedSource(root, manifest, key, binary, run, host = process.platform) {
  const spec = validateSourcePins(manifest, key, false);
  const receipt = readJson(join(root, 'resources/.source-receipts', `${key}.json`));
  const inventory = readJson(join(root, 'resources/.source-receipts/bundle.json'));
  const { fingerprint, ...facts } = receipt;
  requireGraph(['polaris-desktop-bundle-v1', VALIDATION_SCHEMA].includes(inventory.schema)
    && canonical(Object.keys(inventory.platforms ?? {}).sort()) === canonical(Object.keys(DESKTOP_TARGETS).sort())
    && /^[a-f0-9]{40}$/.test(inventory.candidate ?? ''),
  'Packaging source bundle candidate/inventory differs');
  validateBundleOrigin(root, inventory, inventory.candidate, key, receipt);
  if (inventory.schema === VALIDATION_SCHEMA) requireGraph(String((run ?? execFileSync)('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' })).trim() === inventory.candidate, 'Packaging validation checkout differs');
  verifyHash(join(root, 'resources/.source-receipts', `${key}.json`), inventory.platforms[key].receiptSha256);
  const sourceManifest = join(root, desktopSourceManifestPath(spec));
  verifyHash(sourceManifest, spec.sourceManifestSha256);
  verifyHash(join(root, 'scripts/core-source-provision.py'), spec.provisionerSha256);
  const source = readJson(sourceManifest);
  validateSourceManifest(source, spec);
  validateSourceReceipt(receipt.sourceReceipt, source, spec);
  const identity = platformSourceIdentity(receipt.sourceReceipt, source, spec, key, manifest.windowsBuild?.patchSha256);
  requireGraph(receipt.schema === 'polaris-desktop-core-v1' && receipt.platform === key
    && receipt.version === spec.version && receipt.buildTree === spec.platforms[key].buildTree
    && receipt.binarySha256 === inventory.platforms[key].binarySha256
    && canonical(receipt.overlays) === canonical(desktopOverlays(spec, key, manifest.windowsBuild?.patchSha256))
    && receipt.sourceFingerprint === identity.sourceFingerprint && receipt.buildID === identity.buildID
    && receipt.platformInputFingerprint === identity.platformInputFingerprint
    && receipt.mainGoModSha256 === receipt.sourceReceipt?.mainGoModSha256
    && receipt.mainGoSumSha256 === receipt.sourceReceipt?.mainGoSumSha256
    && fingerprint === digest(canonical(facts)), 'Packaged source receipt binding differs');
  validateMacCodeSignature(receipt.macCodeSignature, key);
  verifyHash(binary, receipt.binarySha256);
  if (host === 'darwin' && key.startsWith('mac-')) verifyMacCodeSignatureReceipt(binary, receipt, key, run);
}
