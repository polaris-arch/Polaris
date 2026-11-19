import { createHash } from 'node:crypto';
import { chmodSync, constants, copyFileSync, existsSync, lstatSync, mkdirSync, readdirSync,
  realpathSync, renameSync, statSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';

import { APPLE, ROOT, canonical, demand, equal, filePin, inspectAar, inspectFramework, inspectionStage,
  openComponentZip, parsePlist, readPinned, readReceipt, receiptDigest, receiptPolicy, verifyNativeInputs,
  within, AppleBuildInfoBatch } from './component-admission.mjs';
import { decode, pinned, readMobileSource } from './source-admission.mjs';

const sha = raw => createHash('sha256').update(raw).digest('hex');
const status = receiptPolicy.gitStatus;
const MAX_STATUS_PATHS = 64, MAX_STATUS_PATH_BYTES = 512;
const MAX_STATUS_HASH_BYTES = 1024 ** 2, MAX_STATUS_REPORT_BYTES = 65536;
const utf8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
const displayPaths = new Set([
  'Cargo.toml', 'Cargo.lock', '.gitignore', 'src-tauri/tauri.ios.conf.json',
  'src-tauri/gen/apple/project.yml', 'src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj',
  'crates/tauri-plugin-polaris-ios/ios/Package.swift',
  'crates/tauri-plugin-polaris-ios/ios/Package.resolved',
]);

function validFingerprint({ fingerprint, ...proof }) {
  return fingerprint === receiptDigest(proof);
}

export async function verifyOriginal({
  component, receiptPath, platform, tools = {}, scratch, run, inspectionProgress,
}) {
  const source = readMobileSource(), receipt = readReceipt(receiptPath);
  demand(receipt.schema === 'polaris-mobile-component-v1' && ['android', 'apple'].includes(platform) &&
    receipt.platform === platform && receipt.sourceOverlay === 0 && receipt.AppAccepted === false &&
    receipt.devicePassed === false, 'old/CLI/diagnostic mobile component receipt rejected');
  demand(validFingerprint(receipt), 'component receipt changed');
  await verifyNativeInputs({ facts: receipt.nativeInputs, log: join(dirname(component), 'original-bind.log'),
    output: receipt.output });
  const binding = receipt.input;
  demand(equal(binding.sourceContract, source) && binding.sourceObservation.sourceCommit === source.sourceCommit &&
    binding.sourceObservation.sourceTree === source.sourceTree && binding.sourceOverlay === 0,
  'formal component source input differs');
  const expectedId = 'polaris-mobile-' + receiptDigest(binding);
  const graph = binding.moduleGraph;
  let observed;
  if (platform === 'android') {
    for (const name of ['go', 'javap', 'readelf']) demand(tools[name], 'actual verifier tool missing: ' + name);
    observed = await inspectAar({ aar: component, source, graph, tools, run, scratch, expectedId,
      originalOutput: receipt.output });
  } else {
    observed = await inspectFramework({ framework: component, source, graph, expectedId, run, scratch, inspectionProgress });
  }
  if (!equal(observed, receipt.output)) {
    const changed = [...new Set([...Object.keys(observed), ...Object.keys(receipt.output)])]
      .filter(key => !equal(observed[key], receipt.output[key])).sort();
    throw new Error('actual component bytes/API/native identity differ; fields=' + JSON.stringify(changed) +
      (platform === 'apple' ? '; originalRawHeaders={}' : ''));
  }
  return receipt;
}

export async function sourceStatusObservation({ root = ROOT, runBounded, env }) {
  const started = performance.now();
  const deadline = started + 1000 * status.seconds;
  const chunks = [];
  const append = chunk => { chunks.push(Buffer.from(chunk)); };
  const result = await runBounded(['git', 'status', '--porcelain=v1', '-z', '--untracked-files=all'], {
    cwd: root, env, timeoutSeconds: status.seconds, cleanupSeconds: 2,
    stdoutLimit: status.stdoutBytes, stderrLimit: status.stderrBytes,
    onStdout: append, onStdoutOverflow: append,
  });
  const raw = Buffer.concat(chunks).subarray(0, status.stdoutBytes + 1);
  const statusReason = {
    'exit-or-live-group': 'git-exit-or-live-group',
    'collection-error': 'collector-error',
    'cleanup-incomplete': 'cleanup-error',
  }[result.reason] ?? result.reason ?? (result.complete ? null : 'collector-error');
  let reason = statusReason, cursor = 0, hashBytes = 0;
  const paths = [];
  const reject = code => { reason ??= code; };
  const ended = () => performance.now() >= deadline;
  const next = () => {
    const end = raw.indexOf(0, cursor);
    if (end < 0) return null;
    const entry = raw.subarray(cursor, end);
    cursor = end + 1;
    return entry;
  };
  while (cursor < raw.length && paths.length < MAX_STATUS_PATHS) {
    if (ended()) { reject('deadline'); break; }
    const entry = next();
    if (!entry) { reject('partial-status'); break; }
    const statusText = entry.subarray(0, 2).toString('latin1');
    if (entry.length < 4 || entry[2] !== 32 || !/^[ MADRCU?!T]{2}$/.test(statusText)) {
      reject('invalid-status');
      break;
    }
    const names = [['current', entry.subarray(3)]];
    if (/[RC]/.test(statusText)) {
      const former = next();
      if (!former) { reject('missing-rename-origin'); break; }
      names.push(['rename-origin', former]);
    }
    for (const [role, encoded] of names) {
      if (paths.length >= MAX_STATUS_PATHS || encoded.length > MAX_STATUS_PATH_BYTES) {
        reject('path-budget');
        break;
      }
      const row = { status: statusText, role, pathBytes: encoded.length,
        pathSha256: sha(encoded), path: null, type: null, sha256: null };
      paths.push(row);
      try {
        const name = utf8.decode(encoded);
        if (!name || isAbsolute(name) || name.split(sep).includes('..')) { reject('invalid-path'); continue; }
        row.path = displayPaths.has(name) ? name : null;
        const path = join(root, name), info = lstatSync(path);
        Object.assign(row, { bytes: info.size, mode: info.mode & 0o7777,
          type: info.isSymbolicLink() ? 'symlink' : info.isFile() ? 'file' : info.isDirectory() ? 'directory' : 'other' });
        if (row.type !== 'file') continue;
        if (ended() || info.size > MAX_STATUS_HASH_BYTES - hashBytes) { reject('hash-budget-or-deadline'); continue; }
        if (!within(root, realpathSync(path))) { reject('redirected-path'); continue; }
        const { pin } = pinned(path, MAX_STATUS_HASH_BYTES - hashBytes, false, { allowEmpty: true });
        hashBytes += pin.bytes;
        if (ended()) { reject('deadline'); continue; }
        if (info.size !== pin.bytes) { reject('incomplete-or-changed-file'); continue; }
        row.sha256 = pin.sha256;
      } catch (error) {
        if (error.code === 'ENOENT') {
          row.type = 'absent';
          if (!statusText.includes('D')) reject('metadata-missing');
        } else {
          reject('metadata-error');
        }
      }
    }
  }
  if (cursor < raw.length) reject('path-budget');
  return {
    schema: 'polaris-app-source-status-observation-v1', complete: reason === null, reason,
    statusComplete: result.complete, clean: !raw.length && result.complete, paths,
    observedStdoutBytes: raw.length, observedStderrBytes: result.stderrBytesObserved,
    observedStatusPrefixSha256: sha(raw), hashBytesRead: hashBytes,
    rawStatusRetained: false, fileContentsExported: false, processRSS: null,
    scope: 'derived bounded metadata; exact reviewed names only; no raw stdout/stderr/diff/file bodies',
    limits: { stdoutBytes: status.stdoutBytes, stderrBytes: status.stderrBytes,
      paths: MAX_STATUS_PATHS, pathBytes: MAX_STATUS_PATH_BYTES,
      hashBytes: MAX_STATUS_HASH_BYTES, reportBytes: MAX_STATUS_REPORT_BYTES, seconds: status.seconds,
      pipeOverflowSentinelBytes: 1, hashOverflowSentinelBytes: 1, terminationGraceSeconds: 2 },
  };
}

async function nativeRunner(runner) {
  if (runner) return runner;
  const { createNativeRunner } = await import('./native-runner.mjs');
  return createNativeRunner();
}

export async function appIdentity({ statusLog, runner, run, runBounded } = {}) {
  const { gitEnvironment, requireGitRoot } = await import('./fork-input.mjs');
  if (!runBounded) ({ runBounded } = await nativeRunner(runner));
  if (!run) ({ run } = await nativeRunner(runner));
  const env = gitEnvironment();
  let observation;
  try {
    requireGitRoot(ROOT);
    observation = await sourceStatusObservation({ runBounded, env });
  } catch {
    observation = { schema: 'polaris-app-source-status-observation-v1', complete: false,
      clean: false, statusComplete: false, paths: [], reason: 'collector-error', rawStatusRetained: false,
      fileContentsExported: false, processRSS: null,
      scope: 'incomplete derived metadata; no raw error text or original source export' };
  }
  if (statusLog !== undefined) {
    let raw = canonical(observation, { ascii: true });
    if (raw.length > MAX_STATUS_REPORT_BYTES) {
      Object.assign(observation, { complete: false, reason: 'report-budget', omittedPathCount: observation.paths.length, paths: [] });
      raw = canonical(observation, { ascii: true });
    }
    demand(raw.length <= MAX_STATUS_REPORT_BYTES, 'source status observation exceeds fixed report budget');
    try { writeFileSync(statusLog, raw, { flag: 'wx', mode: 0o600 }); }
    catch { throw new Error('incomplete source status observation could not be written'); }
  }
  demand(observation.complete && observation.clean, 'App consumer source is not frozen; observationComplete=' +
    observation.complete + '; observedPaths=' + observation.paths.length + '; reason=' + observation.reason);
  const { consumer, shared } = receiptPolicy.sourceFiles;
  return { candidate: (await run(['git', 'rev-parse', 'HEAD'], { cwd: ROOT, env })).trim(),
    consumerCode: Object.fromEntries([...consumer, ...shared].map(name => [name, filePin(join(ROOT, name))])) };
}

function protectedParents(path) {
  demand(within(ROOT, path), 'App input escaped checkout');
  for (let current = path; ; current = dirname(current)) {
    demand(!lstatSync(current, { throwIfNoEntry: false })?.isSymbolicLink(), 'App input parent redirected');
    if (current === ROOT) break;
  }
}

const androidReceipt = join(ROOT, 'scripts/mobile-core/android-consumption.json');

export async function materializeAndroid({ component, receiptPath, tools, work, cache, runner, run, lease }) {
  runner = await nativeRunner(runner);
  run ??= runner.run;
  lease ??= runner.withLease;
  return lease(work, cache, readMobileSource(), async (ownedWork, ownedCache) => {
    demand(realpathSync(readReceipt(receiptPath).nativeInputs.cache) === ownedCache,
      'consumer must hold the actual producer cache lease');
    const receipt = await verifyOriginal({ component, receiptPath, platform: 'android', tools, scratch: ownedWork, run });
    const identity = await appIdentity({ runner, run });
    const dest = join(ROOT, 'src-tauri/gen/android/app/libs/libbox.aar');
    protectedParents(dirname(dest));
    mkdirSync(dirname(dest), { recursive: true });
    const stage = join(ownedWork, 'app-libbox.aar');
    const stagedReceipt = join(ownedWork, 'stage-app-consumption.json');
    const items = [[stage, dest], [stagedReceipt, androidReceipt]];
    const { recover, install } = await import('./transaction.mjs');
    const { writeJson } = await import('./producer.mjs');
    await recover(items, ownedWork);
    if (existsSync(dest) && existsSync(androidReceipt)) {
      const existing = readReceipt(androidReceipt);
      if (existing.componentFingerprint === receipt.fingerprint && equal(existing.App, identity) &&
        validFingerprint(existing) && equal(existing.output, receipt.output) && existing.AppAccepted === false &&
        existing.devicePassed === false && existing.sourceOverlay === 0 && equal(filePin(dest), receipt.output.artifact)) return existing;
    }
    demand(!existsSync(stage) && !lstatSync(stage, { throwIfNoEntry: false }), 'unaccepted materialization stage retained');
    copyFileSync(component, stage, constants.COPYFILE_EXCL);
    demand(equal(filePin(stage), receipt.output.artifact), 'materialization copy changed');
    const facts = { schema: 'polaris-mobile-app-input-v1', platform: 'android', App: identity,
      producerCandidate: receipt.input.producer.candidate, componentReceiptSha256: filePin(receiptPath).sha256,
      componentFingerprint: receipt.fingerprint, output: receipt.output, AppAccepted: false, devicePassed: false, sourceOverlay: 0 };
    facts.fingerprint = receiptDigest(facts);
    await writeJson(stagedReceipt, facts);
    await install(items, ownedWork, async () => {
      demand(equal(filePin(dest), receipt.output.artifact) && equal(await appIdentity({ runner, run }), identity),
        'installed input or consumer source changed');
      demand(equal(readReceipt(androidReceipt), facts), 'installed consumption receipt changed');
    });
    return facts;
  });
}

export function appleResourceInputs() {
  const config = decode(readPinned(join(ROOT, 'src-tauri/tauri.ios.conf.json')).bytes, 1024 ** 2);
  demand(equal(config.bundle.resources, ['../resources/data/', '../THIRD-PARTY-LICENSES.md', '../NOTICE', '../LICENSE']),
    'iOS resource contract changed');
  const data = join(ROOT, 'resources/data');
  demand(lstatSync(data).isDirectory(), 'real geo resource directory required');
  const geo = readdirSync(data).sort();
  demand(geo.length === 28 && geo.every(name => name.endsWith('.srs')), 'complete 28 geo resource inventory required');
  const sources = [...geo.map(name => 'resources/data/' + name), 'THIRD-PARTY-LICENSES.md', 'NOTICE', 'LICENSE'];
  return Object.fromEntries(sources.map(source => {
    const path = join(ROOT, source);
    protectedParents(path);
    const { bytes, pin } = readPinned(path);
    if (source.endsWith('.srs')) demand(bytes.subarray(0, 3).equals(Buffer.from('SRS')), 'resource geo bytes lack SRS magic');
    const mode = lstatSync(path).mode & 0o7777;
    demand(mode === 0o644, 'resource source mode differs');
    return ['_up_/' + source, { source, ...pin, mode }];
  }));
}

const appleAssets = join(ROOT, 'src-tauri/gen/apple/assets');
const pinFields = ({ bytes, sha256 }) => ({ bytes, sha256 });

export function appleResources() {
  const rows = appleResourceInputs();
  demand(lstatSync(appleAssets).isDirectory(), 'materialized Apple resources required before input freeze');
  const actual = new Set();
  function verify(current) {
    for (const item of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, item.name), name = relative(appleAssets, path).split(sep).join('/');
      if (item.isFile()) {
        const row = rows[name];
        demand(row && equal(filePin(path), pinFields(row)) && (lstatSync(path).mode & 0o7777) === row.mode,
          'materialized Apple resource differs');
        actual.add(name);
      } else {
        demand(item.isDirectory() && Object.keys(rows).some(path => path.startsWith(name + '/')),
          'unbound Apple resource directory or redirected output');
        verify(path);
      }
    }
  }
  verify(appleAssets);
  demand(actual.size === Object.keys(rows).length, 'materialized Apple resource inventory differs');
  return rows;
}

