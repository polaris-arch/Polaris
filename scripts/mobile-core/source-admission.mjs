// Formal mobile declaration only. Git, source bytes and tool execution are
// independently observed by the producer.
import { readFileSync } from 'node:fs';
import { posix, resolve, win32 } from 'node:path';
import { pathToFileURL } from 'node:url';

const versionPattern = new RegExp(JSON.parse(readFileSync(new URL('../lib/polaris-version.json', import.meta.url))).pattern);
const goVersion = '1.27.4';
const vendorDirectories = {
  'github.com/sagernet/sing-tun': 'sing-tun',
  'github.com/sagernet/tailscale': 'tailscale',
  'github.com/metacubex/utls': 'metacubex-utls',
  'github.com/sagernet/cronet-go': 'cronet-go',
  'github.com/sagernet/fswatch': 'fswatch',
  'github.com/sagernet/sing': 'sing',
  'github.com/sagernet/sing-quic': 'sing-quic',
  'github.com/sagernet/wireguard-go': 'wireguard-go',
  'github.com/database64128/tfo-go/v2': 'tfo-go',
  'github.com/sagernet/sing-vmess': 'sing-vmess',
  'github.com/libp2p/go-nat': 'go-nat',
  'github.com/google/nftables': 'google-nftables',
};

function demand(condition, reason) {
  if (!condition) throw new Error(reason);
}
const object = (value) => value != null && typeof value === 'object' && !Array.isArray(value);
const closed = (value, keys) => object(value) && Object.keys(value).length === keys.length &&
  keys.every((key) => Object.hasOwn(value, key));
const exactList = (value, expected) => Array.isArray(value) && value.length === expected.length &&
  expected.every((entry, index) => value[index] === entry);
const hex = (value, width) => typeof value === 'string' && value.length === width && /^[0-9a-f]+$/.test(value);

function validateSourcePolicy(policy, source) {
  const review = (condition, reason) => demand(condition, 'reviewed source policy: ' + reason);
  review(closed(policy, ['schema', 'sourceCommit', 'sourceTree', 'goDirective', 'goVersion', 'toolchain',
    'goModSha256', 'goSumSha256', 'metacubex', 'vendors']) &&
    policy.schema === 'polaris-reviewed-kernel-source-v4', 'complete explicit declaration required');
  for (const key of ['sourceCommit', 'sourceTree']) {
    review(hex(policy[key], 40) && policy[key] === source[key], key + ' differs');
  }
  review(policy.goDirective === '1.27.0' && policy.goVersion === goVersion &&
    policy.toolchain === 'go' + goVersion, 'exact Go directive/toolchain required');
  for (const key of ['goModSha256', 'goSumSha256']) {
    review(hex(policy[key], 64) && policy[key] === source[key], key + ' differs');
  }

  const { metacubex: meta, vendors } = policy;
  const vendorFields = ['version', 'replacementPath', 'replacementTree', 'replacementFileCount'];
  review(closed(meta, [...vendorFields, 'module', 'bindingPath', 'bindingSha256', 'patchPath', 'patchSha256',
    'upstreamCommit', 'upstreamH1', 'upstreamZipSha256']) &&
    meta.module === 'github.com/metacubex/utls' && meta.version === 'v1.8.8' &&
    meta.replacementPath === './third_party/metacubex-utls' && meta.replacementFileCount === 355 &&
    meta.bindingPath === 'release/dependency-sources/metacubex-utls-replacement.json' &&
    meta.patchPath === 'release/dependency-sources/metacubex-utls-reality-alpn.patch',
  'exact Meta source declaration required');
  review(hex(meta.replacementTree, 40) && hex(meta.upstreamCommit, 40) &&
    ['bindingSha256', 'patchSha256', 'upstreamZipSha256'].every((key) => hex(meta[key], 64)) &&
    typeof meta.upstreamH1 === 'string' && /^h1:[A-Za-z0-9+/]{43}=$/.test(meta.upstreamH1),
  'Meta source hashes/baseline sum differ');
  review(closed(vendors, Object.keys(vendorDirectories)), 'exact reviewed vendor names required');
  for (const [name, pin] of Object.entries(vendors)) {
    review(closed(pin, vendorFields), 'complete vendor declaration required: ' + name);
    review(pin.replacementPath === './third_party/' + vendorDirectories[name] && hex(pin.replacementTree, 40) &&
      Number.isInteger(pin.replacementFileCount) && pin.replacementFileCount > 0 && pin.replacementFileCount <= 65536,
    'invalid vendor pin: ' + name);
    review(typeof pin.version === 'string' && /^v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(pin.version),
      'explicit vendor module version required: ' + name);
  }
  review(vendorFields.every((key) => vendors[meta.module][key] === meta[key]),
    'Meta and reviewed-vendor declarations disagree');
}

