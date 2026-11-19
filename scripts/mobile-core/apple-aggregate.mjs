// Verify the original same-run Apple summaries without running build tools.
import { spawn } from 'node:child_process';
import { readdirSync, realpathSync } from 'node:fs';
import { devNull } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { requireGraph as demand } from '../desktop-core/source-graph.mjs';
import { fingerprint, writeReceipt } from './apple-admission.mjs';
import { decode, pinned, readMobileSource, ReceiptFloat, receiptLimit, receiptPolicy as policy } from './source-admission.mjs';

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
