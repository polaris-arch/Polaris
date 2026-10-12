#!/usr/bin/env node
// Freeze only verified selected bytes. Core producer identities are never rewritten.
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { writeSelectedCoreManifest, actualCoreFile, coreIdentityId, selectedCoreManifest } from './lib/core-identity.mjs';
import { hasDesktopRelease, loadReleaseContract } from './desktop-core/release-consumer.mjs';
import { verifyPackagedSource } from './desktop-core/bundle.mjs';
import { verifyCronetPayload } from './lib/cronet-payload.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const json = (path) => JSON.parse(readFileSync(path, 'utf8'));
const flags = new Map();
for (const argument of process.argv.slice(2)) {
  const match = /^--(platform|directory|mode)=(.+)$/.exec(argument);
  if (!match || flags.has(match[1])) throw Error('Expected unique --platform, --directory and --mode arguments');
  flags.set(match[1], match[2]);
}
const platform = flags.get('platform');
if (!['linux', 'win', 'mac-x64', 'mac-arm64'].includes(platform)) throw Error('Exact desktop platform required');
const directory = resolve(flags.get('directory') ?? join(root, 'resources', platform));
const mode = flags.get('mode') ?? 'prepare';
if (!['prepare', 'verify'].includes(mode)) throw Error('Unknown core identity mode');
const input = json(join(root, 'src-tauri/core-manifest.json'));
const appVersion = json(join(root, 'src-tauri/tauri.conf.json')).version;
let contract;
if (hasDesktopRelease(input)) contract = loadReleaseContract(root, input);
else {
  // Preserve the pure upstream testing/source lane and its own exact receipts;
  // it must not borrow the production fork provider identity or binary pins.
  const binary = platform === 'win' ? 'sing-box.exe' : 'sing-box';
  verifyPackagedSource(root, input, platform, join(directory, binary));
  const receipt = json(join(root, 'resources/.source-receipts', `${platform}.json`));
  const actual = actualCoreFile(join(directory, binary));
  if (actual.sha256 !== receipt.binarySha256) throw Error('Source receipt binary differs');
  const core = { member: binary, ...actual };
  let sidecar = null;
  if (platform === 'linux' || platform === 'win') {
    const member = platform === 'linux' ? 'libcronet.so' : 'libcronet.dll';
    const facts = verifyCronetPayload(join(root, 'resources', platform, member), join(directory, member), platform, input);
    sidecar = { member, bytes: facts.bytes, sha256: facts.sha256 };
  }
  contract = { version: receipt.version, platforms: { [platform]: { providerFingerprint: receipt.sourceFingerprint, core, sidecar } } };
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
    if (old.core.version !== contract.version || old.core.providerFingerprint !== row.providerFingerprint) throw Error('Final selected identity differs');
    console.log(`verified-core-id ${old.id}`);
    process.exit(0);
  }
}
const value = writeSelectedCoreManifest(directory, contract, platform, appVersion);
console.log(`selected-core-id ${value.id}`);