export function materializeAppleResources() {
  const rows = appleResourceInputs();
  protectedParents(appleAssets);
  if (existsSync(appleAssets)) {
    demand(equal(appleResources(), rows), 'retained Apple resource output differs');
    return rows;
  }
  const stage = appleAssets + '.formal-stage';
  mkdirSync(stage, { mode: 0o700 });
  for (const [name, row] of Object.entries(rows)) {
    const dest = join(stage, name);
    mkdirSync(dirname(dest), { recursive: true });
    copyFileSync(join(ROOT, row.source), dest, constants.COPYFILE_EXCL);
    chmodSync(dest, row.mode);
    demand(equal(filePin(dest), pinFields(row)), 'resource copy differs');
  }
  demand(equal(appleResourceInputs(), rows), 'resource source changed during materialization');
  chmodSync(stage, 0o755);
  demand(!lstatSync(appleAssets, { throwIfNoEntry: false }), 'Apple resource output appeared during materialization');
  renameSync(stage, appleAssets);
  demand(equal(appleResources(), rows), 'installed Apple resource output differs');
  return rows;
}

export async function appleLink({ component, receiptPath, output, work, cache, runner, run, lease, inspectionProgress }) {
  runner = await nativeRunner(runner);
  run ??= runner.run;
  lease ??= runner.withLease;
  return lease(work, cache, readMobileSource(), async (ownedWork, ownedCache) => {
    demand(realpathSync(readReceipt(receiptPath).nativeInputs.cache) === ownedCache,
      'consumer must hold the actual producer cache lease');
    const receipt = await verifyOriginal({ component, receiptPath, platform: 'apple', scratch: ownedWork, run, inspectionProgress });
    const facts = { schema: 'polaris-mobile-apple-link-input-v1', App: await appIdentity({ runner, run }),
      framework: realpathSync(component), componentReceipt: realpathSync(receiptPath),
      componentReceiptSha256: filePin(receiptPath).sha256, componentFingerprint: receipt.fingerprint,
      producerInput: receipt.input, componentOutput: receipt.output, resources: appleResources(),
      staticOnly: true, AppAccepted: false, devicePassed: false };
    facts.fingerprint = receiptDigest(facts);
    demand(!lstatSync(output, { throwIfNoEntry: false }), 'fixed link input already exists; do not overwrite a frozen context');
    const { writeJson } = await import('./producer.mjs');
    await writeJson(output, facts);
    return facts;
  });
}

