// Mobile declaration and receipt decoding. The producer independently observes
// Git, source bytes and tool execution.
import { createHash } from 'node:crypto';
import { closeSync, constants, fstatSync, lstatSync, openSync, readFileSync, readSync } from 'node:fs';
import { posix, resolve, win32 } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const versionPattern = new RegExp(JSON.parse(readFileSync(new URL('../lib/polaris-version.json', import.meta.url))).pattern);
const goVersion = '1.27.4';
const sourceContractBytes = 65536;
const utf8 = new TextDecoder('utf-8', { fatal: true });
const statOptions = { bigint: true };
export const receiptPolicy = JSON.parse(readFileSync(new URL('receipt-policy.json', import.meta.url)));
const limits = receiptPolicy.receipt;
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

export class ReceiptFloat {
  constructor(value) {
    demand(Number.isFinite(value), 'nonfinite receipt JSON');
    this.value = value;
  }
}

function lexicalBudget(raw) {
  let quoted = false, escaped = false, depth = 0, tokens = 0, stringBytes = 0;
  for (const byte of raw) {
    if (quoted) {
      demand(++stringBytes <= 6 * limits.stringChars + 2, 'receipt JSON encoded string budget');
      if (escaped) escaped = false;
      else if (byte === 92) escaped = true;
      else if (byte === 34) quoted = false;
    } else if (byte === 34) {
      quoted = true;
      stringBytes = 0;
      tokens++;
    } else if (byte === 123 || byte === 91) {
      demand(++depth <= limits.depth, 'receipt JSON depth budget');
      tokens++;
    } else if (byte === 125 || byte === 93) depth--;
    else if (byte === 44 || byte === 58) tokens++;
    demand(tokens <= 3 * limits.nodes, 'receipt JSON lexical node budget');
  }
}

function text(raw) {
  // Python's byte decoder also admits UTF-16/32 receipts, with or without BOM.
  const utf32be = raw[0] === 0 && raw[1] === 0 && (raw[2] === 0 || (raw[2] === 0xfe && raw[3] === 0xff));
  const utf32le = raw[2] === 0 && raw[3] === 0 && (raw[1] === 0 || (raw[0] === 0xff && raw[1] === 0xfe));
  if (utf32be || utf32le) {
    demand(raw.length % 4 === 0, 'invalid UTF-32 receipt JSON');
    const decode = utf32be ? raw.readUInt32BE.bind(raw) : raw.readUInt32LE.bind(raw);
    const chunks = [];
    for (let at = decode(0) === 0xfeff ? 4 : 0; at < raw.length;) {
      const end = Math.min(at + 4096 * 4, raw.length), points = [];
      for (; at < end; at += 4) points.push(decode(at));
      chunks.push(String.fromCodePoint(...points));
    }
    return chunks.join('');
  }
  if ((raw[0] === 0xff && raw[1] === 0xfe) || (raw.length >= 2 && raw[0] !== 0 && raw[1] === 0)) {
    return new TextDecoder('utf-16le', { fatal: true }).decode(raw);
  }
  if ((raw[0] === 0xfe && raw[1] === 0xff) || (raw.length >= 2 && raw[0] === 0 && raw[1] !== 0)) {
    return new TextDecoder('utf-16be', { fatal: true }).decode(raw);
  }
  return utf8.decode(raw);
}

export const characters = value => value.length === 0 ? 0 : value.length - (value.match(/[\ud800-\udbff][\udc00-\udfff]/g)?.length ?? 0);

