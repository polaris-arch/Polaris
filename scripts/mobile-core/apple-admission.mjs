import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, constants, fsyncSync, lstatSync, mkdirSync, openSync, readFileSync, readdirSync,
  realpathSync, renameSync, rmSync, statSync, writeFileSync, writeSync } from 'node:fs';
import { loadavg } from 'node:os';
import { performance } from 'node:perf_hooks';
import { basename, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

import { characters, decode, pinned, readBoundedInput, ReceiptFloat, receiptLimit, receiptPolicy } from './source-admission.mjs';

const limits = receiptPolicy.receipt;
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

function codepointOrder(left, right) {
  for (let at = 0; at < Math.min(left.length, right.length);) {
    const a = left.codePointAt(at), b = right.codePointAt(at);
    if (a !== b) return a - b;
    at += a > 0xffff ? 2 : 1;
  }
  return left.length - right.length;
}

function serialize(value, maximum, pretty, consume = () => {}) {
  let bytes = 0, nodes = 0, stringChars = 0, buffer = '';
  function push(token) {
    bytes += Buffer.byteLength(token);
    requireSetting(bytes <= maximum, 'receipt JSON byte budget');
    buffer += token;
    if (buffer.length >= 64 * 1024) { consume(buffer); buffer = ''; }
  }
  function encode(entry, depth, isKey = false) {
    requireSetting(++nodes <= limits.nodes && depth <= limits.depth, 'receipt JSON node/depth budget');
    if (entry instanceof ReceiptFloat) push(floatText(entry.value));
    else if (typeof entry === 'string') {
      const length = characters(entry);
      stringChars += length;
      requireSetting(length <= (isKey ? limits.keyChars : limits.stringChars) && stringChars <= maximum, 'receipt JSON string budget');
      requireSetting(pretty || entry.isWellFormed(), 'invalid Unicode in receipt JSON');
      const quoted = JSON.stringify(entry);
      push(pretty ? quoted.replace(/[\u007f-\uffff]/g, char => '\\u' + char.charCodeAt(0).toString(16).padStart(4, '0')) : quoted);
    } else if (Array.isArray(entry) || isObject(entry)) {
      const array = Array.isArray(entry);
      const keys = array ? entry.keys() : Object.keys(entry).sort(codepointOrder);
      push(array ? '[' : '{');
      let first = true;
      for (const key of keys) {
        if (!first) push(',');
        if (pretty) push('\n' + ' '.repeat(2 * (depth + 1)));
        if (!array) { encode(key, depth + 1, true); push(pretty ? ': ' : ':'); }
        encode(entry[key], depth + 1);
        first = false;
      }
      if (pretty && !first) push('\n' + ' '.repeat(2 * depth));
      push(array ? ']' : '}');
    } else if (typeof entry === 'bigint') push(entry.toString());
    else if (entry === null || typeof entry === 'boolean' || Number.isSafeInteger(entry)) push(JSON.stringify(entry));
    else throw new Error('unsupported receipt JSON value');
  }
  encode(value, 0);
  if (pretty) push('\n');
  if (buffer) consume(buffer);
  return bytes;
}

export function fingerprint(value) {
  const hash = createHash('sha256');
  serialize(value, receiptLimit(value), false, chunk => hash.update(chunk));
  return hash.digest('hex');
}

export function writeReceipt(path, facts) {
  const maximum = receiptLimit(facts);
  let size;
  try { size = serialize(facts, maximum, true); }
  catch (error) {
    console.error(JSON.stringify({ schema: 'polaris-receipt-size-observation-v1', phase: 'bounded-serialization-rejected',
      path, maximumBytes: maximum, error: error.message, scope: 'no full serialized bytes produced or installed' }));
    throw error;
  }
  console.error(JSON.stringify({
    schema: 'polaris-receipt-size-observation-v1', phase: 'serialize-before-install',
    path, bytes: size, maximumBytes: maximum, receiptSchema: facts.schema,
    canonicalValueBytesByTopLevelField: Object.fromEntries(Object.entries(facts).map(([key, value]) =>
      [key, serialize(value, maximum, false)])),
    scope: 'actual serialized size; not installation or carrier acceptance',
  }));
  const temporary = path + '.tmp';
  const fd = openSync(temporary, 'wx');
  try {
    try {
      const written = serialize(facts, maximum, true, chunk => {
        const bytes = Buffer.from(chunk);
        for (let at = 0; at < bytes.length;) at += writeSync(fd, bytes, at, bytes.length - at);
      });
      requireSetting(written === size, 'receipt changed during serialization');
      fsyncSync(fd);
    } finally { closeSync(fd); }
    renameSync(temporary, path);
  } catch (error) {
    rmSync(temporary, { force: true });
    throw error;
  }
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

const XCODE_RESPONSE_BYTES = 2 * 1024 ** 2;
const XCODE_DEADLINE_MS = 3600 * 1000;
const XCODE_TERMINATION_MS = 10 * 1000;
const xcodeDecoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });

function runXcodeSettings(args, { repo, env, slot }) {
  return new Promise((complete, reject) => {
    const started = performance.now();
    const child = spawn('xcodebuild', args, { cwd: repo, env, detached: true, stdio: ['ignore', 'pipe', 'pipe'] });
    const output = [];
    let stdoutBytes = 0, stderrBytes = 0, stderrTail = Buffer.alloc(0);
    let failure, terminateTimer, heartbeat;

    const observe = event => {
      const processRunning = child.exitCode === null && child.signalCode === null;
      writeSync(1, 'APPLE_TOOL_PROGRESS=' + JSON.stringify({
        event, phase: 'build-settings-admission', slot, pid: child.pid,
        elapsedSeconds: Math.round(performance.now() - started) / 1000,
        stdoutBytes, stderrBytes, processRunning, load1: Math.round(loadavg()[0] * 1000) / 1000,
        peakRSSBytes: null, scope: 'direct process/pipe bytes only; no descendant liveness or acceptance',
      }) + '\n');
    };

    function signalGroup(signal) {
      if (!child.pid) return false;
      try { process.kill(-child.pid, signal); return true; }
      catch (error) {
        if (error.code !== 'ESRCH') failure ??= new Error('Xcode build-settings process group retirement failed');
        return false;
      }
    }

    function stop(cause) {
      if (failure) return;
      failure = cause;
      signalGroup('SIGTERM');
      terminateTimer = setTimeout(() => signalGroup('SIGKILL'), XCODE_TERMINATION_MS);
    }

    function progress(event) {
      try { observe(event); } catch (error) { stop(error); }
    }

    const interrupt = () => stop(new Error('Apple build-settings observation interrupted'));
    const deadline = setTimeout(() => stop(new Error('Apple build-settings observation deadline exceeded')), XCODE_DEADLINE_MS);
    process.on('SIGTERM', interrupt);
    process.on('SIGINT', interrupt);
    child.once('spawn', () => {
      progress('start');
      progress('waiting');
      heartbeat = setInterval(() => { if (!failure) progress('waiting'); }, 30 * 1000);
    });
    child.on('error', stop);
    child.stdout.on('error', stop);
    child.stderr.on('error', stop);
    child.stdout.on('data', chunk => {
      stdoutBytes += chunk.length;
      // Normalizing CRLF can shrink the source by at most half.
      if (stdoutBytes > 2 * XCODE_RESPONSE_BYTES) stop(new Error('Apple build-settings response exceeds observation bound'));
      else if (!failure) output.push(chunk);
    });
    child.stderr.on('data', chunk => {
      stderrBytes += chunk.length;
      stderrTail = Buffer.concat([stderrTail, chunk]).subarray(-4 * 8192);
    });
    child.once('close', (code) => {
      clearTimeout(deadline);
      clearTimeout(terminateTimer);
      clearInterval(heartbeat);
      process.off('SIGTERM', interrupt);
      process.off('SIGINT', interrupt);
      if (signalGroup(0)) {
        failure ??= new Error('native session still owns descendants; Apple build-settings rejected');
        signalGroup('SIGKILL');
      }
      if (failure) { reject(failure); return; }
      try {
        observe('end');
        requireSetting(code === 0, 'actual tool failed (xcodebuild): ' + stderrTail.toString('utf8').slice(-8192));
        complete(Buffer.from(xcodeDecoder.decode(Buffer.concat(output)).replace(/\r\n?/g, '\n')));
      } catch (error) {
        reject(error);
      }
    });
  });
}