function frozenLink(path, expectedSha) {
  demand(filePin(path).sha256 === expectedSha, 'fixed Apple link input changed');
  const link = readReceipt(path);
  demand(link.schema === 'polaris-mobile-apple-link-input-v1' && link.staticOnly === true && validFingerprint(link),
    'Apple link context differs');
  return link;
}

export async function verifyLink({ path, expectedSha, scratch, runner, run, inspectionProgress }) {
  runner = await nativeRunner(runner);
  run ??= runner.run;
  const link = frozenLink(path, expectedSha);
  demand(filePin(link.componentReceipt).sha256 === link.componentReceiptSha256, 'original component receipt changed');
  const receipt = await verifyOriginal({ component: link.framework, receiptPath: link.componentReceipt, platform: 'apple',
    scratch, run, inspectionProgress });
  demand(receipt.fingerprint === link.componentFingerprint && equal(receipt.output, link.componentOutput) &&
    equal(await appIdentity({ runner, run }), link.App) && equal(appleResources(), link.resources),
  'actual component/App/resource link context changed');
  return link;
}

export async function inspectAndroidCarrier({ apk, abi, r8, component, receiptPath, tools, scratch, runner, run }) {
  runner = await nativeRunner(runner);
  run ??= runner.run;
  const receipt = await verifyOriginal({ component, receiptPath, platform: 'android', tools, scratch, run });
  const material = readReceipt(androidReceipt), componentReceiptSha256 = filePin(receiptPath).sha256;
  demand(material.schema === 'polaris-mobile-app-input-v1' && material.platform === 'android' &&
    material.componentFingerprint === receipt.fingerprint && material.componentReceiptSha256 === componentReceiptSha256 &&
    equal(material.output, receipt.output) && material.sourceOverlay === 0 && material.AppAccepted === false &&
    material.devicePassed === false && validFingerprint(material), 'actual materialized App input changed');
  const current = await appIdentity({ runner, run });
  demand(equal(current, material.App), 'App candidate changed after SDK materialization');
  const pins = Object.fromEntries(Object.entries(receipt.output.nativeLibraries).map(([name, row]) => [name, pinFields(row)]));
  const verify = join(ROOT, 'scripts/verify-apk.mjs');
  const payload = JSON.parse(await run([process.execPath, verify, '--sdk-payload', apk, abi, JSON.stringify(pins)], { cwd: ROOT }));
  await run([process.execPath, join(ROOT, 'scripts/assert-r8-evidence.mjs'), r8], { cwd: ROOT });
  for (const target of payload.selectedABIs) await run([process.execPath, verify, apk, '--abi', target], { cwd: ROOT });
  demand(equal(await appIdentity({ runner, run }), current) && equal(readReceipt(androidReceipt), material),
    'consumer input changed during final carrier inspection');
  const r8Files = {};
  function readR8(current) {
    for (const item of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, item.name);
      if (item.isDirectory()) readR8(path);
      else if (statSync(path).isFile()) r8Files[relative(r8, path).split(sep).join('/')] = filePin(path);
    }
  }
  readR8(r8);
  return { schema: 'polaris-mobile-android-carrier-v1', App: current, producerCandidate: receipt.input.producer.candidate,
    componentFingerprint: receipt.fingerprint, componentReceiptSha256, apk: filePin(apk), ...payload, r8Files,
    AppCarrierValidated: true, AppAccepted: false, devicePassed: false, signatureTrustValidated: false };
}

