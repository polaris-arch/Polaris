// Mobile declaration, exact source checkout, reviewed source observation, and receipt decoding.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, constants, fstatSync, lstatSync, mkdirSync, openSync, readFileSync, readlinkSync, readSync, realpathSync, statSync } from 'node:fs';
import { devNull } from 'node:os';
import { dirname, join, posix, resolve, win32 } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const versionPattern = new RegExp(JSON.parse(readFileSync(new URL('../lib/polaris-version.json', import.meta.url))).pattern);
const goVersion = '1.27.4';
const sourceContractBytes = 65536;
const sourceControlBytes = 2 * 1024 * 1024;
const sourceTagsPath = 'cmd/internal/build_libbox/main.go';
const vendorFileBytes = 32 * 1024 * 1024;
const vendorTotalBytes = 512 * 1024 * 1024;
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
const review = (condition, reason) => demand(condition, 'reviewed source policy: ' + reason);
const object = (value) => value != null && typeof value === 'object' && !Array.isArray(value);
const closed = (value, keys) => object(value) && Object.keys(value).length === keys.length &&
  keys.every((key) => Object.hasOwn(value, key));
const exactList = (value, expected) => Array.isArray(value) && value.length === expected.length &&
  expected.every((entry, index) => value[index] === entry);
const hex = (value, width) => typeof value === 'string' && value.length === width && /^[0-9a-f]+$/.test(value);

