// Verify the original same-run Apple summaries without running build tools.
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, constants, fsyncSync, openSync, readdirSync, realpathSync, renameSync, rmSync, writeSync } from 'node:fs';
import { devNull } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { requireGraph as demand } from '../desktop-core/source-graph.mjs';
import { characters, decode, pinned, readMobileSource, ReceiptFloat, receiptLimit, receiptPolicy as policy } from './source-admission.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const limits = policy.receipt;
const readLimit = limits.appleComponentBytes;
const utf8 = new TextDecoder('utf-8', { fatal: true });
const slots = policy.appleScopes['device-and-simulator'];
const targets = {
  'device-arm64': { rust: 'aarch64-apple-ios', carrier: 'ios-arm64' },
  'sim-arm64': { rust: 'aarch64-apple-ios-sim', carrier: 'ios-arm64-simulator' },
};
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);

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
    demand(bytes <= maximum, 'receipt JSON byte budget');
    buffer += token;
    if (buffer.length >= 64 * 1024) { consume(buffer); buffer = ''; }
  }
  function encode(entry, depth, isKey = false) {
    demand(++nodes <= limits.nodes && depth <= limits.depth, 'receipt JSON node/depth budget');
    if (entry instanceof ReceiptFloat) push(floatText(entry.value));
    else if (typeof entry === 'string') {
      const length = characters(entry);
      stringChars += length;
      demand(length <= (isKey ? limits.keyChars : limits.stringChars) && stringChars <= maximum, 'receipt JSON string budget');
      demand(pretty || entry.isWellFormed(), 'invalid Unicode in receipt JSON');
      const quoted = JSON.stringify(entry);
      push(pretty ? quoted.replace(/[\u007f-\uffff]/g, char => '\\u' + char.charCodeAt(0).toString(16).padStart(4, '0')) : quoted);
    } else if (Array.isArray(entry) || object(entry)) {
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

function fingerprint(value) {
  const hash = createHash('sha256');
  serialize(value, receiptLimit(value), false, chunk => hash.update(chunk));
  return hash.digest('hex');
}

function checkFingerprint({ fingerprint: original, ...facts }) {
  return original === fingerprint(facts);
}

function same(left, right) {
  if (left === right) return true;
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left) && Array.isArray(right) && left.length === right.length &&
      left.every((entry, index) => same(entry, right[index]));
  }
  const numeric = value => value instanceof ReceiptFloat || ['boolean', 'number', 'bigint'].includes(typeof value);
  if (numeric(left) && numeric(right)) {
    const a = left instanceof ReceiptFloat ? left.value : left;
    const b = right instanceof ReceiptFloat ? right.value : right;
    return a == b; // Python numeric equality, including exact int/float comparisons.
  }
  if (!object(left) || !object(right) || left instanceof ReceiptFloat || right instanceof ReceiptFloat) return false;
  const names = Object.keys(left);
  return names.length === Object.keys(right).length && names.every(name => Object.hasOwn(right, name) && same(left[name], right[name]));
}

function readReceipt(path) {
  const { bytes, pin } = pinned(path, readLimit, true);
  return { value: decode(bytes, readLimit, receiptLimit), pin };
}

function writeReceipt(path, facts) {
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
      demand(written === size, 'receipt changed during serialization');
      fsyncSync(fd);
    } finally { closeSync(fd); }
    renameSync(temporary, path);
  } catch (error) {
    rmSync(temporary, { force: true });
    throw error;
  }
}