export async function staticLinkmap(text, framework, selected, derived, inspectionProgress) {
  const reads = {};
  return inspectionStage('static-linkmap', reads, inspectionProgress, () => {
    demand(['# Object files:', '# Sections:', '# Symbols:'].every(marker => text.includes(marker)),
      'actual extension linker map required');
    const original = join(framework, selected.libraryIdentifier, 'Libbox.framework/Libbox');
    const binary = realpathSync(original);
    derived = realpathSync(derived);
    demand(statSync(derived).isDirectory(), 'actual fixed derived-data directory required');
    const members = new Map();
    for (const member of selected.members) {
      if (!members.has(member.name)) members.set(member.name, []);
      members.get(member.name).push(member);
    }
    const consumed = [], pins = new Map(), resolutions = new Map();
    function originalPin(path, resolved) {
      resolutions.set(path, resolved);
      if (!pins.has(resolved)) pins.set(resolved, filePin(resolved, { observations: reads }));
      return pins.get(resolved);
    }
    const body = text.split('# Object files:', 2)[1].split('# Sections:', 1)[0];
    for (const line of body.split(/\r?\n/)) {
      const match = /^\[\s*(\d+)\]\s+(.+)\(([^()]+)\)$/.exec(line.trim());
      if (!match || !match[2].includes('Libbox.framework')) continue;
      const [, ordinal, archive, name] = match;
      const linked = realpathSync(archive);
      demand((linked === binary || within(derived, linked)) && members.has(name),
        'extension link map used a different/unbound static SDK input');
      const pin = originalPin(archive, linked);
      demand(equal(pin, selected.binary), 'extension link map used a different/unbound static SDK input');
      for (const [header, digest] of Object.entries(selected.headerSha256)) {
        const path = join(dirname(archive), 'Headers', header), actual = realpathSync(path);
        demand(within(dirname(linked), actual), 'actual selected framework header escaped');
        demand(originalPin(path, actual).sha256 === digest, 'actual selected framework headers changed');
      }
      const linkOrdinal = Number(ordinal);
      demand(Number.isSafeInteger(linkOrdinal), 'link map object ordinal is unsupported');
      consumed.push({ linkOrdinal, archive: linked, archivePin: pin, memberName: name, originalMembers: members.get(name) });
    }
    demand(consumed.some(row => row.originalMembers.some(member => member.goCarrier)), 'link map lacks original Go carrier member');
    demand(consumed.some(row => row.originalMembers.some(member => !member.goCarrier)), 'link map lacks original generated/native members');
    demand(new Set(consumed.map(row => row.linkOrdinal)).size === consumed.length, 'link map object ordinals duplicate');
    demand(realpathSync(original) === binary, 'selected framework resolution changed');
    for (const [path, resolved] of resolutions) demand(realpathSync(path) === resolved, 'linked framework resolution changed');
    for (const [resolved, pin] of pins) {
      demand(equal(filePin(resolved, { observations: reads }), pin), 'linked framework bytes changed during member inspection');
    }
    return consumed;
  });
}

