#!/usr/bin/env node
// Freeze only verified selected bytes. Core producer identities are never rewritten.
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { writeSelectedCoreManifest, actualCoreFile, coreIdentityId, selectedCoreManifest } from './lib/core-identity.mjs';
import { hasDesktopRelease, releaseInstallationContract, verifyPackagedRelease, regularPath } from './desktop-core/release-consumer.mjs';
import { verifyPackagedSource } from './desktop-core/bundle.mjs';
import { verifyCronetPayload } from './lib/cronet-payload.mjs';
import { hasTemporaryDesktop, candidateBuildSelection, candidateInstallationContract, loadCandidate, verifiedCandidateSource, verifyCandidateAppCarrier, freezeCandidateBuildOutput } from './desktop-core/native-dns-candidate.mjs';
import { requireGraph as demand } from './desktop-core/source-graph.mjs';

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
const candidateMode = Object.hasOwn(process.env, 'POLARIS_NATIVE_DNS_CANDIDATE');
let candidateReceipt;
const temporaryDefault = hasTemporaryDesktop(input);
if (candidateMode || temporaryDefault) {
  if (candidateMode) demand(process.env.POLARIS_NATIVE_DNS_CANDIDATE === '1', 'Explicit candidate build mode required');
  const source = verifiedCandidateSource(root), binding = loadCandidate(root);
  candidateBuildSelection(source, root, platform, directory);
  contract = candidateInstallationContract(binding, source.bindingSha256);
  const original = join(root, 'resources', platform);
  const helperNames = [platform === 'win' ? 'polaris-helper.exe' : 'polaris-helper', ...(platform === 'win' ? ['polaris-cleaner.exe'] : [])];
  if (mode === 'prepare' && temporaryDefault && !candidateMode) {
    // Packaging has already staged (and, where required, signed) these exact
    // helper/cleaner bytes. Never replace them with a debug build here.
    regularPath(directory, true);
    demand(!existsSync(join(directory, 'candidate-carrier.json')) && !existsSync(join(directory, 'core-manifest.json')),
      'Default installation identity must be frozen exactly once');
    const built = Object.fromEntries(helperNames.map((name) =>
      [name, actualCoreFile(join(directory, name))]));
    candidateReceipt = { schema: 'polaris-native-dns-app-carrier-v1', source, built };
  } else if (mode === 'prepare') {
    // Freeze actual outputs from this checkout's preceding Cargo build once.
    // Workspace feature unification may subsequently rebuild target/debug; those
    // mutable outputs cannot redefine the already selected carrier identity.
    mkdirSync(join(root, 'resources/.native-dns-app'), { recursive: true });
    regularPath(join(root, 'resources/.native-dns-app'), true);
    mkdirSync(directory); // exclusive: no overlay onto an existing installation
    const copy = (from, to) => {
      regularPath(from, true);
      for (const entry of readdirSync(from, { withFileTypes: true })) {
        const src = join(from, entry.name), dst = join(to, entry.name);
        if (entry.isDirectory()) { regularPath(src, true); mkdirSync(dst); copy(src, dst); }
        else { regularPath(src); copyFileSync(src, dst); }
      }
    };
    copy(original, directory);
    const built = Object.fromEntries(helperNames.map((name) =>
      [name, freezeCandidateBuildOutput(root, directory, name)]));
    candidateReceipt = { schema: 'polaris-native-dns-app-carrier-v1', source, built };
    regularPath(directory, true);
    for (const [name, expected] of Object.entries(built)) actualCoreFile(join(directory, name), expected);
  } else {
    // Source/App/run is revalidated above, while every helper/cleaner byte is
    // checked against its frozen preparation receipt and the manifest below.
    verifyCandidateAppCarrier(directory, source, platform);
    regularPath(join(directory, 'core-manifest.json'));
  }
} else if (hasDesktopRelease(input)) {
  contract = releaseInstallationContract(root, input);
  // Verify the original acquisition/source/member/signature evidence as well
  // as the selected installation bytes; an environment path is never evidence.
  verifyPackagedRelease(root, input, platform, join(directory, contract.platforms[platform].core.member));
}
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
    if (platform === 'win' && actualCoreFile(join(directory, 'polaris-cleaner.exe')).sha256 !== old.core.cleanerSha256) throw Error('Final cleaner bytes differ from manifest');
    if (old.core.version !== contract.version || old.core.providerFingerprint !== row.providerFingerprint) throw Error('Final selected identity differs');
    console.log(`verified-core-id ${old.id}`);
    process.exit(0);
  }
}
const value = writeSelectedCoreManifest(directory, contract, platform, appVersion);
if (candidateReceipt) writeFileSync(join(directory, 'candidate-carrier.json'), JSON.stringify(candidateReceipt, null, 2) + '\n', { flag: 'wx' });
console.log(`selected-core-id ${value.id}`);
