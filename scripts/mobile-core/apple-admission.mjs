import { createHash } from 'node:crypto';
import { constants, lstatSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { pinned, readBoundedInput } from './source-admission.mjs';

const SDKS = new Map([['device-arm64', 'iphoneos'], ['sim-arm64', 'iphonesimulator']]);
const APP = 'polaris_iOS';
const EXTENSION = 'polaris_PacketTunnel';
const TARGETS = [APP, EXTENSION];
const FIELDS = ['ARCHS', 'VALID_ARCHS', 'EXCLUDED_ARCHS'];
const ARCHITECTURES = new Set(['arm64', 'arm64e', 'armv7', 'armv7s', 'i386', 'x86_64']);
const FLOAT = Symbol('float');
// Match the whitespace accepted by the original resolved-settings parser.
const WHITESPACE = /[\u0009-\u000d\u001c-\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+/;

class AdmissionError extends Error {}

function requireSetting(condition, message) {
  if (!condition) throw new AdmissionError(message);
}

function isObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function pin(raw) {
  return { bytes: raw.length, sha256: createHash('sha256').update(raw).digest('hex') };
}

function architectureValue(value) {
  if (value == null) return { type: 'NoneType' };
  if (typeof value === 'boolean') return { type: 'bool' };
  if (value === FLOAT) return { type: 'float' };
  if (typeof value === 'number') return { type: 'int' };
  if (Array.isArray(value)) return { type: 'list' };
  if (isObject(value)) return { type: 'dict' };

  // Preserve the observation digest for lone surrogates in JSON strings.
  const loneSurrogate = /([\uD800-\uDFFF])/u;
  const raw = loneSurrogate.test(value)
    ? Buffer.concat(value.split(loneSurrogate).map((part, index) => {
      if (index % 2 === 0) return Buffer.from(part);
      const unit = part.charCodeAt(0);
      return Buffer.from([0xed, 0x80 | ((unit >> 6) & 0x3f), 0x80 | (unit & 0x3f)]);
    }))
    : Buffer.from(value);
  const visible = value.length <= 256 && !/[^A-Za-z0-9_$(){}.+\- \t\n\r\v\f]/.test(value);
  return { type: 'str', encoding: 'utf8-surrogatepass', ...pin(raw),
    ...(visible ? { value } : { valueWithheld: true }) };
}

function architectureObservation(rows, raw, slot, query) {
  const observed = Array.isArray(rows) ? rows.slice(0, 3).map((row) => {
    if (!isObject(row)) return { rowShape: 'not-object' };
    const target = TARGETS.includes(row.target) ? row.target : 'other';
    const settings = row.buildSettings;
    if (!isObject(settings)) return { target, settingsShape: 'not-object' };
    const architectureSettings = Object.fromEntries(FIELDS.map((key) =>
      [key, { present: Object.hasOwn(settings, key), ...architectureValue(settings[key]) }]));
    return { target, architectureSettings };
  }) : [];
  return { schema: 'polaris-apple-architecture-observation-v1', slot, query, rows: observed,
    rowCount: Array.isArray(rows) ? rows.length : null, normalizedResponseText: pin(raw),
    componentAccepted: false, devicePassed: false,
    scope: 'selected architecture fields before admission; full settings/environment not retained' };
}

function checkedBuildSettings(rows, sdk, minimum, expectedTargets) {
  requireSetting(Array.isArray(rows) && rows.length === expectedTargets.length, 'actual Apple target set differs');
  const selected = {};
  for (const row of rows) {
    requireSetting(isObject(row) && expectedTargets.includes(row.target) && !Object.hasOwn(selected, row.target),
      'actual Apple target set differs');
    const { target, buildSettings: settings } = row;
    requireSetting(isObject(settings), 'actual Apple target build settings required');
    const fields = {};
    for (const field of FIELDS) {
      const value = !Object.hasOwn(settings, field) && field === 'EXCLUDED_ARCHS' ? '' : settings[field];
      const tokens = typeof value === 'string' ? value.split(WHITESPACE).filter(Boolean) : [];
      const reason = typeof value !== 'string' ? 'non-string'
        : tokens.some((token) => !ARCHITECTURES.has(token)) ? 'unknown-token'
          : new Set(tokens).size !== tokens.length ? 'duplicate-token' : null;
      if (reason) {
        throw new AdmissionError('unresolved Apple architecture setting: ' +
          JSON.stringify({ target, field, reason, observed: architectureValue(value) }));
      }
      fields[field] = tokens;
    }
    requireSetting(fields.ARCHS.length === 1 && fields.ARCHS[0] === 'arm64' &&
      fields.VALID_ARCHS.length === 1 && fields.VALID_ARCHS[0] === 'arm64' && !fields.EXCLUDED_ARCHS.includes('arm64'),
    'selected Apple architecture is excluded by actual build settings');
    requireSetting(settings.PLATFORM_NAME === sdk && settings.CONFIGURATION === 'release',
      'actual Apple platform/configuration differs');
    const sdkRoot = settings.SDKROOT;
    const sdkName = typeof sdkRoot === 'string' && sdkRoot.startsWith('/')
      ? sdkRoot.split('/').filter((part) => part && part !== '.').at(-1) : '';
    const sdkPattern = sdk === 'iphoneos' ? /^iPhoneOS[0-9]+(?:\.[0-9]+)*\.sdk$/ : /^iPhoneSimulator[0-9]+(?:\.[0-9]+)*\.sdk$/;
    requireSetting(sdkRoot === sdk || sdkPattern.test(sdkName), 'actual Apple SDK root differs');
    requireSetting(settings.IPHONEOS_DEPLOYMENT_TARGET === minimum, 'actual Apple deployment version differs');
    if (sdk === 'iphoneos' && target === APP) {
      requireSetting(fields.EXCLUDED_ARCHS.includes('x86_64'), 'device Intel exclusion must remain');
    }
    selected[target] = { target, platform: sdk, requestedArchitecture: 'arm64', effectiveArchitectures: ['arm64'],
      deploymentTarget: minimum, ...fields };
  }
  return selected;
}

// Scheme results can defer a settings rejection until the explicit extension
// has been observed and admitted. Only sanitized data crosses between queries.
export function inspectAppleArchitecture(raw, { slot, query, minimum, scheme }) {
  const sdk = SDKS.get(slot);
  requireSetting(sdk && (query === 'scheme' || query === 'extension'), 'closed Apple settings observation required');
  let rows;
  try {
    rows = JSON.parse(raw.toString('utf8'), (key, value, context) =>
      typeof value === 'number' && FIELDS.includes(key) && /[.eE]/.test(context.source) ? FLOAT : value);
  } catch (error) {
    if (!(error instanceof SyntaxError)) throw error;
    return { error: 'invalid resolved Apple settings JSON' };
  }
  const observation = architectureObservation(rows, raw, slot, query);
  if (query === 'scheme' && (!Array.isArray(rows) || !rows.every(isObject))) {
    return { observation, error: 'actual Apple build-settings array required' };
  }
  try {
    if (query === 'scheme') {
      const targets = rows.length === 1 && rows[0].target === APP ? [APP] : TARGETS;
      return { observation, admission: checkedBuildSettings(rows, sdk, minimum, targets) };
    }
    const extension = checkedBuildSettings(rows, sdk, minimum, [EXTENSION]);
    if (typeof scheme === 'string') return { observation, error: scheme };
    if (Object.hasOwn(scheme, EXTENSION)) {
      requireSetting(JSON.stringify(scheme[EXTENSION]) === JSON.stringify(extension[EXTENSION]),
        'scheme and explicit extension settings differ');
    }
    return { observation, admission: { ...scheme, ...extension } };
  } catch (error) {
    if (!(error instanceof AdmissionError)) throw error;
    return query === 'scheme' ? { observation, admission: error.message } : { observation, error: error.message };
  }
}

const NATIVE_TUN_ARGS = 'PolarisNativeTunCompiler.args';
const NATIVE_TUN_CONTRACT = 'polaris-native-tun-v1';
const NATIVE_TUN_DEFINE = Buffer.from('-D\nPOLARIS_NATIVE_TUN_V1\n');
const HEADER_BYTES = 2 * 1024 ** 2;
const ARG_BYTES = 128;

// Match the Python parser's whitespace, word boundaries, and LF-only multiline anchors.
const TUN_SPACE = '\\u0009-\\u000d\\u001c-\\u0020\\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000';
const TUN_WS = `[${TUN_SPACE}]`;
const TUN_WORD = '[\\p{L}\\p{N}_]';
const TUN_WORD_END = `(?!${TUN_WORD})`;
const TUN_LINE_START = '(?:^|(?<=\\n))';
const TUN_TRIM = new RegExp(`^${TUN_WS}+|${TUN_WS}+$`, 'g');
const TUN_NAMES = new RegExp(`(?<!${TUN_WORD})(?:LibboxRuntimeTun(?:Platform|Session|Receipt)|setRuntimeTunPlatform|newTunSession)${TUN_WORD_END}`, 'u');
const trimTunSpace = text => text.replace(TUN_TRIM, '');

// Exact selectors from the pinned sagernet/gomobile v0.1.13 generator.
const TUN_PLATFORM = {
  runtimeTunVersion: '- (int32_t)runtimeTunVersion;',
  newTunSession: '- (id<LibboxRuntimeTunSession> _Nullable)newTunSession:'
    + '(id<LibboxTunOptions> _Nullable)options previous:'
    + '(id<LibboxRuntimeTunSession> _Nullable)previous receipt:'
    + '(LibboxRuntimeTunReceipt* _Nullable)receipt error:'
    + '(NSError* _Nullable* _Nullable)error;',
};
const OWNER_ID = '- (NSString* _Nonnull)ownerID;';
const TUN_SESSION = {
  ownerID: OWNER_ID,
  fileDescriptor: '- (int32_t)fileDescriptor;',
  ...Object.fromEntries(['prepare', 'validate', 'abort', 'retire'].map(name =>
    [name, '- (BOOL)' + name + ':(NSError* _Nullable* _Nullable)error;'])),
};

function tunDeclaration(text, kind, name, protocols) {
  const bodies = [];
  const declaration = new RegExp(`@${kind}${TUN_WS}+${name}${TUN_WORD_END}([^\\n]*)`, 'gu');
  for (const match of text.matchAll(declaration)) {
    const tail = trimTunSpace(match[1]);
    if (tail.startsWith(';')) continue; // A forward declaration supplies no methods.
    const remainder = text.slice(match.index + match[0].length);
    const end = remainder.search(new RegExp(`@end${TUN_WORD_END}`, 'u'));
    requireSetting(end >= 0, 'native TUN declaration is unterminated: ' + name);
    const body = remainder.slice(0, end);
    requireSetting(!new RegExp(`@(?:protocol|interface)${TUN_WORD_END}`, 'u').test(body),
      'native TUN declaration overlaps another block: ' + name);
    if (kind === 'protocol') {
      requireSetting(new RegExp(`^<${TUN_WS}*NSObject${TUN_WS}*>$`).test(tail),
        'native TUN protocol inheritance differs: ' + name);
    } else {
      const inheritance = new RegExp(`^:${TUN_WS}*NSObject${TUN_WS}*<([^>]+)>${TUN_WS}*\\{?$`).exec(tail);
      requireSetting(inheritance, 'native TUN class inheritance differs: ' + name);
      const actual = inheritance[1].split(',').map(trimTunSpace);
      requireSetting(new Set(actual).size === actual.length && protocols.every(item => actual.includes(item)),
        'native TUN class protocol identity differs: ' + name);
    }
    bodies.push(body);
  }
  requireSetting(bodies.length === 1, 'native TUN declaration missing or duplicated: ' + name);
  return bodies[0];
}

function tunMethods(body, expected) {
  const declarations = body.match(new RegExp(`[-+]${TUN_WS}*\\([^;]*?;`, 'g')) ?? [];
  const token = new RegExp(`@?[A-Za-z_]${TUN_WORD}*|[^${TUN_SPACE}]`, 'gu');
  const tokens = value => value.match(token).join(' ');
  for (const [name, declaration] of Object.entries(expected)) {
    const selector = new RegExp(`^[-+]${TUN_WS}*\\([^)]*\\)${TUN_WS}*${name}${TUN_WORD_END}`, 'u');
    const candidates = declarations.filter(part => selector.test(part));
    requireSetting(candidates.length === 1 && tokens(candidates[0]) === tokens(declaration),
      'native TUN selector/return ABI differs: ' + name);
  }
}

function nativeTunABI(headers) {
  const texts = headers.map(text => text.replace(/\/\*[\s\S]*?\*\/|\/\/[^\n]*/g, ' '));
  const relevant = texts.filter(text => TUN_NAMES.test(text));
  if (relevant.length === 0) return false;
  const conditional = new RegExp(`${TUN_LINE_START}${TUN_WS}*#${TUN_WS}*(if|ifdef|ifndef|elif|else|endif)${TUN_WORD_END}([^\\n]*)`, 'gu');
  for (const text of relevant) {
    const conditions = [...text.matchAll(conditional)];
    if (!conditions.length) continue;
    const guard = trimTunSpace(conditions[0][2]);
    // Only the generated include guard may condition these declarations.
    requireSetting(conditions.length === 2 && conditions[0][1] === 'ifndef' && /^__[A-Za-z0-9_]+_H__$/.test(guard)
      && conditions[1][1] === 'endif' && !trimTunSpace(conditions[1][2])
      && new RegExp(`${TUN_LINE_START}${TUN_WS}*#${TUN_WS}*define${TUN_WS}+${guard}${TUN_WS}*(?=\\n|$)`).test(text),
    'native TUN declarations use unsupported preprocessor conditions');
  }
  const text = texts.map(part => part.replaceAll('\\\n', '').replace(/(?:^|(?<=\n))[ \t]*#[^\n]*/g, '')).join('\n');
  for (const [name, methods] of [['LibboxRuntimeTunPlatform', TUN_PLATFORM], ['LibboxRuntimeTunSession', TUN_SESSION]]) {
    tunMethods(tunDeclaration(text, 'protocol', name), methods);
    tunMethods(tunDeclaration(text, 'interface', name, ['goSeqRefInterface', name]), methods);
  }
  tunMethods(tunDeclaration(text, 'interface', 'LibboxRuntimeTunReceipt', ['goSeqRefInterface']),
    { ownerID: OWNER_ID, published: '- (BOOL)published;' });
  tunMethods(tunDeclaration(text, 'interface', 'LibboxCommandServer', ['goSeqRefInterface']),
    { setRuntimeTunPlatform: '- (BOOL)setRuntimeTunPlatform:'
      + '(id<LibboxRuntimeTunPlatform> _Nullable)platform error:(NSError* _Nullable* _Nullable)error;' });
  return true;
}

function writeCompilerArgs(path, bytes) {
  const info = lstatSync(path, { throwIfNoEntry: false });
  requireSetting(!info || info.isFile(), 'TLS/native compiler output redirected or is nonregular');
  const flag = constants.O_WRONLY | constants.O_CREAT | constants.O_TRUNC | constants.O_NOFOLLOW | constants.O_NONBLOCK;
  writeFileSync(path, bytes, { flag });
}

function readCompilerArgs(path) {
  return pinned(path, ARG_BYTES, false, { allowEmpty: true });
}

// Receive the exact selected header contents already pinned by the caller.
// Own only the native response file; the caller retains TLS and Xcode custody.
export function nativeTunCompilerResponse(action, { headers, prepared, derived, receipt }) {
  requireSetting(['prepare', 'verify-parent', 'install', 'verify-installed'].includes(action)
    && typeof prepared === 'string' && prepared.length > 0, 'closed native TUN compiler action required');
  requireSetting(Array.isArray(headers) && headers.length > 0 && headers.length <= 64
    && headers.every(text => typeof text === 'string' && text.isWellFormed())
    && headers.reduce((total, text) => total + Buffer.byteLength(text), 0) <= HEADER_BYTES,
  'libbox generated header inventory exceeds budget');
  const present = nativeTunABI(headers);
  const args = present ? NATIVE_TUN_DEFINE : Buffer.alloc(0);
  const compilerArgs = pin(args);
  const result = { contractVersion: NATIVE_TUN_CONTRACT, present, compilerArgs };
  const original = join(prepared, NATIVE_TUN_ARGS);
  if (action === 'prepare') {
    writeCompilerArgs(original, args);
    requireSetting(readCompilerArgs(original).bytes.equals(args), 'prepared native TUN compiler response file changed');
    return result;
  }
  requireSetting(isObject(receipt) && Object.keys(receipt).length === 3
    && receipt.contractVersion === NATIVE_TUN_CONTRACT && receipt.present === present && Object.hasOwn(receipt, 'compilerArgs'),
  'prepared native TUN ABI decision differs from actual headers');
  const parent = readCompilerArgs(original);
  requireSetting(isObject(receipt.compilerArgs) && Object.keys(receipt.compilerArgs).length === 2
    && receipt.compilerArgs.bytes === parent.pin.bytes && receipt.compilerArgs.sha256 === parent.pin.sha256,
  'parent native TUN compiler response file changed');
  requireSetting(parent.bytes.equals(args), 'unexpected native TUN compiler arguments');
  if (action === 'verify-parent') return result;

  requireSetting(typeof derived === 'string' && derived.length > 0, 'derived native TUN response directory required');
  const output = join(derived, NATIVE_TUN_ARGS);
  if (action === 'install') {
    writeCompilerArgs(output, parent.bytes);
  } else {
    requireSetting(readCompilerArgs(output).bytes.equals(parent.bytes),
      'installed native TUN compiler input changed');
  }
  return result;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [, , slot, query, minimum, scheme] = process.argv;
  if (slot === '--native-tun') {
    try {
      // A JSON escape can expand one header byte to six; paths are separate argv.
      const { headers, receipt } = JSON.parse(readBoundedInput(6 * HEADER_BYTES + 4096).toString('utf8'));
      process.stdout.write(JSON.stringify(nativeTunCompilerResponse(query, { headers, receipt, prepared: minimum, derived: scheme })));
    } catch (error) {
      console.error(error.message);
      process.exitCode = 1;
    }
  } else {
    const result = inspectAppleArchitecture(readFileSync(0), { slot, query, minimum, scheme: JSON.parse(scheme) });
    process.stdout.write(JSON.stringify(result));
  }
}