function verifyAppleResources(archive, appRoot, expected) {
  const prefix = appRoot + 'assets/';
  const actual = [...archive.entries.keys()]
    .filter(name => name.startsWith(prefix) && !name.endsWith('/')).map(name => name.slice(prefix.length));
  demand(actual.length === Object.keys(expected).length && actual.every(name => Object.hasOwn(expected, name)),
    'final App resource inventory differs');
  return Object.fromEntries(Object.entries(expected).map(([name, row]) => {
    const path = prefix + name, bytes = archive.member(path);
    const pin = { bytes: bytes.length, sha256: sha(bytes), mode: archive.entries.get(path).mode & 0o7777 };
    demand(equal(pin, { ...pinFields(row), mode: row.mode }), 'final App resource bytes/mode differ');
    return [name, pin];
  }));
}

export function appleVersionRaw(version) {
  demand(typeof version === 'string' && /^\d+(?:\.\d+){1,2}$/.test(version), 'explicit Apple deployment/SDK version required');
  const [major, minor, patch = 0] = version.split('.').map(Number);
  demand(major > 0 && major <= 0xffff && minor <= 0xff && patch <= 0xff, 'Apple deployment/SDK version out of range');
  return major * 0x10000 + minor * 0x100 + patch;
}

export async function inspectAppleCarrier({
  ipa, linkPath, linkSha, linkmap, receiptPath, scratch, derived, simulatorArch, runner, run, inspectionProgress,
}) {
  demand(simulatorArch === undefined || simulatorArch === null || simulatorArch === 'arm64',
    'formal Apple supports only device-arm64 and sim-arm64');
  runner = await nativeRunner(runner);
  run ??= runner.run;
  const link = frozenLink(linkPath, linkSha);
  demand(filePin(receiptPath).sha256 === link.componentReceiptSha256, 'original Apple component receipt changed');
  const receipt = await inspectionStage('verify-original', null, inspectionProgress, () => verifyOriginal({
    component: link.framework, receiptPath, platform: 'apple', scratch, run, inspectionProgress,
  }));
  demand(receipt.fingerprint === link.componentFingerprint && equal(receipt.output, link.componentOutput),
    'actual link component differs');
  const current = await appIdentity({ runner, run });
  demand(equal(current, link.App) && equal(appleResources(), link.resources),
    'App source/resources changed after link context was frozen');
  const source = readMobileSource();
  const selectedTarget = simulatorArch ? 'ios-arm64-simulator' : 'ios-arm64';
  const [cpu, platform] = APPLE[selectedTarget];
  const minimum = appleVersionRaw(source.apple.appMinimumOS);
  const sdk = appleVersionRaw(source.apple[simulatorArch ? 'simulatorSDK' : 'iPhoneSDK']);
  const { container } = await import('../apple-carrier.mjs');
  const batch = new AppleBuildInfoBatch();
  const { binaries, resources, artifactPin } = await inspectionStage('ipa-parse', null, inspectionProgress, async () => {
    const artifactPin = filePin(ipa);
    const archive = openComponentZip(ipa);
    try {
      const names = [...archive.entries.keys()];
      const apps = names.filter(name => /^Payload\/[^/]+\.app\/Info\.plist$/.test(name));
      demand(apps.length === 1, 'one actual host App required');
      const appRoot = apps[0].slice(0, -'Info.plist'.length);
      const extensions = names.filter(name => name.startsWith(appRoot + 'PlugIns/') && name.endsWith('.appex/Info.plist'));
      demand(extensions.length === 1, 'one actual PacketTunnel required');
      demand(!names.some(name => name.startsWith(appRoot) && (name.endsWith('.a') || name.includes('Libbox.framework/'))),
        'build-only static SDK must not be copied into the installed App');
      const extensionRoot = extensions[0].slice(0, -'Info.plist'.length);
      const resources = verifyAppleResources(archive, appRoot, link.resources);
      const app = await parsePlist(archive.member(apps[0]), { run, scratch });
      const extension = await parsePlist(archive.member(extensions[0]), { run, scratch });
      demand(extension.CFBundleIdentifier === app.CFBundleIdentifier + '.PacketTunnel' &&
        extension.NSExtension?.NSExtensionPointIdentifier === 'com.apple.networkextension.packet-tunnel' &&
        app.PolarisAppGroup === extension.PolarisAppGroup && typeof app.PolarisAppGroup === 'string' &&
        app.PolarisAppGroup.startsWith('group.') && !app.PolarisAppGroup.includes('$('),
      'actual App/extension identities/group differ');
      const binaries = {};
      for (const [name, path, info] of [['app', appRoot, app], ['extension', extensionRoot, extension]]) {
        demand(equal(info.UIDeviceFamily, [1, 2]) &&
          equal(info.CFBundleSupportedPlatforms, [simulatorArch ? 'iPhoneSimulator' : 'iPhoneOS']) &&
          appleVersionRaw(info.MinimumOSVersion) === minimum, 'actual device carrier scope differs');
        const raw = archive.member(path + info.CFBundleExecutable);
        const parsed = container(raw, batch.observe);
        demand(parsed.kind === 'Mach-O64' && parsed.cpu === cpu && parsed.filetype === 2,
          'actual App binary target must be the selected thin MH_EXECUTE');
        const [command] = parsed.platformCommands;
        demand(parsed.platformCommands.length === 1 && command.platform === platform && command.minOSRaw === minimum &&
          command.sdkRaw === sdk, 'actual ' + name + ' native platform/deployment/SDK differs');
        const row = { bytes: raw.length, sha256: sha(raw), bundleIdentifier: info.CFBundleIdentifier };
        if (name === 'extension') {
          const infos = parsed.sections.flatMap(section => section.buildInfo ? [section.buildInfo] : []);
          const ids = parsed.sections.flatMap(section => (section.buildIDs ?? []).map(note => note.value));
          demand(infos.length === 1 && ids.length === 1, 'final extension must retain original Go source carrier');
          batch.bind(infos[0], { arch: simulatorArch ?? 'arm64', simulator: Boolean(simulatorArch),
            expectedId: 'polaris-mobile-' + receiptDigest(receipt.input), actualId: ids[0] });
          Object.assign(row, { goBuildInfo: infos[0], buildID: ids[0] });
        }
        binaries[name] = row;
      }
      await batch.verify({ source, graph: receipt.input.moduleGraph });
      return { binaries, resources, artifactPin };
    } finally {
      archive.close();
    }
  });
  const original = readPinned(linkmap);
  const consumed = await staticLinkmap(utf8.decode(original.bytes), link.framework,
    receipt.output.slices[selectedTarget], derived, inspectionProgress);
  demand(equal(await appIdentity({ runner, run }), current) && filePin(linkPath).sha256 === linkSha &&
    equal(filePin(ipa), artifactPin) && equal(filePin(linkmap), original.pin), 'final App/link source context changed');
  return { schema: 'polaris-mobile-apple-carrier-v1', App: current, ipa: artifactPin, binaries,
    producerCandidate: receipt.input.producer.candidate, componentFingerprint: receipt.fingerprint, linkInputSha256: linkSha,
    selectedTarget, resources, linkedOriginalStaticMembers: consumed, linkmap: original.pin, AppCarrierValidated: true,
    AppAccepted: false, devicePassed: false, signatureTrustValidated: false,
    scope: 'App/PacketTunnel compile/static source and original bytes; signing permissions/device lifecycle are separate' };
}