async function admitResolvedSettings(repo, output, env) {
  const scope = env.POLARIS_APPLE_SCOPE ?? 'device-only';
  requireSetting(Object.hasOwn(receiptPolicy.appleScopes, scope), 'unknown Apple carrier scope');
  const slots = receiptPolicy.appleScopes[scope];
  const project = join(repo, 'src-tauri/gen/apple/polaris.xcodeproj');
  const config = join(repo, 'src-tauri/tauri.ios.conf.json');
  const inputs = [join(project, 'project.pbxproj'), join(project, 'xcshareddata/xcschemes/polaris_iOS.xcscheme'), config];
  const { bytes, pin: configPin } = pinned(config, limits.artifactBytes);
  const minimum = decode(bytes, limits.artifactBytes)?.bundle?.iOS?.minimumSystemVersion;
  requireSetting(typeof minimum === 'string' && /^[0-9]+\.[0-9]+(?:\.[0-9]+)?$/.test(minimum),
    'explicit iOS deployment version required');
  const inputPins = [...inputs.slice(0, -1).map(path => pinned(path, limits.artifactBytes).pin), configPin];

  async function inspect(query, slot, command, scheme) {
    const raw = await runXcodeSettings(command, { repo, env, slot });
    requireSetting(raw.length <= XCODE_RESPONSE_BYTES, 'Apple build-settings response exceeds observation bound');
    const checked = inspectAppleArchitecture(raw, { slot, query, minimum, scheme });
    if (checked.observation) writeReceipt(join(output, 'build-settings-' + slot + '-' + query + '.json'), checked.observation);
    requireSetting(!Object.hasOwn(checked, 'error'), checked.error);
    return checked.admission;
  }

  const observations = {};
  for (const slot of slots) {
    const sdk = SDKS.get(slot), destination = sdk === 'iphoneos' ? 'iOS' : 'iOS Simulator';
    const common = ['-project', project, '-configuration', 'release', '-sdk', sdk,
      '-showBuildSettings', '-json', 'ARCHS=arm64', 'ONLY_ACTIVE_ARCH=YES',
      'CODE_SIGNING_ALLOWED=NO', 'CODE_SIGNING_REQUIRED=NO'];
    const scheme = await inspect('scheme', slot, ['-scheme', APP, '-destination', 'generic/platform=' + destination, ...common]);
    observations[slot] = await inspect('extension', slot, ['-target', EXTENSION, ...common], scheme);
  }
  requireSetting(inputs.every((path, i) => matchesPin(inputPins[i], pinned(path, limits.artifactBytes).pin)),
    'Apple admission inputs changed during inspection');
  writeReceipt(join(output, 'build-settings-admission.json'), {
    schema: 'polaris-apple-build-settings-admission-v2', slots: observations,
    requiredSlots: slots, observedSlots: Object.keys(observations), inputPins,
    scope: 'resolved configuration only; no compiler/carrier/device acceptance',
  });
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
const stripHeaderComments = text => text.replace(/\/\*[\s\S]*?\*\/|\/\/[^\n]*/g, ' ');
const stripHeaderDirectives = text => text.replaceAll('\\\n', '').replace(/(?:^|(?<=\n))[ \t]*#[^\n]*/g, '');

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
  const texts = headers.map(stripHeaderComments);
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
  const text = texts.map(stripHeaderDirectives).join('\n');
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

function checkCompilerOutput(path) {
  const info = lstatSync(path, { throwIfNoEntry: false });
  requireSetting(!info || info.isFile(), 'TLS/native compiler output redirected or is nonregular');
}

function writeCompilerArgs(path, bytes) {
  checkCompilerOutput(path);
  const flag = constants.O_WRONLY | constants.O_CREAT | constants.O_TRUNC | constants.O_NOFOLLOW | constants.O_NONBLOCK;
  writeFileSync(path, bytes, { flag });
}

function readCompilerArgs(path) {
  return pinned(path, ARG_BYTES, false, { allowEmpty: true });
}

// Receive the exact selected header contents already pinned by the caller.
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
  requireSetting(matchesPin(receipt.compilerArgs, parent.pin), 'parent native TUN compiler response file changed');
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

const TLS_FLAGS = 'PolarisTLSFingerprintLinker.args';
const TLS_RECEIPT = 'PolarisTLSFingerprintLinker.json';
const TLS_RECEIPT_BYTES = 65536;
const TLS_FLAGS_BYTES = 256;
const TLS_FUNCTION = 'LibboxTLSFingerprintCapabilities';
const TLS_FLAG_CONTENT = Buffer.from(`-Xlinker\n-u\n-Xlinker\n_${TLS_FUNCTION}\n`);
const RECEIPT_LIMITS = receiptPolicy.receipt;
const headerDecoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });

function tlsCapabilityPresent(headers, defined) {
  const nullableRegions = new RegExp(`(?<!${TUN_WORD})NS_ASSUME_NONNULL_(?:BEGIN|END)${TUN_WORD_END}`, 'gu');
  const functionMarker = new RegExp(`(?<!${TUN_WORD})${TLS_FUNCTION}${TUN_WORD_END}${TUN_WS}*\\(`, 'u');
  const declarations = headers.flatMap(header => stripHeaderDirectives(stripHeaderComments(header))
    .replace(nullableRegions, '').split(/[;{}]/).filter(part => functionMarker.test(part)).map(trimTunSpace));
  requireSetting((declarations.length > 0) === defined, 'libbox TLS capability declaration/definition mismatch');
  if (defined) {
    const abi = new RegExp(`^(?:FOUNDATION_EXPORT${TUN_WS}+)?NSString${TUN_WS}*\\*${TUN_WS}*_Nonnull${TUN_WS}+${TLS_FUNCTION}`
      + `${TUN_WS}*\\(${TUN_WS}*NSError${TUN_WS}*\\*${TUN_WS}*_Nullable${TUN_WS}*\\*${TUN_WS}*_Nullable${TUN_WS}+${TUN_WORD}+${TUN_WS}*\\)$`, 'u');
    requireSetting(declarations.length === 1, 'libbox TLS capability declaration is not unique');
    requireSetting(abi.test(declarations[0]), 'libbox TLS capability C ABI differs from the optional Swift bridge');
  }
  return defined;
}

function matchesPin(expected, actual) {
  return isObject(expected) && Object.keys(expected).length === 2
    && expected.bytes === actual.bytes && expected.sha256 === actual.sha256;
}

function frameworkInput(boundary, path) {
  let physical;
  try { physical = realpathSync(path); } catch { throw new Error('libbox framework input missing or unresolvable'); }
  const subpath = relative(boundary, physical);
  requireSetting(!isAbsolute(subpath) && subpath !== '..' && !subpath.startsWith(`..${sep}`),
    'libbox input escaped selected framework');
  return physical;
}

function selectedTLSInputs(env) {
  const root = env.POLARIS_MOBILE_LINK_INPUT;
  const context = pinned(root, RECEIPT_LIMITS.appleLinkBytes, true);
  requireSetting(context.pin.sha256 === env.POLARIS_MOBILE_LINK_INPUT_SHA256,
    'actual mobile link context changed before TLS capability preparation');
  const link = decode(context.bytes, RECEIPT_LIMITS.appleLinkBytes, receiptLimit);
  const device = env.PLATFORM_NAME === SDKS.get('device-arm64');
  requireSetting((device || env.PLATFORM_NAME === SDKS.get('sim-arm64')) && env.ARCHS === 'arm64',
    'unrequested PacketTunnel TLS capability target');
  const identifier = device ? 'ios-arm64' : 'ios-arm64-simulator';
  const selected = link.componentOutput?.slices?.[identifier];
  requireSetting(selected?.libraryIdentifier === identifier, 'selected libbox slice differs from PacketTunnel target');
  const framework = resolve(link.framework, identifier, 'Libbox.framework');
  requireSetting(lstatSync(framework, { throwIfNoEntry: false })?.isDirectory(), 'libbox framework root redirected/missing');
  const boundary = realpathSync(framework);
  const binaryPath = frameworkInput(boundary, join(framework, 'Libbox'));
  const { pin: binary } = pinned(binaryPath, RECEIPT_LIMITS.artifactBytes);
  requireSetting(matchesPin(selected.binary, binary), 'actual libbox link bytes differ');
  const headerRoot = frameworkInput(boundary, join(framework, 'Headers'));
  const files = readdirSync(headerRoot).filter(name => name.endsWith('.h')).sort();
  requireSetting(files.length > 0 && files.length <= 64, 'libbox generated header inventory exceeds budget');
  let remaining = HEADER_BYTES;
  const headers = Object.create(null);
  for (const name of files) {
    const path = frameworkInput(boundary, join(headerRoot, name));
    requireSetting(statSync(path).size <= remaining, 'libbox generated header inventory exceeds budget');
    const { bytes, pin } = pinned(path, remaining);
    remaining -= bytes.length;
    headers[basename(path)] = { text: headerDecoder.decode(bytes), pin };
  }
  return { binaryPath, binary, headers };
}