function validateSourcePolicy(policy, source) {
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

export function receiptLimit(value) {
  demand(object(value) && !(value instanceof ReceiptFloat), 'receipt must be an object');
  if (value.schema === 'polaris-mobile-component-v1' && value.platform === 'apple') return limits.appleComponentBytes;
  if (value.schema === 'polaris-mobile-apple-link-input-v1') return limits.appleLinkBytes;
  return limits.controlBytes;
}

function floatText(n) {
  if (Object.is(n, -0)) return '-0.0';
  if (n === 0) return '0.0';
  const [coefficient, power = '0'] = Math.abs(n).toString().split('e');
  const decimal = coefficient.indexOf('.');
  const digits = coefficient.replace('.', '');
  const leading = digits.search(/[1-9]/);
  const exponent = Number(power) + (decimal < 0 ? coefficient.length : decimal) - leading - 1;
  const significant = digits.slice(leading).replace(/0+$/, '');
  const sign = n < 0 ? '-' : '';
  if (exponent < -4 || exponent >= 16) {
    return sign + significant[0] + (significant.length > 1 ? '.' + significant.slice(1) : '') +
      'e' + (exponent < 0 ? '-' : '+') + String(Math.abs(exponent)).padStart(2, '0');
  }
  const point = exponent + 1;
  if (point <= 0) return sign + '0.' + '0'.repeat(-point) + significant;
  if (point >= significant.length) return sign + significant.padEnd(point, '0') + '.0';
  return sign + significant.slice(0, point) + '.' + significant.slice(point);
}

export function codepointOrder(left, right) {
  for (let at = 0; at < Math.min(left.length, right.length);) {
    const a = left.codePointAt(at), b = right.codePointAt(at);
    if (a !== b) return a - b;
    at += a > 0xffff ? 2 : 1;
  }
  return left.length - right.length;
}

export function* canonicalChunks(value, { maximum, pretty = false, ascii = pretty } = {}) {
  let nodes = 0, stringChars = 0, bytes = 0, buffer = '';
  const active = new Set();
  function* encode(entry, depth, isKey = false) {
    if (maximum !== undefined) {
      demand(++nodes <= limits.nodes && depth <= limits.depth, 'receipt JSON node/depth budget');
    }
    if (entry instanceof ReceiptFloat) {
      yield floatText(entry.value);
    } else if (typeof entry === 'string') {
      if (maximum !== undefined) {
        const length = characters(entry);
        stringChars += length;
        demand(length <= (isKey ? limits.keyChars : limits.stringChars) && stringChars <= maximum,
          'receipt JSON string budget');
      }
      demand(ascii || entry.isWellFormed(), 'invalid Unicode in receipt JSON');
      const quoted = JSON.stringify(entry);
      yield ascii ? quoted.replace(/[\u007f-\uffff]/g,
        char => '\\u' + char.charCodeAt(0).toString(16).padStart(4, '0')) : quoted;
    } else if (Array.isArray(entry) || object(entry)) {
      demand(!active.has(entry), 'cyclic receipt JSON');
      active.add(entry);
      const array = Array.isArray(entry);
      const keys = array ? entry.keys() : Object.keys(entry).sort(codepointOrder);
      yield array ? '[' : '{';
      let first = true;
      for (const key of keys) {
        if (!first) yield ',';
        if (pretty) yield '\n' + ' '.repeat(2 * (depth + 1));
        if (!array) {
          yield* encode(key, depth + 1, true);
          yield pretty ? ': ' : ':';
        }
        yield* encode(entry[key], depth + 1);
        first = false;
      }
      if (pretty && !first) yield '\n' + ' '.repeat(2 * depth);
      yield array ? ']' : '}';
      active.delete(entry);
    } else if (typeof entry === 'bigint') {
      const magnitude = entry < 0n ? -entry : entry;
      demand(maximum === undefined || magnitude < (1n << BigInt(limits.integerBits)),
        'receipt JSON integer budget');
      yield entry.toString();
    } else if (entry === null || typeof entry === 'boolean' || Number.isSafeInteger(entry)) {
      yield JSON.stringify(entry);
    } else {
      demand(typeof entry === 'number' && Number.isFinite(entry), 'unsupported receipt JSON value');
      yield floatText(entry);
    }
  }
  function* document() {
    yield* encode(value, 0);
    if (pretty) yield '\n';
  }
  for (const token of document()) {
    if (maximum !== undefined) {
      bytes += Buffer.byteLength(token);
      demand(bytes <= maximum, 'receipt JSON byte budget');
    }
    buffer += token;
    if (buffer.length >= 64 * 1024) {
      yield Buffer.from(buffer);
      buffer = '';
    }
  }
  if (buffer) yield Buffer.from(buffer);
}

export function canonical(value, options) {
  return Buffer.concat([...canonicalChunks(value, options)]);
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
function decodeJSON(raw, maximum, receiptLimit, acceptDocument, decodeText = text) {
  demand(raw.length <= maximum, receiptLimit ? 'original receipt exceeds its byte budget' : 'mobile input contract is too large');
  if (receiptLimit) lexicalBudget(raw);
  const source = decodeText(raw), whitespace = /[ \t\r\n]*/y;
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
  if (acceptDocument) {
    skip();
    while (at < source.length) {
      const start = at;
      const document = value(0);
      acceptDocument(document, source.slice(start, at));
      skip();
    }
    return;
  }
  const result = value(0);
  skip();
  if (at !== source.length) return fails();
  if (receiptLimit) demand(raw.length <= receiptLimit(result) && stringChars <= receiptLimit(result), 'original receipt exceeds its schema budget');
  return result;
}

export function decode(raw, maximum, receiptLimit) {
  return decodeJSON(raw, maximum, receiptLimit);
}

export function decodeDocuments(raw, maximum) {
  const rows = [];
  decodeJSON(raw, maximum, undefined, value => rows.push(value));
  return rows;
}

function moduleList(raw) {
  const paths = new Set(), rows = [];
  decodeJSON(raw, limits.controlBytes, undefined, (row, document) => {
    demand(object(row) && typeof row.Path === 'string', 'invalid actual module graph');
    demand(!paths.has(row.Path), 'empty/duplicate module graph');
    paths.add(row.Path);
    rows.push(document);
  }, bytes => new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes));
  demand(rows.length > 0, 'empty/duplicate module graph');
  // Preserve original nested values and numeric spellings in the producer's receipt.
  return '[' + rows.join(',') + ']';
}

function withPinnedDescriptor(path, maximum, observation, { allowEmpty = false, executable } = {}, inspect) {
  const before = lstatSync(path, { ...statOptions, throwIfNoEntry: false });
  if (observation) console.error(JSON.stringify({
    schema: 'polaris-receipt-size-observation-v1', phase: 'read', path,
    bytes: Number(before?.size ?? 0n), symlink: before?.isSymbolicLink() ?? false, maximumReadBytes: maximum,
    scope: 'actual lstat size; not receipt or carrier acceptance',
  }));
  demand(before?.isFile() && (allowEmpty || before.size > 0n) && before.size <= BigInt(maximum), 'bounded regular original receipt or artifact required');
  if (executable !== undefined) review(((before.mode & 0o111n) !== 0n) === executable, 'vendor mode differs');
  const fd = openSync(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0));
  try {
    const identity = info => info.isFile() && ['dev', 'ino', 'mode', 'size', 'mtimeNs', 'ctimeNs'].every(key => info[key] === before[key]);
    demand(identity(fstatSync(fd, statOptions)), 'original file changed before bounded read');
    const value = inspect(fd, Number(before.size));
    demand(identity(fstatSync(fd, statOptions)) && identity(lstatSync(path, statOptions)), 'original file changed during bounded read');
    return value;
  } finally { closeSync(fd); }
}

function readPinnedBytes(path, maximum, observation, options) {
  return withPinnedDescriptor(path, maximum, observation, options, (fd, size) => {
    const bytes = Buffer.allocUnsafe(size);
    for (let at = 0; at < bytes.length;) {
      const length = readSync(fd, bytes, at, bytes.length - at, at);
      demand(length > 0, 'original file changed during bounded read');
      at += length;
    }
    return bytes;
  });
}

export function pinnedDigest(path, maximum, options) {
  return withPinnedDescriptor(path, maximum, false, options, (fd, size) => {
    const hash = createHash('sha256'), block = Buffer.allocUnsafe(Math.min(size, 1024 ** 2));
    for (let at = 0; at < size;) {
      const length = readSync(fd, block, 0, Math.min(size - at, block.length), at);
      demand(length > 0, 'original file changed during bounded read');
      hash.update(block.subarray(0, length));
      at += length;
    }
    return { bytes: size, sha256: hash.digest('hex') };
  });
}

