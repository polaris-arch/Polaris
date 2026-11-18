import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

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

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [, , slot, query, minimum, scheme] = process.argv;
  const result = inspectAppleArchitecture(readFileSync(0), { slot, query, minimum, scheme: JSON.parse(scheme) });
  process.stdout.write(JSON.stringify(result));
}
