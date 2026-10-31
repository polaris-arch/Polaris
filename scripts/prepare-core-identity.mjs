#!/usr/bin/env node
// Freeze only verified selected bytes. Core producer identities are never rewritten.
import { prepareLocalInstallation, rejectLocalDistribution } from './desktop-core/local-development.mjs';
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
  if (mode === 'maintenance-anchor') {
    // Source/provider verification above is identical. No carrier/manifest is
    // published before the cleaner BIN exists; this does not weaken prepare.
  } else if (mode === 'prepare' && temporaryDefault && !candidateMode) {
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
if (mode === 'maintenance-anchor') {
  const selected = selectedCoreManifest(contract, platform, appVersion);
  // Candidate App carrier does not exist yet. The verified original producer
  // directory supplies these same selected core/sidecar objects.
  const sourceDirectory = candidateMode || temporaryDefault ? join(root, 'resources', platform) : directory;
  for (const file of selected.core.files) actualCoreFile(join(sourceDirectory, file.name), file);
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
if (candidateReceipt) writeFileSync(join(directory, 'candidate-carrier.json'), JSON.stringify(candidateReceipt, null, 2) + '\n', { flag: 'wx' });
console.log(`selected-core-id ${value.id}`);