export function pinned(path, maximum, observation = false, options) {
  const bytes = readPinnedBytes(path, maximum, observation, options);
  return { bytes, pin: { bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') } };
}

function sourceDeclaration(value, key) {
  if (value instanceof ReceiptFloat) return ['sourceOverlay', 'replacementFileCount'].includes(key) ? undefined : value.value;
  if (Array.isArray(value)) return value.map(item => sourceDeclaration(item));
  if (object(value)) return Object.fromEntries(Object.entries(value).map(([name, item]) => [name, sourceDeclaration(item, name)]));
  return value;
}

export function validateSourceBytes(raw) {
  const source = decode(raw, sourceContractBytes);
  validateMobileSource(sourceDeclaration(source));
  return source;
}

export function readMobileSource() {
  const path = fileURLToPath(new URL('source-contract.json', import.meta.url));
  return validateSourceBytes(pinned(path, sourceContractBytes).bytes);
}

function verifySourceModule(raw, source) {
  const { goDirective, toolchain, vendors } = source.sourcePolicy;
  const text = utf8.decode(raw);
  review(!text.includes('/*') && !text.includes('\0'), 'unsupported source module syntax');
  const directives = new Map(), dependencies = new Map(), replacements = new Map();
  let block, generatorDeclared = false;
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.split('//', 1)[0].trim();
    if (!line) continue;
    if (line === ')') {
      review(block !== undefined, 'unmatched source module block');
      block = undefined;
      continue;
    }
    const tokens = line.split(/\s+/);
    const directive = block ?? tokens.shift();
    if (exactList(tokens, ['('])) {
      review(block === undefined && ['require', 'replace', 'exclude', 'retract'].includes(directive),
        'unsupported source module block');
      block = directive;
      continue;
    }
    review(!/["`]/.test(line), 'quoted source module declaration refused');
    const [name, version] = tokens;
    if (['module', 'go', 'toolchain'].includes(directive)) {
      review(block === undefined && !directives.has(directive) && tokens.length === 1,
        'duplicate or malformed source module directive');
      if (directive !== 'toolchain') review(line === rawLine.trim(), 'actual Go/module directives differ');
      directives.set(directive, name);
    } else if (directive === 'require') {
      review(tokens.length === 2 && !dependencies.has(name), 'duplicate or malformed source dependency');
      dependencies.set(name, version);
      if (name === 'github.com/sagernet/gomobile') generatorDeclared = block === 'require' && line === rawLine.trim();
    } else if (directive === 'replace') {
      const marker = tokens.indexOf('=>'), right = tokens.length - marker - 1;
      review((marker === 1 || marker === 2) && (right === 1 || right === 2) &&
        marker === tokens.lastIndexOf('=>') && !replacements.has(name),
      'duplicate or malformed source replacement');
      review(right === 2 || marker === 1, 'version-scoped local replacement refused');
      replacements.set(name, right === 1 ? tokens[marker + 1] : null);
    }
  }
  review(block === undefined, 'unterminated source module block');
  review(directives.get('module') === source.goModule && directives.get('go') === goDirective &&
    directives.get('toolchain') === toolchain, 'actual Go/module directives differ');
  review([...replacements.values()].filter(path => path !== null).length === Object.keys(vendors).length,
    'exact reviewed local replacements required');
  for (const [name, pin] of Object.entries(vendors)) {
    review(replacements.get(name) === pin.replacementPath, 'exact reviewed local replacements required');
    review(dependencies.get(name) === pin.version, 'actual vendor dependency version differs: ' + name);
  }
  review(generatorDeclared && dependencies.get('github.com/sagernet/gomobile') === source.gomobileVersion,
    'actual Meta/generator dependency version differs');
}

function gitObject(kind, raw) {
  return createHash('sha1').update(kind + ' ' + raw.length + '\0').update(raw).digest();
}

function vendorTree(entries) {
  const rows = [...entries].map(([name, value]) => {
    const directory = value instanceof Map;
    const mode = directory ? '40000' : value.mode;
    const digest = directory ? vendorTree(value) : value.digest;
    return {
      sort: Buffer.from(name + (directory ? '/' : '')),
      row: Buffer.concat([Buffer.from(mode + ' ' + name + '\0'), digest]),
    };
  });
  rows.sort((left, right) => Buffer.compare(left.sort, right.sort));
  return gitObject('tree', Buffer.concat(rows.map(({ row }) => row)));
}

export function gitEnvironment(environment = process.env) {
  const env = Object.fromEntries(Object.entries(environment).filter(([name]) => !name.startsWith('GIT_')));
  return Object.assign(env, { GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: devNull, GIT_NO_REPLACE_OBJECTS: '1', GIT_TERMINAL_PROMPT: '0' });
}

function sourceGit(args, operation, timeout = 30_000) {
  try {
    // Preparation inherits the producer's owned process group. Git never owns
    // a detached group or reads from the declaration/parent's standard input.
    return execFileSync('git', ['--no-replace-objects', ...args], {
      env: gitEnvironment(), timeout, maxBuffer: limits.controlBytes, stdio: ['ignore', 'pipe', 'pipe'],
    });
  } catch {
    throw new Error('mobile source Git ' + operation + ' failed');
  }
}

function verifyCheckout(root, source, raw, generatorOutputs) {
  root = realpathSync(root);
  const git = (...args) => sourceGit(['-C', root, ...args], 'verification');
  const text = (...args) => utf8.decode(git(...args)).trim();
  demand(realpathSync(text('rev-parse', '--show-toplevel')) === root, 'Git worktree differs from actual source root');
  demand(text('cat-file', '-t', source.sourceTagObject) === 'tag', 'annotated source tag required');
  demand(text('rev-parse', 'refs/tags/' + source.sourceTag + '^{tag}') === source.sourceTagObject &&
    text('rev-parse', source.sourceTagObject + '^{commit}') === source.sourceCommit &&
    text('rev-parse', source.sourceCommit + '^{tree}') === source.sourceTree, 'mobile source tag/object/commit/tree differs');
  demand(text('rev-parse', 'HEAD') === source.sourceCommit &&
    git('status', '--porcelain=v1', '--untracked-files=no', '-z').length === 0,
    'source checkout must be exact and clean before mobile bind');
  const declaration = join(root, sourceTagsPath);
  demand(realpathSync(dirname(declaration)) === dirname(declaration) &&
    pinned(declaration, sourceControlBytes).pin.sha256 === source.declaredTagsSource.sha256,
    'source-owned tags declaration differs');
  // Zero overlay includes ignored compiler/embed inputs. Only the producer's
  // verified generator output has the existing fixed build/ exception. Use
  // Git's quoted listing, including its local core.quotePath setting, because
  // historically only a rendered path starting with build/ was admitted.
  const inventoryText = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
  for (const args of [['--others', '--ignored', '--exclude-standard'], ['--others', '--exclude-standard']]) {
    const listing = inventoryText.decode(git('ls-files', ...args))
      .replace(/^[\p{White_Space}\x1c-\x1f]+|[\p{White_Space}\x1c-\x1f]+$/gu, '');
    const names = listing ? listing.split(/\r\n|[\r\n\v\f\x1c-\x1e\x85\u2028\u2029]/) : [];
    demand(names.every(name => generatorOutputs && name.startsWith('build/')),
      'untracked ignored compiler/embed input is not a zero-overlay source');
  }
  return {
    schema: 'polaris-mobile-source-observation-v1', sourceCommit: source.sourceCommit,
    sourceTree: source.sourceTree, sourceTag: source.sourceTag, sourceTagObject: source.sourceTagObject,
    sourceOverlay: 0, inputContractSha256: createHash('sha256').update(raw).digest('hex'), artifactAcceptance: false,
    reviewedSource: reviewedSource(root, source),
  };
}

function sourceCheckout(root, raw, prepare, generatorOutputs = false) {
  const source = validateSourceBytes(raw);
  demand(source.declaredTagsSource?.path === sourceTagsPath && hex(source.declaredTagsSource.sha256, 64),
    'source-owned tags declaration differs');
  if (prepare) {
    root = resolve(root);
    // The caller owns the private parent. Never follow an alternate checkout
    // or create the source through a redirected parent or leaf.
    demand(realpathSync(dirname(root)) === dirname(root), 'mobile source slot parent is redirected');
    const slot = lstatSync(root, { throwIfNoEntry: false });
    demand(!slot || slot.isDirectory(), 'mobile source slot is redirected or not a directory');
    if (!slot) {
      // Reserve the leaf exclusively before Git. An EEXIST race refuses here
      // rather than initializing a source directory owned by another producer.
      mkdirSync(root, { mode: 0o700 });
      sourceGit(['init', '--quiet', root], 'initialization', 3_600_000);
      const ref = 'refs/tags/' + source.sourceTag;
      sourceGit(['-C', root, 'fetch', '--depth=1', '--no-tags', source.sourceURL, source.sourceTagObject + ':' + ref],
        'immutable tag fetch', 3_600_000);
      sourceGit(['-C', root, 'checkout', '--detach', source.sourceCommit], 'detached checkout', 3_600_000);
    }
    demand(realpathSync(root) === root, 'mobile source slot is redirected');
  }
  return verifyCheckout(root, source, raw, generatorOutputs);
}

export function reviewedSource(root, source) {
  root = realpathSync(root);
  // Validate actual vendor bytes against the pinned Git inventory. The full
  // checkout entry additionally checks the tag, HEAD, status and zero overlay.
  const git = (...args) => utf8.decode(sourceGit(['-C', root, ...args], 'vendor verification'));
  review(realpathSync(git('rev-parse', '--show-toplevel').trim()) === root, 'Git worktree differs from actual source root');
  const policy = source.sourcePolicy, { metacubex: meta, vendors } = policy;
  const readInsideSource = (name, read) => {
    const path = join(root, name), parent = dirname(path);
    const checkParent = () => review(realpathSync(parent) === parent, 'reviewed source directory redirected: ' + name);
    checkParent();
    const result = read(path);
    checkParent();
    return result;
  };
  const control = (name, sha256, allowEmpty = false) => {
    const { bytes, pin } = readInsideSource(name, path => pinned(path, sourceControlBytes, false, { allowEmpty }));
    review(pin.sha256 === sha256, 'actual ' + name + ' differs');
    return bytes;
  };
  verifySourceModule(control('go.mod', policy.goModSha256), source);
  control('go.sum', policy.goSumSha256, true);
  control(meta.patchPath, meta.patchSha256);
  const binding = decode(control(meta.bindingPath, meta.bindingSha256), sourceControlBytes);
  const changed = ['conn.go', 'reality.go'];
  const expected = { schema: 'polaris-repository-module-replacement-v1', module: meta.module,
    upstreamRepository: 'https://github.com/MetaCubeX/utls', upstreamCommit: meta.upstreamCommit,
    upstreamVersion: meta.version, upstreamH1: meta.upstreamH1, upstreamModuleZipSHA256: meta.upstreamZipSha256,
    replacementPath: meta.replacementPath, patch: meta.patchPath, patchSHA256: meta.patchSha256 };
  review(object(binding) && Object.entries(expected).every(([key, value]) => binding[key] === value) &&
    exactList(binding.changedFiles, changed), 'Meta baseline/patch binding differs');
  const inventory = binding.files;
  review(object(inventory) && Object.keys(inventory).length === meta.replacementFileCount,
    'complete 355-file Meta inventory required');
  for (const [name, pin] of Object.entries(inventory)) {
    review(closed(pin, ['beforeSHA256', 'afterSHA256', 'bytes']) && hex(pin.beforeSHA256, 64) &&
      hex(pin.afterSHA256, 64) && Number.isSafeInteger(pin.bytes) && pin.bytes >= 0, 'Meta file pin differs: ' + name);
  }
  review(exactList(Object.keys(inventory).filter(name => inventory[name].beforeSHA256 !== inventory[name].afterSHA256).sort(), changed),
    'Meta changed-file inventory differs');

  let totalBytes = 0;
  const observed = {};
  for (const [module, pin] of Object.entries(vendors)) {
    const prefix = pin.replacementPath.slice(2);
    const tree = git('rev-parse', source.sourceCommit + ':' + prefix).trim();
    review(tree === pin.replacementTree, 'vendor subtree differs: ' + module);
    const rows = git('ls-tree', '-r', '-z', tree).split('\0');
    review(rows.pop() === '' && rows.length === pin.replacementFileCount, 'vendor inventory differs: ' + module);
    const entries = new Map();
    for (const row of rows) {
      const match = /^(100644|100755|120000) blob ([a-f0-9]{40})\t([^\\\x00-\x1f\x7f]+)$/.exec(row);
      review(match, 'vendor path or mode differs');
      const [, mode, blob, name] = match;
      const parts = name.split('/');
      review(parts.every(part => part && part !== '.' && part !== '..'), 'vendor path differs');
      const path = prefix + '/' + name;
      let bytes;
      if (mode === '120000') {
        review(path === 'third_party/tailscale/.stignore', 'unexpected vendor symlink');
        bytes = readInsideSource(path, file => readlinkSync(file, { encoding: 'buffer' }));
        review(bytes.equals(Buffer.from('.gitignore')), 'unexpected vendor symlink');
      } else {
        bytes = readInsideSource(path, file => readPinnedBytes(file, vendorFileBytes, false,
          { allowEmpty: true, executable: mode === '100755' }));
      }
      totalBytes += bytes.length;
      review(totalBytes <= vendorTotalBytes, 'vendor inventory exceeds byte budget');
      const digest = gitObject('blob', bytes);
      review(digest.toString('hex') === blob, 'vendor bytes differ: ' + path);
      if (module === meta.module) {
        const metaPin = inventory[name];
        review(metaPin && metaPin.bytes === bytes.length &&
          metaPin.afterSHA256 === createHash('sha256').update(bytes).digest('hex'), 'actual Meta bytes differ: ' + name);
      }
      const leaf = parts.pop();
      let parent = entries;
      for (const part of parts) {
        if (!parent.has(part)) parent.set(part, new Map());
        parent = parent.get(part);
        review(parent instanceof Map, 'vendor file/directory collision');
      }
      review(!parent.has(leaf), 'duplicate vendor tree entry');
      parent.set(leaf, { mode, digest });
    }
    review(vendorTree(entries).toString('hex') === tree, 'actual vendor subtree differs: ' + module);
    observed[module] = { path: pin.replacementPath, tree, files: rows.length };
  }
  // The closed source policy contains only ASCII keys and scalar string/integer values.
  const canonical = value => JSON.stringify(value, (_, item) => object(item)
    ? Object.fromEntries(Object.keys(item).sort().map(key => [key, item[key]])) : item);
  return {
    schema: 'polaris-reviewed-source-observation-v2',
    sourcePolicySha256: createHash('sha256').update(canonical(policy)).digest('hex'),
    replacementTree: meta.replacementTree, replacementFileCount: meta.replacementFileCount,
    bindingSha256: meta.bindingSha256, patchSha256: meta.patchSha256, vendors: observed,
    baselineH1Role: 'upstream provenance only; local replacement has no h1',
  };
}

function mobileLinkerFlagsPrefix(source) {
  return '-X github.com/sagernet/sing-box/constant.Version=' + source.version +
    ' -X github.com/sagernet/sing-box/constant.DirectoryRetirementSourceIdentity=' + source.sourceCommit +
    ' -X runtime.godebugDefault=multipathtcp=0,tlssha1=1 -checklinkname=0 -s -w -buildid=polaris-mobile-';
}

export function bindingPlans(source) {
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
  return { linkerFlagsPrefix: mobileLinkerFlagsPrefix(source), platforms };
}

// Android Go tool output and text from a validated Apple Mach-O section share
// this projection. The native observers own the original bytes and framing.
export function parseMobileBuildInfo(raw) {
  demand(typeof raw === 'string', 'native BuildInfo observation kind differs');
  const lines = raw.replace(/[\u0009-\u000d\u001c-\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+$/u, '')
    .split(/\r\n|[\n\r\u000b\u000c\u001c-\u001e\u0085\u2028\u2029]/u);
  const version = /: go(\p{Nd}+\.\p{Nd}+\.\p{Nd}+)$/u.exec(lines.shift());
  demand(version, 'Missing Go BuildInfo');
  const modules = new Map(), settings = new Map();
  let path = null, previous;
  for (const line of lines) {
    if (line === '\t') {
      previous = undefined;
      continue;
    }
    const [prefix, kind, value, first, second] = line.split('\t');
    demand(prefix === '' && value !== undefined, 'Malformed Go BuildInfo row');
    if (kind === 'mod' || kind === 'dep') {
      demand(first !== undefined && !modules.has(value), 'Duplicate/malformed module row');
      previous = { kind, version: first };
      if (second) previous.sum = second;
      modules.set(value, previous);
      continue;
    }
    if (kind === '=>') {
      demand(previous && !Object.hasOwn(previous, 'replacement') && value, 'Unbound/duplicate replacement');
      previous.replacement = value;
      if (first) previous.replacementVersion = first;
      if (second) previous.replacementSum = second;
    } else if (kind === 'build') {
      const at = value.indexOf('='), key = value.slice(0, at);
      demand(at > 0 && !settings.has(key), 'Malformed/duplicate build setting');
      const setting = value.slice(at + 1);
      settings.set(key, setting.startsWith('"') ? JSON.parse(setting) : setting);
    } else if (kind === 'path') {
      demand(path === null, 'Duplicate BuildInfo path');
      path = value;
    } else {
      throw new Error('Unsupported Go BuildInfo row');
    }
    previous = undefined;
  }
  return { goVersion: version[1], modules: Object.fromEntries(modules), settings: Object.fromEntries(settings), path };
}

const field = (value, key) => Object.hasOwn(value, key) ? value[key] : '';
const present = (value) => value != null && typeof value === 'object' ? Object.keys(value).length !== 0 : Boolean(value);
function declareNativeGraph(graph, module, vendors) {
  demand(Array.isArray(graph) && graph.every(row => object(row) && typeof row.Path === 'string'),
    'actual source graph module rows differ');
  const declared = new Map(graph.map(row => [row.Path, row]));
  demand(declared.size === graph.length, 'duplicate actual source graph module');
  demand(graph.every(row => !present(row.Replace) || object(row.Replace)), 'invalid graph replacement');
  const local = graph.filter(row => present(row.Replace) && !present(row.Replace.Version));
  demand(local.length === Object.keys(vendors).length && local.every(row => Object.hasOwn(vendors, row.Path)),
    'actual graph replacements differ');
  const source = declared.get(module);
  demand(source?.Main === true && typeof source.Dir === 'string', 'actual main source graph root differs');
  demand(posix.isAbsolute(source.Dir) && !source.Dir.split('/').includes('..'), 'absolute source graph root required');
  demand(!source.Dir.includes('\0'), 'absolute source graph root required');
  const root = realpathSync(source.Dir);
  const doubleSlash = /^\/\/[^/]/.test(source.Dir) || source.Dir === '//';
  for (const row of local) {
    const pin = vendors[row.Path], replacement = row.Replace;
    const expected = (doubleSlash ? '/' : '') + posix.join(source.Dir, pin.replacementPath.slice(2));
    demand(replacement.Path === pin.replacementPath && replacement.Dir === expected
      && statSync(expected, { throwIfNoEntry: false })?.isDirectory(), 'local graph path differs');
    const relative = posix.relative(root, realpathSync(expected));
    demand(relative !== '..' && !relative.startsWith('../') && !posix.isAbsolute(relative), 'local graph path differs');
    demand(!present(row.Main) && field(row, 'Sum') === '' && field(replacement, 'Sum') === ''
      && field(replacement, 'Version') === '', 'local replacement must not invent h1/version');
    demand(row.Version === pin.version, 'actual vendor graph version differs: ' + row.Path);
  }
  return declared;
}

export function checkMobileBuildInfo({ source: declaration, graph, target, expectedId, actualId, raw }) {
  const source = validateMobileSource(sourceDeclaration(declaration));
  const { osName, arch, mode, simulator } = target;
  const android = osName === 'android';
  demand(android
    ? mode === 'c-shared' && ['arm64', 'arm', '386', 'amd64'].includes(arch) && simulator === false
    : osName === 'ios' && mode === 'c-archive' && arch === 'arm64' && typeof simulator === 'boolean',
  'actual mobile BuildInfo target differs');
  const observed = parseMobileBuildInfo(raw);
  demand(observed.goVersion === source.goVersion, 'native Go version differs');
  const { settings, modules } = observed;
  demand(object(settings) && object(modules), 'actual native BuildInfo shape differs');
  for (const [key, expected] of Object.entries({
    GOOS: osName, GOARCH: arch, CGO_ENABLED: '1', '-buildmode': mode, '-trimpath': 'true',
  })) {
    demand(settings[key] === expected, 'actual native setting differs: ' + key);
  }
  const tags = new Set([...source[android ? 'android' : 'apple'].tags,
    ...(android ? [] : ['ios']), ...(simulator ? ['iossimulator'] : [])]);
  const actualTags = typeof settings['-tags'] === 'string' ? new Set(settings['-tags'].split(',')) : new Set();
  demand(tags.size === actualTags.size && [...tags].every(tag => actualTags.has(tag)), 'native feature tags differ');
  demand(typeof expectedId === 'string' && actualId === expectedId, 'actual source/tool/target BuildID input differs');
  // With trimpath Go can omit this setting. The tool/target binding and
  // actual BuildID are still required separately.
  if (Object.hasOwn(settings, '-ldflags')) {
    const fingerprint = expectedId.replace(/^polaris-mobile-/, '');
    demand(hex(fingerprint, 64) && settings['-ldflags'] === mobileLinkerFlagsPrefix(source) + fingerprint,
      'reported native ldflags differ');
  }
  const { metacubex, vendors } = source.sourcePolicy;
  const declared = declareNativeGraph(graph, source.goModule, vendors);
  // Go may prune unlinked vendors, while the complete source graph is required.
  for (const name of [metacubex.module, 'github.com/sagernet/sing-tun']) {
    demand(Object.hasOwn(modules, name), 'required local source module absent from native carrier: ' + name);
  }
  demand(declared.get(metacubex.module).Version === metacubex.version, 'actual Meta source graph version differs');
  const actualMeta = modules[metacubex.module];
  demand(actualMeta && actualMeta.version === metacubex.version && field(actualMeta, 'sum') === ''
    && actualMeta.replacement === metacubex.replacementPath && actualMeta.replacementVersion === '(devel)'
    && field(actualMeta, 'replacementSum') === '', 'actual Meta local replacement identity differs; baseline h1 is not a local sum');
  demand(Object.hasOwn(modules, source.goModule), 'original core source module absent from native carrier');
  for (const [name, linked] of Object.entries(modules)) {
    demand(object(linked), 'actual native module row differs');
    // Generated binding main packages are separate from source dependencies.
    if ((name === 'command-line-arguments' || name === 'gobind') && linked.kind === 'mod') continue;
    const expected = declared.get(name);
    demand(expected, 'native module not in actual source graph: ' + name);
    if (Object.hasOwn(vendors, name)) {
      demand(linked.kind === 'dep' && field(linked, 'sum') === '', 'native local module must not invent h1 or main role: ' + name);
    }
    if (present(expected.Main)) {
      if (linked.kind === 'dep') {
        demand(linked.replacement === expected.Dir && linked.version === 'v0.0.0-00010101000000-000000000000',
          'generated binding must point at the exact verified source module');
        continue;
      }
      demand(linked.version === '(devel)' || linked.version === '', 'main module version differs');
    } else {
      demand(linked.version === expected.Version, 'native module version differs: ' + name);
    }
    const replacement = expected.Replace;
    if (present(replacement)) {
      const diagnostic = 'native replacement differs: ' + name + '; observed=' + JSON.stringify(linked)
        + '; declaredPath=' + JSON.stringify(replacement.Path) + '; declaredDir=' + JSON.stringify(replacement.Dir);
      const versioned = present(replacement.Version);
      demand(typeof replacement.Path === 'string' && (linked.replacement === replacement.Path
        || (!versioned && linked.replacement === replacement.Dir)), diagnostic);
      // Go can emit (devel) for a versionless replacement in a linked binary.
      demand((versioned ? [replacement.Version] : ['', '(devel)']).includes(field(linked, 'replacementVersion')),
        'replacement version differs; ' + diagnostic);
      demand(field(linked, 'replacementSum') === field(replacement, 'Sum'), 'replacement h1 differs; ' + diagnostic);
    } else {
      demand(!Object.hasOwn(linked, 'replacement'), 'undeclared native module replacement');
      if (present(expected.Sum)) demand(linked.sum === expected.Sum, 'native module h1 differs: ' + name);
    }
  }
  return observed;
}

export function aarGoInvocationIdentity(raw, version, basename) {
  demand(typeof raw === 'string' && typeof version === 'string' && /^\p{Nd}+\.\p{Nd}+\.\p{Nd}+$/u.test(version),
    'raw AAR Go BuildInfo observation differs');
  const end = raw.indexOf('\n'), header = raw.slice(0, end), body = raw.slice(end + 1);
  const suffix = ': go' + version;
  demand(end >= 0 && body.startsWith('\t') && header.endsWith(suffix), 'raw AAR Go BuildInfo header/body differs');
  const filename = header.slice(0, -suffix.length);
  // Ignore redundant slash and dot components without resolving parent components.
  demand(filename.startsWith('/') && !filename.includes('\0') &&
    filename.split('/').findLast(part => part && part !== '.') === basename, 'raw AAR Go BuildInfo filename differs');
  return 'go' + version + '\n' + body;
}

function nativeBuildInfoReport(evidence) {
  const androidFields = ['source', 'graph', 'target', 'expectedId', 'actualId', 'raw'];
  const aarReinspection = closed(evidence, [...androidFields, 'originalAarGoBuildInfo']);
  if ((aarReinspection || closed(evidence, androidFields)) && evidence.target?.osName === 'android') {
    const facts = checkMobileBuildInfo(evidence);
    if (aarReinspection) {
      const { targets, abis } = evidence.source.android;
      const abi = abis[targets.indexOf('android/' + evidence.target.arch)];
      const basename = 'jni_' + abi + '_libbox.so';
      demand(aarGoInvocationIdentity(evidence.raw, facts.goVersion, basename) ===
        aarGoInvocationIdentity(evidence.originalAarGoBuildInfo, facts.goVersion, basename),
      'original AAR Go BuildInfo body differs');
    }
    return facts;
  }
  const bound = closed(evidence, ['source', 'graph', 'appleObservations']);
  demand((bound || closed(evidence, ['appleObservations'])) && Array.isArray(evidence.appleObservations)
    && evidence.appleObservations.length > 0, 'invalid Apple BuildInfo observations');
  return evidence.appleObservations.map(row => {
    if (closed(row, ['raw'])) return parseMobileBuildInfo(row.raw);
    demand(bound && closed(row, ['target', 'expectedId', 'actualId', 'raw']) && row.target?.osName === 'ios',
      'invalid bound Apple BuildInfo observation');
    return checkMobileBuildInfo({ ...row, source: evidence.source, graph: evidence.graph });
  });
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
    } else if ((args.length === 2 || args.length === 3 && args[2] === '--generator-outputs') && args[0] === '--verify-checkout') {
      console.log(JSON.stringify(sourceCheckout(args[1], readBoundedInput(sourceContractBytes), false, args.length === 3)));
    } else if ((args.length === 3 || args.length === 4 && args[3] === '--generator-outputs') && args[0] === '--verify-checkout-file') {
      console.log(JSON.stringify(sourceCheckout(args[1], pinned(args[2], sourceContractBytes).bytes, false, args.length === 4)));
    } else if (args.length === 3 && args[0] === '--prepare-checkout') {
      console.log(JSON.stringify(sourceCheckout(args[1], pinned(args[2], sourceContractBytes).bytes, true)));
    } else if (args.length === 2 && args[0] === '--module-list') {
      console.log(moduleList(readPinnedBytes(args[1], limits.controlBytes, false, { allowEmpty: true })));
    } else if (args.length === 4 && args[0] === '--native-buildinfo') {
      const [, path, size, digest] = args;
      const { bytes, pin } = pinned(path, limits.controlBytes);
      demand(pin.bytes === Number(size) && pin.sha256 === digest, 'native BuildInfo evidence pin differs');
      const evidence = decode(bytes, limits.controlBytes, () => limits.controlBytes);
      console.log(JSON.stringify(nativeBuildInfoReport(evidence)));
    } else {
      // The producer sends its full declaration once and reuses these plans.
      demand(args.length === 0, 'usage: source-admission.mjs [--versions | --verify-checkout ROOT [--generator-outputs] | --verify-checkout-file ROOT DECLARATION [--generator-outputs] | --prepare-checkout ROOT DECLARATION | --module-list PATH | --native-buildinfo PATH BYTES SHA256]');
      console.log(JSON.stringify(bindingPlans(validateSourceBytes(readBoundedInput(sourceContractBytes)))));
    }
  } catch (error) {
    console.error(error instanceof SyntaxError ? 'invalid mobile source declaration JSON' : error.message);
    process.exitCode = 1;
  }
}
