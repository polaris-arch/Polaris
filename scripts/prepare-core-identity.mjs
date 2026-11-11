#!/usr/bin/env node
// Freeze only verified selected bytes. Core producer identities are never rewritten.
import { prepareLocalInstallation, rejectLocalDistribution } from './desktop-core/local-development.mjs';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { writeSelectedCoreManifest, actualCoreFile, coreIdentityId, selectedCoreManifest } from './lib/core-identity.mjs';
import { releaseInstallationContract, verifyPackagedRelease } from './desktop-core/release-consumer.mjs';
import { requireGraph as demand } from './desktop-core/source-graph.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const json = (path) => JSON.parse(readFileSync(path, 'utf8'));
const flags = new Map();
for (const argument of process.argv.slice(2)) {
  const match = /^--(platform|directory|mode|helper-binary|selection)=(.+)$/.exec(argument);
  if (!match || flags.has(match[1])) throw Error('Expected unique --platform, --directory and --mode arguments');
  flags.set(match[1], match[2]);
}
const platform = flags.get('platform');
if (!['linux', 'win', 'mac-x64', 'mac-arm64'].includes(platform)) throw Error('Exact desktop platform required');
const directory = resolve(flags.get('directory') ?? join(root, 'resources', platform));
const mode = flags.get('mode') ?? 'prepare';
if (!['prepare', 'verify', 'maintenance-anchor'].includes(mode)) throw Error('Unknown core identity mode');
if (mode === 'maintenance-anchor') demand(platform === 'win' && flags.has('helper-binary'), 'Exact Windows helper build input required');
else demand(!flags.has('helper-binary'), 'Helper override is only a producer anchor input');
const selection = flags.get('selection') ?? 'formal';
if (selection === 'local-development') {
  demand(platform === 'linux' && !flags.has('helper-binary'), 'Local development selection is Linux-only');
  prepareLocalInstallation(root, directory, mode);
  console.log('Verified compiled local development installation');
  process.exit(0);
}
demand(selection === 'formal', 'Unknown compiler selection');
rejectLocalDistribution(root, directory);
const input = json(join(root, 'src-tauri/core-manifest.json'));
const appVersion = json(join(root, 'src-tauri/tauri.conf.json')).version;
// Verify the original acquisition/member/signature evidence as well as the
// selected installation bytes; an environment path is never evidence.
const contract = releaseInstallationContract(root, input);
verifyPackagedRelease(root, input, platform, join(directory, contract.platforms[platform].core.member));
if (mode === 'maintenance-anchor') {
  const selected = selectedCoreManifest(contract, platform, appVersion);
  for (const file of selected.core.files) actualCoreFile(join(directory, file.name), file);
  const helper = actualCoreFile(resolve(flags.get('helper-binary')));
  process.stdout.write(JSON.stringify({ core_id: selected.id, helper_sha256: helper.sha256 }));
  process.exit(0);
}
function checkBinAnchor(value) {
  if (platform !== 'win' || !Object.hasOwn(process.env, 'POLARIS_WINDOWS_INSTALL_ANCHOR')) return;
  const anchor = JSON.parse(process.env.POLARIS_WINDOWS_INSTALL_ANCHOR);
  demand(Object.keys(anchor).sort().join(',') === 'core_id,helper_sha256'
    && anchor.core_id === value.id && anchor.helper_sha256 === value.core.helperSha256,
  'Final installation differs from the cleaner BIN producer anchor');
}
// Reject a mismatched producer anchor before publishing the full manifest.
if (platform === 'win' && Object.hasOwn(process.env, 'POLARIS_WINDOWS_INSTALL_ANCHOR')) {
  checkBinAnchor(selectedCoreManifest(contract, platform, appVersion,
    actualCoreFile(join(directory, 'polaris-helper.exe')).sha256));
}
if (mode !== 'prepare') {
  const old = json(join(directory, 'core-manifest.json'));
  if (old.schema !== 'polaris-core-manifest-v1' || old.id !== coreIdentityId(old.core) || old.id !== selectedCoreManifest(contract, platform, appVersion).id || old.core.platform !== platform || old.appVersion !== appVersion) throw Error('Existing installation manifest differs');
  // Both helper and core bytes were frozen before App compilation. Nothing
  // transforms either content identity after the App's expectation is embedded.
  if (mode === 'verify') {
    const row = contract.platforms[platform];
    for (const expected of [row.core, ...(row.sidecar ? [row.sidecar] : [])]) actualCoreFile(join(directory, expected.member), expected);
    const helper = platform === 'win' ? 'polaris-helper.exe' : 'polaris-helper';
    if (actualCoreFile(join(directory, helper)).sha256 !== old.core.helperSha256) throw Error('Final helper bytes differ from manifest');
    if (platform === 'win' && actualCoreFile(join(directory, 'polaris-cleaner.exe')).sha256 !== old.core.cleanerSha256) throw Error('Final cleaner bytes differ from manifest');
    if (old.core.version !== contract.version || old.core.providerFingerprint !== row.providerFingerprint) throw Error('Final selected identity differs');
    checkBinAnchor(old);
    console.log(`verified-core-id ${old.id}`);
    process.exit(0);
  }
}
const value = writeSelectedCoreManifest(directory, contract, platform, appVersion);
checkBinAnchor(value);
console.log(`selected-core-id ${value.id}`);