// Native tool output is observed under the existing Python parent lease. Xcode
// consumes only the prepared bytes and never observes or uses its stale fd.
function tlsCompilerResponse(action, env) {
  if (action === 'select') return selectedTLSInputs(env);
  const derived = env.DERIVED_FILE_DIR;
  requireSetting(typeof derived === 'string' && derived.length > 0, 'TLS compiler response directory required');
  if (action === 'prepare') {
    // Only the admitted parent sends this pinned snapshot and its nm decision.
    const { input, defined } = JSON.parse(readBoundedInput(6 * HEADER_BYTES + TLS_RECEIPT_BYTES).toString('utf8'));
    requireSetting(typeof defined === 'boolean', 'TLS native symbol observation required');
    const headers = Object.entries(input.headers);
    const texts = headers.map(([, header]) => header.text);
    const capabilityPresent = tlsCapabilityPresent(texts, defined);
    mkdirSync(derived, { recursive: true });
    const flagsPath = join(derived, TLS_FLAGS), receiptPath = join(derived, TLS_RECEIPT);
    checkCompilerOutput(flagsPath);
    checkCompilerOutput(receiptPath);
    const nativeTun = nativeTunCompilerResponse('prepare', { headers: texts, prepared: derived });
    const flags = capabilityPresent ? TLS_FLAG_CONTENT : Buffer.alloc(0);
    const receipt = { capabilityPresent, binary: input.binary, headers: Object.fromEntries(headers.map(([name, row]) => [name, row.pin])),
      flags: pin(flags), devicePassed: false, nativeTun };
    writeCompilerArgs(flagsPath, flags);
    writeCompilerArgs(receiptPath, `${JSON.stringify(receipt)}\n`);
    return;
  }

  requireSetting(['install', 'verify-installed', 'verify-parent'].includes(action), 'closed TLS compiler response action required');
  const prepared = env.POLARIS_TLS_PREPARED_ROOT;
  const receipt = pinned(join(prepared, TLS_RECEIPT), TLS_RECEIPT_BYTES);
  requireSetting(receipt.pin.sha256 === env.POLARIS_TLS_PREPARED_SHA256, 'parent TLS capability receipt changed');
  const value = decode(receipt.bytes, TLS_RECEIPT_BYTES);
  requireSetting(isObject(value) && Object.keys(value).length === 6
    && ['binary', 'headers', 'flags', 'nativeTun'].every(key => Object.hasOwn(value, key))
    && typeof value.capabilityPresent === 'boolean' && value.devicePassed === false,
  'prepared TLS capability receipt structure differs');
  const flags = pinned(join(prepared, TLS_FLAGS), TLS_FLAGS_BYTES, false, { allowEmpty: true });
  requireSetting(matchesPin(value.flags, flags.pin), 'parent TLS capability flags changed');
  const { binary, headers } = selectedTLSInputs(env);
  const headerEntries = Object.entries(headers);
  requireSetting(matchesPin(value.binary, binary) && isObject(value.headers)
    && Object.keys(value.headers).length === headerEntries.length
    && headerEntries.every(([name, row]) => matchesPin(value.headers[name], row.pin)),
  'prepared TLS capability inputs differ from selected framework');
  requireSetting(flags.bytes.equals(value.capabilityPresent ? TLS_FLAG_CONTENT : Buffer.alloc(0)),
    'unexpected TLS capability linker flags');

  if (action !== 'verify-parent') mkdirSync(derived, { recursive: true });
  const derivedFlags = join(derived, TLS_FLAGS), derivedReceipt = join(derived, TLS_RECEIPT);
  if (action === 'install') {
    checkCompilerOutput(derivedFlags);
    checkCompilerOutput(derivedReceipt);
  }
  const nativeTun = nativeTunCompilerResponse(action, {
    headers: headerEntries.map(([, row]) => row.text), prepared, derived, receipt: value.nativeTun,
  });
  if (action === 'install') {
    writeCompilerArgs(derivedFlags, flags.bytes);
    writeCompilerArgs(derivedReceipt, receipt.bytes);
  } else if (action === 'verify-parent') {
    requireSetting(realpathSync(derived) === realpathSync(prepared), 'final TLS verifier must use parent prepared inputs');
    return { capabilityPresent: value.capabilityPresent, nativeTunHeaderABIAvailable: nativeTun.present };
  } else {
    requireSetting(pinned(derivedFlags, TLS_FLAGS_BYTES, false, { allowEmpty: true }).bytes.equals(flags.bytes)
      && pinned(derivedReceipt, TLS_RECEIPT_BYTES).bytes.equals(receipt.bytes), 'installed TLS linker inputs changed');
    return { scope: 'installed TLS linker input bytes only; final native verification belongs to parent',
      loadedQueryExecuted: false, devicePassed: false };
  }
}

const entry = process.argv[1];
if (entry && basename(entry) === 'apple-admission.mjs' && realpathSync(entry) === fileURLToPath(import.meta.url)) {
  const [, , mode, action, ...extra] = process.argv;
  try {
    if (mode === '--tls-link') {
      const result = tlsCompilerResponse(action, process.env);
      if (result !== undefined) console.log(JSON.stringify(result));
    } else {
      requireSetting(mode === '--build-settings' && isAbsolute(action ?? '') && extra.length === 0,
        'Apple build-settings requires one absolute output directory');
      await admitResolvedSettings(fileURLToPath(new URL('../../', import.meta.url)), action, process.env);
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