function cleanGit(args) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_')));
  Object.assign(env, { GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: devNull, GIT_NO_REPLACE_OBJECTS: '1', GIT_TERMINAL_PROMPT: '0' });
  return new Promise((complete, reject) => {
    const child = spawn('git', args, { cwd: root, env, detached: true, stdio: ['ignore', 'pipe', 'pipe'] });
    const output = [];
    let stdout = 0, stderr = 0, failure, termTimer, killTimer, settled = false;
    const stop = reason => {
      if (failure) return;
      failure = new Error(reason);
      signal('SIGTERM');
      termTimer = setTimeout(() => signal('SIGKILL'), 1000);
      killTimer = setTimeout(() => {
        child.stdout.destroy();
        child.stderr.destroy();
        finish();
      }, 2000);
    };
    function signal(name) {
      if (!child.pid) return;
      try { process.kill(-child.pid, name); }
      catch (error) { if (error.code !== 'ESRCH') failure ??= new Error('Git process group retirement failed'); }
    }
    const interrupt = () => stop('App source observation interrupted');
    const timer = setTimeout(() => stop('App source observation deadline exceeded'), policy.gitStatus.seconds * 1000);
    process.once('SIGTERM', interrupt);
    process.once('SIGINT', interrupt);
    function finish(code) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      clearTimeout(termTimer);
      clearTimeout(killTimer);
      process.removeListener('SIGTERM', interrupt);
      process.removeListener('SIGINT', interrupt);
      if (child.pid) {
        try {
          process.kill(-child.pid, 0);
          failure ??= new Error('Git left a live process group');
          signal('SIGKILL');
        } catch (error) { if (error.code !== 'ESRCH') failure ??= new Error('Git process group unavailable'); }
      }
      if (failure || code !== 0) reject(failure ?? new Error('App source observation failed'));
      else complete(Buffer.concat(output));
    }
    child.stdout.on('data', chunk => {
      if (failure) return;
      stdout += chunk.length;
      if (stdout > policy.gitStatus.stdoutBytes) stop('App source status stdout budget exceeded');
      else output.push(chunk);
    });
    child.stderr.on('data', chunk => {
      stderr += chunk.length;
      if (stderr > policy.gitStatus.stderrBytes) stop('App source status stderr budget exceeded');
    });
    child.stdout.once('error', () => stop('App source status stdout unavailable'));
    child.stderr.once('error', () => stop('App source status stderr unavailable'));
    child.once('error', () => stop('App source observation could not start'));
    child.once('close', finish);
  });
}

async function currentSource() {
  const actualRoot = utf8.decode(await cleanGit(['rev-parse', '--show-toplevel'])).trim();
  demand(realpathSync(actualRoot) === realpathSync(root), 'Git worktree differs from actual source root');
  const status = await cleanGit(['status', '--porcelain=v1', '-z', '--untracked-files=all']);
  demand(status.length === 0, 'App author and consumer source must be frozen');
  const candidate = utf8.decode(await cleanGit(['rev-parse', 'HEAD'])).trim();
  const env = process.env;
  demand(candidate && candidate === env.POLARIS_APP_CANDIDATE, 'handoff candidate differs');
  demand(env.GITHUB_ACTIONS === 'true', 'same-run hosted handoff required');
  demand(/^[0-9]+$/.test(env.GITHUB_RUN_ID ?? '') && /^[0-9]+$/.test(env.GITHUB_RUN_ATTEMPT ?? '') &&
    Object.hasOwn(env, 'GITHUB_REPOSITORY'), 'actual run/attempt/repository required');
  const pins = new Map();
  const roster = kind => Object.fromEntries([...policy.sourceFiles[kind], ...policy.sourceFiles.shared].map(name => {
    if (!pins.has(name)) pins.set(name, pinned(join(root, name), limits.artifactBytes).pin);
    return [name, pins.get(name)];
  }));
  const source = readMobileSource();
  return {
    context: { candidate, runId: env.GITHUB_RUN_ID, attempt: env.GITHUB_RUN_ATTEMPT, repository: env.GITHUB_REPOSITORY },
    producer: { candidate, files: roster('producer') },
    app: { candidate, consumerCode: roster('consumer') },
    source,
  };
}

