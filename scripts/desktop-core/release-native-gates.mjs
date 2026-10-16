// Current-attempt native inspection of immutable published inputs. These gates
// record App acquisition/inspection; they never relabel the fork producer.
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonical, digest, requireGraph as demand } from './source-graph.mjs';
import { contractPublicAssets, validateUpstreamReadback } from './upstream-release.mjs';
import { ADAPTER_FILES, consumeRelease, loadReleaseContract, portableCanonical,
  regularPath, releaseRepositoryId, RELEASE_TARGETS, verifyPackagedRelease } from './release-consumer.mjs';

const HOSTS = { linux: ['linux', 'x64'], win: ['win32', 'x64'],
  'mac-x64': ['darwin', 'x64'], 'mac-arm64': ['darwin', 'arm64'] };
const same = (a, b, reason) => demand(canonical(a) === canonical(b), reason);
const parse = (path) => { regularPath(path); return JSON.parse(readFileSync(path, 'utf8')); };
export function releaseGateContext(env = process.env) {
  demand(env.GITHUB_ACTIONS === 'true' && env.GITHUB_REPOSITORY === 'polaris-arch/Polaris'
    && /^[1-9][0-9]*$/.test(env.GITHUB_RUN_ID ?? '')
    && /^[1-9][0-9]*$/.test(env.GITHUB_RUN_ATTEMPT ?? ''), 'Current App Actions attempt required');
  return { repository: env.GITHUB_REPOSITORY, runId: env.GITHUB_RUN_ID, attempt: env.GITHUB_RUN_ATTEMPT };
}
function adapter(root, candidate) {
  demand(/^[a-f0-9]{40}$/.test(candidate ?? '')
    && execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim() === candidate
    && !execFileSync('git', ['status', '--porcelain', '--untracked-files=no'], { cwd: root, encoding: 'utf8' }).trim(),
  'Native release gate candidate differs or is dirty');
  const facts = { candidate, fileSha256: Object.fromEntries(ADAPTER_FILES.map((name) => {
    const path = join(root, name); regularPath(path); return [name, digest(readFileSync(path))];
  })) };
  return { schema: 'polaris-app-release-adapter-v1', facts, fingerprint: digest(portableCanonical(facts)) };
}
export function verifyNativeReleaseGate(gate, contract, selection, expectedAdapter, context, key) {
  const { fingerprint, ...facts } = gate;
  demand(gate.schema === 'polaris-app-native-release-gate-v1'
    && fingerprint === digest(portableCanonical(facts)) && gate.platform === key
    && gate.candidate === expectedAdapter.facts.candidate
    && gate.contractSha256 === selection.contractSha256, 'Native release gate identity differs');
  same(gate.context, context, 'Native release gate attempt differs');
  same([gate.host, gate.arch], HOSTS[key], 'Native release inspection host differs');
  const receipt = gate.receipt, { schema, fingerprint: receiptHash, ...receiptFacts } = receipt;
  demand(schema === 'polaris-app-release-consumption-v1'
    && receiptHash === digest(portableCanonical(receiptFacts)) && receipt.platform === key
    && receipt.contractSha256 === selection.contractSha256
    && receipt.assetIndexSha256 === contract.assetIndex.sha256, 'Native acquisition receipt differs');
  same(receipt.adapter, expectedAdapter, 'Native App adapter differs');
  same(receipt.producer, contract.platforms[key], 'Original release producer differs');
  same(receipt.releaseAsset, contract.platforms[key].releaseAsset, 'Original public asset differs');
  same(gate.readback.before, gate.readback.after, 'Native public identity changed');
  demand(gate.readback.schema === 'polaris-app-release-readback-v1'
    && gate.readback.contractSha256 === selection.contractSha256, 'Native readback contract differs');
  same(receipt.publicReadback, gate.readback.before, 'Native readback snapshot differs');
  const snapshot = receipt.publicReadback;
  validateUpstreamReadback(snapshot, contract);
  demand(snapshot.repositoryId === releaseRepositoryId(contract) && snapshot.releaseId === contract.releaseId
    && snapshot.tag === contract.tagObject && snapshot.sourceCommit === contract.sourceCommit
    && snapshot.sourceTree === contract.sourceTree, 'Native public source identity differs');
  same(snapshot.assets.map(({ name, id, bytes, url }) => ({ name, id, bytes, url })),
    contractPublicAssets(contract)
      .map((pin) => ({ name: pin.name, id: pin.assetId, bytes: pin.bytes, url: pin.url })), 'Native public asset inventory differs');
  demand(gate.indexSha256 === contract.assetIndex.sha256
    && gate.inspection === 'download-extract-buildinfo-buildid-cronet-native-signature'
    && gate.actualAppAcceptance === 'pending load, gvisor/mixed, feature and final carrier', 'Native inspection scope differs');
  return gate;
}
export function verifyNativeReleaseBundle(root, manifest, bundle, candidate, env = process.env) {
  regularPath(bundle, true);
  const contract = loadReleaseContract(root, manifest), identity = adapter(root, candidate), context = releaseGateContext(env);
  const inventory = parse(join(bundle, 'bundle.json'));
  demand(inventory.schema === 'polaris-app-native-release-bundle-v1' && inventory.candidate === candidate
    && inventory.contractSha256 === manifest.desktopRelease.contractSha256, 'Native release bundle differs');
  same(inventory.context, context, 'Native release bundle attempt differs');
  same(Object.keys(inventory.platforms).sort(), Object.keys(RELEASE_TARGETS).sort(), 'All four native release gates required');
  same(readdirSync(bundle).sort(), ['bundle.json', ...Object.keys(RELEASE_TARGETS)].sort(), 'Unexpected native release bundle entry');
  for (const key of Object.keys(RELEASE_TARGETS)) {
    const directory = join(bundle, key); regularPath(directory, true);
    same(readdirSync(directory), ['native-gate.json'], 'Unexpected native gate payload');
    const path = join(directory, 'native-gate.json'); regularPath(path);
    demand(digest(readFileSync(path)) === inventory.platforms[key], 'Native gate file SHA differs');
    verifyNativeReleaseGate(parse(path), contract, manifest.desktopRelease, identity, context, key);
  }
  return inventory;
}