export function validateMobileSource(value) {
  demand(object(value) && value.schema === 'polaris-mobile-fork-input-v1' && value.role === 'mobile',
    'formal mobile source role is required');
  demand(value.repository === 'polaris-arch/sing-box-polaris' && value.repositoryId === 1421657509 &&
    value.sourceURL === 'https://github.com/polaris-arch/sing-box-polaris', 'mobile source repository differs');
  demand(value.goModule === 'github.com/sagernet/sing-box', 'file branding must not rename the Go module');
  for (const [key, width] of [['sourceCommit', 40], ['sourceTree', 40], ['sourceTagObject', 40],
    ['goModSha256', 64], ['goSumSha256', 64]]) {
    demand(hex(value[key], width), 'actual immutable source field required: ' + key);
  }
  const match = typeof value.version === 'string' && versionPattern.exec(value.version);
  demand(match && match[0] === value.version && match[3] === 'alpha' && value.sourceTag === 'v' + value.version,
    'mobile source tag/version differs');
  validateSourcePolicy(value.sourcePolicy, value);
  demand(value.sourceOverlay === 0 && exactList(value.patches, []) && exactList(value.dependencyPatches, []),
    'formal mobile source must be zero-overlay');
  demand(value.goVersion === value.sourcePolicy.goVersion && value.gomobileVersion === 'v0.1.13' &&
    value.gobindVersion === value.gomobileVersion, 'formal generator must match the actual source module and API');
  demand(value.artifactPins == null && value.producerReceipt == null && value.AppAccepted === false &&
    value.devicePassed === false && value.publicationEligible === false, 'input declarations cannot issue artifact acceptance');

  const { android, apple } = value;
  demand(object(android) && exactList(android.targets, ['android/arm64', 'android/arm', 'android/386', 'android/amd64']) &&
    exactList(android.abis, ['arm64-v8a', 'armeabi-v7a', 'x86', 'x86_64']) && android.minimumAPI === 24 &&
    android.minimumLoadAlignment === 16384 && android.javaMajor === 17 && android.legacyAPI21Enabled === false,
  'formal Android requires one API24/four-ABI AAR and no legacy variant');
  demand(android.ndkRevision === '30.0.16248370' && android.ndkSourceRevision === android.ndkRevision &&
    android.ndkPackage === 'android-ndk-r30' && android.ndkArchivePath === 'scripts/mobile-core/ndk-r30-linux.json',
  'formal Android NDK differs');
  demand(object(apple) && exactList(apple.targets, ['ios/arm64', 'iossimulator/arm64']) &&
    exactList(apple.slices, ['ios-arm64', 'ios-arm64-simulator']) && apple.minimumOS === '15.0' &&
    apple.appMinimumOS === '17.0' && apple.macOSTvOSIncluded === false,
  'formal Apple requires exactly two arm64 iOS platform slices');
  demand(apple.frameworkName === 'Libbox.xcframework' && android.libname === 'box' && apple.libname === 'box' &&
    android.javaPackage === 'io.nekohasekai', 'existing SDK bindings must be retained');
  for (const { tags, outputName } of [android, apple]) {
    demand(Array.isArray(tags) && tags.length > 0 && new Set(tags).size === tags.length &&
      tags.every((tag) => typeof tag === 'string' && /^[a-z0-9_]+$/.test(tag)) &&
      ['with_gvisor', 'with_naive_outbound', 'with_tailscale', 'with_clash_api'].every((tag) => tags.includes(tag)),
    'formal mobile feature tags differ');
    demand(typeof outputName === 'string' && outputName.startsWith('polaris-box-' + value.version + '-'),
      'mobile asset filename must use the polaris-box prefix');
    demand(posix.basename(outputName) === outputName && win32.basename(outputName) === outputName,
      'asset basename cannot escape output slot');
  }
  const appleTags = new Set([...android.tags, 'with_dhcp', 'grpcnotrace', 'with_low_memory']);
  demand(appleTags.size === apple.tags.length && apple.tags.every((tag) => appleTags.has(tag)), 'Apple target-only tags differ');
  return value;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    // Python already rejected duplicate keys and nonfinite input. Distinguish
    // JSON literals for the fields whose contract specifically requires int.
    const nonInteger = Symbol('non-integer literal');
    const source = JSON.parse(readFileSync(0, 'utf8'), (key, value, context) =>
      (key === 'sourceOverlay' || key === 'replacementFileCount') && typeof value === 'number' &&
      /[.eE]/.test(context.source) ? nonInteger : value);
    validateMobileSource(source);
  } catch (error) {
    console.error(error instanceof SyntaxError ? 'invalid mobile source declaration JSON' : error.message);
    process.exitCode = 1;
  }
}