async function aggregate(folder, output, scope) {
  demand(Object.hasOwn(policy.appleScopes, scope) && scope === (process.env.POLARIS_APPLE_SCOPE ?? 'device-only'),
    'CLI/environment Apple scope differs');
  const requiredSlots = policy.appleScopes[scope];
  const scopeFields = { carrierScope: scope, requiredSlots };
  const { context, producer, app, source } = await currentSource();
  const exactSlots = () => same(readdirSync(folder).sort(), ['component', ...requiredSlots].sort());
  demand(exactSlots(), 'aggregate requires exactly component and declared carrier slots');

  const { value: transport } = readReceipt(join(folder, 'component', 'component-transport.json'));
  const { value: manifest, pin: manifestPin } = readReceipt(join(folder, 'component', 'component-handoff.json'));
  demand(transport.schema === 'polaris-apple-handoff-transport-v1' && same(transport.context, context) &&
    same(manifestPin, transport.manifest) && checkFingerprint(transport), 'aggregate component run differs');
  const inScope = (value, kind) => value.schema === 'polaris-apple-' + kind + '-v2' &&
    value.carrierScope === scope && same(value.requiredSlots, requiredSlots);
  demand(inScope(manifest, 'component-handoff'), 'Apple handoff declared scope/requiredSlots differs');
  demand(same(manifest.App, producer) && same(manifest.sourceContract, source) && same(manifest.context, context) &&
    manifest.producerJob === 'component' && checkFingerprint(manifest), 'aggregate original component/source differs');

  const carriers = {};
  let compiler;
  for (const slot of requiredSlots) {
    const dir = join(folder, slot);
    const { value: row } = readReceipt(join(dir, 'carrier-' + slot + '.json'));
    demand(inScope(row, 'carrier-handoff'), 'Apple handoff declared scope/requiredSlots differs');
    demand(row.slot === slot && same(row.context, context) && same(row.componentTransport, transport) &&
      row.devicePassed === false && row.signatureTrustValidated === false && row.publicationEligible === false &&
      checkFingerprint(row), 'aggregate slot/context/component/scope differs');
    const { value: receipt, pin: receiptPin } = readReceipt(join(dir, 'summary-app-carrier.json'));
    demand(same(receiptPin, row.receiptPin) && same(receipt, row.receipt), 'aggregate original carrier receipt bytes differ');
    const { value: rust, pin: rustPin } = readReceipt(join(dir, 'summary-rust-receipt.json'));
    const { bytes, sha256 } = receipt.rustInput;
    demand(same(rustPin, { bytes, sha256 }), 'aggregate original Rust receipt bytes differ');
    demand(rust.schema === 'polaris-formal-apple-rust-v1' && same(rust.App, app) && checkFingerprint(rust) &&
      rust.target === targets[slot].rust && Object.hasOwn(rust, 'linkInputSha256') && Object.hasOwn(receipt, 'linkInputSha256') &&
      same(rust.linkInputSha256, receipt.linkInputSha256) && rust.devicePassed === false,
      'aggregate Rust target/App/link identity differs');
    const rustCompiler = { tools: rust.tools, rustc: rust.rustc };
    demand(Object.hasOwn(rust, 'tools') && Object.hasOwn(rust, 'rustc'), 'aggregate actual Rust tool bytes/version differ');
    compiler ??= rustCompiler;
    demand(same(compiler, rustCompiler), 'aggregate actual Rust tool bytes/version differ');
    demand(receipt.schema === 'polaris-mobile-apple-carrier-v1' && checkFingerprint(receipt) && same(receipt.App, app) &&
      receipt.producerCandidate === producer.candidate && Object.hasOwn(manifest, 'componentFingerprint') &&
      Object.hasOwn(receipt, 'componentFingerprint') && same(receipt.componentFingerprint, manifest.componentFingerprint) &&
      receipt.selectedTarget === targets[slot].carrier &&
      receipt.AppCarrierValidated === true && receipt.AppAccepted === false &&
      receipt.devicePassed === false && receipt.signatureTrustValidated === false, 'aggregate actual carrier identity/scope differs');
    carriers[slot] = row;
  }

  demand(exactSlots(), 'aggregate contains extra slot');
  const facts = {
    schema: 'polaris-apple-unsigned-chain-v3', context, App: producer, componentTransport: transport, carriers,
    completeSlots: requiredSlots, devicePassed: false, signatureTrustValidated: false, publicationEligible: false,
    ...scopeFields, validatedSlots: requiredSlots, notRequestedSlots: slots.filter(slot => !requiredSlots.includes(slot)),
  };
  facts.fingerprint = fingerprint(facts);
  writeReceipt(output, facts);
}

try {
  const { values } = parseArgs({ options: { scope: { type: 'string' }, summaries: { type: 'string' }, output: { type: 'string' } } });
  demand(values.scope && values.summaries && values.output, 'aggregate scope, summaries and output required');
  await aggregate(resolve(values.summaries), resolve(values.output), values.scope);
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