async function main() {
  const opts = Object.fromEntries([
    'component', 'receipt', 'work', 'cache', 'link-input', 'scratch', 'link-input-sha256',
    'go', 'javap', 'readelf', 'output', 'artifact', 'r8', 'linkmap', 'derived-data', 'simulator-arch', 'abi',
  ].map(name => [name, { type: 'string' }]));
  const { values: args, positionals } = parseArgs({ args: process.argv.slice(2), options: opts, allowPositionals: true });
  const [action] = positionals;
  demand(positionals.length === 1 && ['verify-android', 'materialize-android', 'apple-link', 'verify-link', 'android', 'apple'].includes(action),
    'mobile consumption action required');
  const file = name => args[name] && resolve(args[name]);
  const component = file('component'), receiptPath = file('receipt'), scratch = file('scratch');
  const output = file('output'), work = file('work'), cache = file('cache');
  const tools = Object.fromEntries(['go', 'javap', 'readelf'].filter(name => args[name]).map(name => [name, file(name)]));
  // The Xcode service supplies no compiler reservation descriptor.
  if (action === 'verify-link') delete process.env.POLARIS_BUILD_RESERVATION_FD;
  const runner = await nativeRunner();
  const { run, withLease } = runner;
  const source = readMobileSource();
  const linkPath = file('link-input'), linkSha = args['link-input-sha256'];
  let result;
  if (action === 'verify-link') {
    demand(linkPath && linkSha && scratch, 'original fixed Apple link context and scratch required');
    result = await verifyLink({ path: linkPath, expectedSha: linkSha, scratch, runner });
  } else if (['verify-android', 'materialize-android', 'apple-link'].includes(action)) {
    demand(component && receiptPath && work && cache, 'original inputs/owned slots required');
    if (action === 'materialize-android') result = await materializeAndroid({ component, receiptPath, tools, work, cache, runner });
    else if (action === 'apple-link') {
      demand(output && !Object.keys(tools).length, 'Apple link consumes static component and fixed link context only');
      result = await appleLink({ component, receiptPath, output, work, cache, runner });
    } else {
      result = await withLease(work, cache, source, async (ownedWork, ownedCache) => {
        demand(realpathSync(readReceipt(receiptPath).nativeInputs.cache) === ownedCache,
          'verifier must hold the actual producer cache lease');
        return verifyOriginal({ component, receiptPath, platform: 'android', tools, run, scratch: ownedWork });
      });
    }
  } else {
    const artifact = file('artifact'), r8 = file('r8'), linkmap = file('linkmap'), derived = file('derived-data');
    const abi = args.abi ?? 'arm64-v8a', simulatorArch = args['simulator-arch'];
    demand(artifact && receiptPath && scratch && output && lstatSync(scratch).isDirectory(), 'fixed owned scratch required');
    const ownedCache = realpathSync(readReceipt(receiptPath).nativeInputs.cache);
    result = await withLease(scratch, ownedCache, source, async ownedWork => {
      if (action === 'android') {
        demand(component && r8 && Object.keys(tools).length === 3 && ['arm64-v8a', 'armeabi-v7a', 'universal'].includes(abi),
          'Android original inputs/tools required');
        return inspectAndroidCarrier({ apk: artifact, abi, r8, component, receiptPath, tools, scratch: ownedWork, runner });
      }
      demand(linkPath && linkSha && linkmap && derived, 'Apple original link context required');
      return inspectAppleCarrier({ ipa: artifact, linkPath, linkSha, linkmap, receiptPath, scratch: ownedWork, derived, simulatorArch, runner });
    });
    result.fingerprint = receiptDigest(result);
    demand(!lstatSync(output, { throwIfNoEntry: false }), 'final carrier receipt already frozen');
    const { writeJson } = await import('./producer.mjs');
    await writeJson(output, result);
    console.log(JSON.stringify({ fingerprint: result.fingerprint, devicePassed: false, signatureTrustValidated: false }));
    return;
  }
  console.log(JSON.stringify({ fingerprint: result.fingerprint, AppAccepted: false, devicePassed: false }));
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(error => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