// State is local to the original receipt. Object keys are compared after
// unescaping, and numeric types survive parsing for CPython fingerprint parity.
export function decode(raw, maximum, receiptLimit) {
  demand(raw.length <= maximum, receiptLimit ? 'original receipt exceeds its byte budget' : 'mobile input contract is too large');
  if (receiptLimit) lexicalBudget(raw);
  const source = text(raw), whitespace = /[ \t\r\n]*/y;
  const scalar = /(?:true|false|null|-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)/y;
  let at = 0, nodes = 0, stringChars = 0;
  const skip = () => { whitespace.lastIndex = at; whitespace.exec(source); at = whitespace.lastIndex; };
  const count = depth => demand(++nodes <= limits.nodes && depth <= limits.depth, 'receipt JSON node/depth budget');
  const fails = () => { throw new Error('invalid receipt JSON'); };
  function string(isKey = false) {
    const start = at++;
    for (; at < source.length; at++) {
      if (source[at] === '\\') at++;
      else if (source[at] === '"') {
        const value = JSON.parse(source.slice(start, ++at));
        // Source declarations also passed through Python's canonical UTF-8 encoder.
        if (!receiptLimit) demand(value.isWellFormed(), 'invalid Unicode in mobile source declaration');
        const length = characters(value);
        stringChars += length;
        demand(length <= (isKey ? limits.keyChars : limits.stringChars) && stringChars <= maximum, 'receipt JSON string budget');
        return value;
      }
    }
    return fails();
  }
  function value(depth) {
    skip();
    count(depth);
    if (source[at] === '"') return string();
    if (source[at] === '{' || source[at] === '[') {
      const isObject = source[at++] === '{', end = isObject ? '}' : ']';
      const properties = isObject ? Object.create(null) : [];
      skip();
      if (source[at] === end) { at++; return properties; }
      while (at < source.length) {
        if (isObject) {
          if (source[at] !== '"') return fails();
          count(depth + 1);
          const key = string(true);
          demand(!Object.hasOwn(properties, key), 'duplicate mobile contract key');
          skip();
          if (source[at++] !== ':') return fails();
          properties[key] = value(depth + 1);
        } else properties.push(value(depth + 1));
        skip();
        if (source[at] === end) { at++; return properties; }
        if (source[at++] !== ',') return fails();
        skip();
      }
      return fails();
    }
    scalar.lastIndex = at;
    const token = scalar.exec(source)?.[0];
    if (!token) return fails();
    at = scalar.lastIndex;
    if (token === 'null' || token === 'true' || token === 'false') return JSON.parse(token);
    if (/[.eE]/.test(token)) return new ReceiptFloat(Number(token));
    demand(token.length <= Math.ceil(limits.integerBits * Math.log10(2)) + 1, 'receipt JSON integer budget');
    const integer = BigInt(token);
    demand((integer < 0 ? -integer : integer).toString(2).length <= limits.integerBits, 'receipt JSON integer budget');
    const small = Number(integer);
    return Number.isSafeInteger(small) ? small : integer;
  }
  const result = value(0);
  skip();
  if (at !== source.length) return fails();
  if (receiptLimit) demand(raw.length <= receiptLimit(result) && stringChars <= receiptLimit(result), 'original receipt exceeds its schema budget');
  return result;
}