async function main() {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
  const [command, ...args] = process.argv.slice(2), values = Object.fromEntries(args.map((arg) => {
    const match = /^--(platform|candidate|bundle-dir)=(.+)$/.exec(arg); demand(match, 'Unexpected native gate argument'); return match.slice(1);
  }));
  demand(Object.keys(values).length === args.length && values.candidate && values['bundle-dir'], 'Native gate candidate/bundle required');
  const manifest = parse(join(root, 'src-tauri/core-manifest.json')), contract = loadReleaseContract(root, manifest);
  const identity = adapter(root, values.candidate), context = releaseGateContext(), bundle = resolve(values['bundle-dir']);
  if (command === 'acquire') {
    const key = values.platform; demand(Object.hasOwn(HOSTS, key), 'Explicit native release platform required');
    same([process.platform, process.arch], HOSTS[key], 'Actual release acquisition host differs');
    const receipts = await consumeRelease(root, manifest, [key]);
    verifyPackagedRelease(root, manifest, key, join(root, 'resources', key, RELEASE_TARGETS[key].core));
    const facts = { schema: 'polaris-app-native-release-gate-v1', candidate: values.candidate, context,
      contractSha256: manifest.desktopRelease.contractSha256, platform: key, host: process.platform, arch: process.arch,
      receipt: receipts[key], readback: parse(join(root, 'resources/.release-receipts/public-readback.json')),
      indexSha256: digest(readFileSync(join(root, 'resources/.release-receipts', contract.assetIndex.name))),
      inspection: 'download-extract-buildinfo-buildid-cronet-native-signature',
      actualAppAcceptance: 'pending load, gvisor/mixed, feature and final carrier' };
    const gate = { ...facts, fingerprint: digest(portableCanonical(facts)) };
    verifyNativeReleaseGate(gate, contract, manifest.desktopRelease, identity, context, key);
    const dest = join(bundle, key); mkdirSync(dest, { recursive: true }); regularPath(dest, true);
    writeFileSync(join(dest, 'native-gate.json'), JSON.stringify(gate, null, 2) + '\n', { flag: 'wx' });
  } else if (command === 'assemble') {
    demand(!values.platform, 'Assembly requires all native platforms');
    const platforms = Object.fromEntries(Object.keys(RELEASE_TARGETS).map((key) => {
      const path = join(bundle, key, 'native-gate.json');
      verifyNativeReleaseGate(parse(path), contract, manifest.desktopRelease, identity, context, key);
      return [key, digest(readFileSync(path))];
    }));
    writeFileSync(join(bundle, 'bundle.json'), JSON.stringify({ schema: 'polaris-app-native-release-bundle-v1',
      candidate: values.candidate, context, contractSha256: manifest.desktopRelease.contractSha256, platforms }, null, 2) + '\n', { flag: 'wx' });
    verifyNativeReleaseBundle(root, manifest, bundle, values.candidate);
  } else throw Error('Unknown native release gate command');
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