export function pinned(path, maximum, observation = false, { allowEmpty = false } = {}) {
  const before = lstatSync(path, { ...statOptions, throwIfNoEntry: false });
  if (observation) console.error(JSON.stringify({
    schema: 'polaris-receipt-size-observation-v1', phase: 'read', path,
    bytes: Number(before?.size ?? 0n), symlink: before?.isSymbolicLink() ?? false, maximumReadBytes: maximum,
    scope: 'actual lstat size; not receipt or carrier acceptance',
  }));
  demand(before?.isFile() && (allowEmpty || before.size > 0n) && before.size <= BigInt(maximum), 'bounded regular original receipt or artifact required');
  const fd = openSync(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0));
  try {
    const identity = info => info.isFile() && ['dev', 'ino', 'size', 'mtimeNs', 'ctimeNs'].every(key => info[key] === before[key]);
    demand(identity(fstatSync(fd, statOptions)), 'original file changed before bounded read');
    const bytes = Buffer.allocUnsafe(Number(before.size));
    let at = 0;
    while (at < bytes.length) {
      const length = readSync(fd, bytes, at, bytes.length - at, at);
      demand(length > 0, 'original file changed during bounded read');
      at += length;
    }
    demand(identity(fstatSync(fd, statOptions)) && identity(lstatSync(path, statOptions)), 'original file changed during bounded read');
    return { bytes, pin: { bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') } };
  } finally { closeSync(fd); }
}

function sourceDeclaration(value, key) {
  if (value instanceof ReceiptFloat) return ['sourceOverlay', 'replacementFileCount'].includes(key) ? undefined : value.value;
  if (Array.isArray(value)) return value.map(item => sourceDeclaration(item));
  if (object(value)) return Object.fromEntries(Object.entries(value).map(([name, item]) => [name, sourceDeclaration(item, name)]));
  return value;
}

function validateSourceBytes(raw) {
  const source = decode(raw, sourceContractBytes);
  validateMobileSource(sourceDeclaration(source));
  return source;
}

export function readMobileSource() {
  const path = fileURLToPath(new URL('source-contract.json', import.meta.url));
  return validateSourceBytes(pinned(path, sourceContractBytes).bytes);
}

function bindingPlans(source) {
  const linkerFlagsPrefix = '-X github.com/sagernet/sing-box/constant.Version=' + source.version +
    ' -X github.com/sagernet/sing-box/constant.DirectoryRetirementSourceIdentity=' + source.sourceCommit +
    ' -X runtime.godebugDefault=multipathtcp=0,tlssha1=1 -checklinkname=0 -s -w -buildid=polaris-mobile-';
  const argument = input => ({ input });
  const platforms = Object.fromEntries(['android', 'apple'].map(platform => {
    const profile = source[platform];
    const android = platform === 'android';
    const argv = [argument('gomobile'), 'bind', '-o', argument('output'), '-target', profile.targets.join(','),
      '-libname=' + profile.libname, '-trimpath', '-buildvcs=false', '-work', '-x',
      '-ldflags', argument('ldflags'), '-tags', profile.tags.join(','),
      ...(android ? ['-androidapi', argument('minimumAPI'), '-bootclasspath', argument('bootclasspath'),
        '-javapkg=' + profile.javaPackage] : ['-iosversion', profile.minimumOS]),
      './experimental/libbox'];
    const facts = { schema: 'polaris-mobile-bind-input-v1', platform, targets: profile.targets, tags: profile.tags,
      gomobileVersion: source.gomobileVersion, gobindVersion: source.gobindVersion, artifactAcceptance: false };
    const summary = { sourceOverlay: 0, actualToolIdentityRequiredBeforeExecution: true,
      requiredOutputChecks: android ? ['four exact ABI native ELF', 'API24 Android ident', '16KiB PT_LOAD',
        'complete JNI/Java API', 'actual Go BuildInfo/module/replacement', 'actual BuildID and source/tool input binding',
        'final APK/R8/alignment'] : ['two exact arm64 iOS platform slices', 'static archives and generated Objective-C/Swift API',
        'source/module/runtime inputs', 'actual SDK/compiler identities and BuildID', 'final App/PacketTunnel static-link provenance'],
      producerReceipt: null, AppAccepted: false, devicePassed: false, publicationEligible: false };
    return [platform, { schema: 'polaris-mobile-bind-plan-v1', argv, facts, summary }];
  }));
  return { linkerFlagsPrefix, platforms };
}

export function readBoundedInput(maximum) {
  const bytes = Buffer.allocUnsafe(maximum + 1);
  let received = 0;
  while (received < bytes.length) {
    const count = readSync(0, bytes, received, bytes.length - received, null);
    if (count === 0) break;
    received += count;
  }
  demand(received <= maximum, 'mobile input contract is too large');
  return bytes.subarray(0, received);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const args = process.argv.slice(2);
    if (args.length === 1 && args[0] === '--versions') {
      const source = readMobileSource();
      console.log(`${source.goVersion} ${source.version}`);
    } else {
      // The producer sends its full declaration once and reuses these plans.
      demand(args.length === 0, 'usage: source-admission.mjs [--versions]');
      console.log(JSON.stringify(bindingPlans(validateSourceBytes(readBoundedInput(sourceContractBytes)))));
    }
  } catch (error) {
    console.error(error instanceof SyntaxError ? 'invalid mobile source declaration JSON' : error.message);
    process.exitCode = 1;
  }
}
