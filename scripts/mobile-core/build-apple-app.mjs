#!/usr/bin/env node
// Explicit App + PacketTunnel compile/link entry, with original source proof.
import { fstatSync, linkSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync, statSync } from 'node:fs';
import { basename, isAbsolute, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { performance } from 'node:perf_hooks';
import { parseArgs } from 'node:util';
import { canonical, decode, load, require as demand, ROOT } from './fork-input.mjs';
import { filePin, parsePlist, readReceipt, receiptDigest } from './component-admission.mjs';
import { appleLink, inspectAppleCarrier, materializeAppleResources } from './mobile-consumption.mjs';
import { createNativeRunner, nativeCapacity, nativeEnvironment, privateDir, progressWrite } from './native-runner.mjs';
import { writeJson } from './producer.mjs';
import { pinned } from './source-admission.mjs';
import { verifyImport, writeArchiveZip } from './apple-handoff.mjs';
import { observeTLS } from './apple-admission.mjs';

const SCRIPT_ROOT = join(ROOT, 'scripts/mobile-core');
const TOOLS = ['source', 'go', 'gomobile', 'gobind', 'developer'];
const REQUIRED = [...TOOLS, 'work', 'cache'];
const SDK_BY_SLOT = new Map([['device-arm64', 'iphoneos'], ['sim-arm64', 'iphonesimulator']]);
const APPLE_PHASES = new Set(['component-build-or-revalidate', 'component-import-verification', 'component-link-input',
  'rust-target-build', 'host-swift-controls', 'app-extension-archive', 'final-carrier-inspection']);
const same = (left, right) => Buffer.from(canonical(left)).equals(Buffer.from(canonical(right)));

async function hostSwiftControls(root, appEnv, run) {
  const host = process.arch === 'arm64' ? 'arm64' : process.arch === 'x64' ? 'x86_64' : null;
  demand(host, 'unsupported actual Mac control architecture');
  const cfg = decode(pinned(join(ROOT, 'src-tauri/tauri.conf.json'), 65536).bytes);
  const minimum = cfg.bundle.macOS.minimumSystemVersion;
  demand(typeof minimum === 'string' && /^[0-9]+\.[0-9]+(?:\.[0-9]+)?$/u.test(minimum),
    'explicit Mac control deployment target required');
  const env = { ...appEnv };
  delete env.IPHONEOS_DEPLOYMENT_TARGET;
  const sdk = (await run(['xcrun', '--sdk', 'macosx', '--show-sdk-path'], { env })).trim();
  demand(isAbsolute(sdk) && statSync(sdk).isDirectory(), 'actual host Mac SDK required');
  Object.assign(env, { SDKROOT: sdk, PLATFORM_NAME: 'macosx', ARCHS: host, MACOSX_DEPLOYMENT_TARGET: minimum });
  const unit = join(root, 'measurement-conditions-test');
  const plugin = join(ROOT, 'crates/tauri-plugin-polaris-ios/ios');
  await run(['xcrun', '--sdk', 'macosx', 'swiftc', '-sdk', sdk, '-target', host + '-apple-macosx' + minimum,
    '-parse-as-library', join(plugin, 'Sources/MeasurementDeviceState.swift'),
    join(plugin, 'Tests/DeviceConditionsTests.swift'), '-o', unit],
  { env, log: join(root, 'original-swift-control-build.log'), capture: 'none' });
  await run([unit], { env, log: join(root, 'original-swift-control-run.log'), capture: 'none' });
}

async function zipApp(app, root, output, runner) {
  const view = mkdtempSync(join(root, 'ipa-view-'));
  const entries = [];
  const visit = (original, staged) => {
    const info = lstatSync(original);
    demand(!info.isSymbolicLink(), 'archived App contains a redirected entry');
    if (info.isDirectory()) {
      mkdirSync(staged, { recursive: true, mode: 0o700 });
      for (const name of readdirSync(original).sort()) visit(join(original, name), join(staged, name));
    } else {
      demand(info.isFile(), 'archived App contains a non-regular entry');
      // Same-filesystem snapshot: no extra App bytes or archive mutation.
      linkSync(original, staged);
      entries.push(relative(view, staged));
    }
  };
  try {
    visit(app, join(view, 'Payload', basename(app)));
    await writeArchiveZip(view, output, entries.sort(), runner);
  } finally {
    rmSync(view, { recursive: true, force: true });
  }
}

function childrenWithSuffix(root, suffix) {
  return readdirSync(root).filter(name => name.endsWith(suffix)).map(name => join(root, name));
}

function linkmaps(root) {
  const result = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) result.push(...linkmaps(path));
    else if (entry.name === 'PolarisPacketTunnel-LinkMap.txt') result.push(path);
  }
  return result;
}

async function main() {
  const { values: args } = parseArgs({ options: {
    ...Object.fromEntries([...REQUIRED, 'simulator-arch', 'signing-team', 'component-work', 'rust-target',
      'slot', 'progress-fd'].map(name => [name, { type: 'string' }])),
    simulator: { type: 'boolean' }, 'imported-component': { type: 'boolean' },
  } });
  demand(REQUIRED.every(name => args[name]), 'explicit source, tools, work and cache are required');
  demand(process.platform === 'darwin', 'Apple App producer requires the selected Mac host');
  demand(args['simulator-arch'] === undefined || args['simulator-arch'] === 'arm64', 'unsupported simulator architecture');
  const slot = args.simulator ? 'sim-arm64' : 'device-arm64';
  demand(args.slot === undefined || args.slot === slot, 'selected carrier slot differs');
  const platform = SDK_BY_SLOT.get(slot);
  let progressFd;
  if (args['progress-fd'] !== undefined) {
    progressFd = Number(args['progress-fd']);
    demand(/^[0-9]+$/u.test(args['progress-fd']) && Number.isSafeInteger(progressFd) && progressFd > 2
      && progressFd !== Number(process.env.POLARIS_BUILD_RESERVATION_FD), 'dedicated Apple progress descriptor required');
    fstatSync(progressFd);
  }
  const runner = createNativeRunner({ borrowedProgressFd: progressFd });
  let phase;
  const run = (argv, options) => runner.run(argv, { observationFd: progressFd,
    observationPhase: phase, observationSlot: slot, ...options });
  async function timed(name, action) {
    demand(APPLE_PHASES.has(name), 'unknown Apple progress phase');
    const previous = phase;
    phase = name;
    const opened = performance.now();
    const report = (event, completed = null) => {
      const row = { phase: name, slot, event, pid: process.pid,
        elapsedSeconds: Math.round(performance.now() - opened) / 1000, completed,
        scope: 'phase wait observation only; not compiler liveness or carrier acceptance' };
      if (progressFd === undefined) console.log('APPLE_PHASE_PROGRESS=' + JSON.stringify(row));
      else progressWrite(progressFd, 'APPLE_PHASE_PROGRESS=', row);
    };
    let complete = false, primary, observerError, timer;
    try {
      report('start');
      timer = setInterval(() => {
        try { report('waiting'); } catch (error) { observerError ??= error; }
      }, 30000);
      const result = await action();
      if (observerError) throw new Error('Apple phase observation failed', { cause: observerError });
      complete = true;
      return result;
    } catch (error) {
      primary = error;
      throw error;
    } finally {
      clearInterval(timer);
      phase = previous;
      try { report('end', complete); } catch (error) { if (!primary) throw error; }
    }
  }

  const value = load();
  const root = privateDir(args.work), cache = privateDir(args.cache);
  const component = privateDir(args['component-work'] ?? join(root, 'component'));
  const rustTarget = privateDir(args['rust-target'] ?? join(root, 'rust-target'));
  demand(new Set([root, cache, component, rustTarget].map(path => statSync(path).dev)).size === 1,
    'App/component/Rust/cache slots must share the reserved filesystem');
  const lease = (work, data, action, settings) => runner.withLease(work, data, value, action, settings);
  await lease(root, cache, async () => { await materializeAppleResources(); });
  if (args['imported-component']) {
    await lease(root, cache, () => timed('component-import-verification',
      () => verifyImport(component, cache, ...TOOLS.map(name => args[name]), runner)));
  } else {
    const argv = [process.execPath, join(SCRIPT_ROOT, 'producer.mjs'), 'apple',
      ...TOOLS.flatMap(name => ['--' + name, args[name]]), '--work', component, '--cache', cache];
    await timed('component-build-or-revalidate', () => run(argv,
      { log: join(root, 'original-component-runner.log'), capture: 'none', downstreamCleanupSeconds: 25 }));
  }

  const link = join(root, 'apple-link-input.json'), receiptPath = join(component, 'component-receipt.json');
  const componentCache = readReceipt(receiptPath).nativeInputs.cache;
  if (!lstatSync(link, { throwIfNoEntry: false })) {
    await timed('component-link-input', () => appleLink({
      component: join(component, 'Libbox.xcframework'), receiptPath, work: privateDir(join(root, 'link-work')),
      cache: componentCache, output: link, run, runner,
    }));
  }
  const build = async () => {
    let env = { ...process.env, DEVELOPER_DIR: realpathSync(args.developer), POLARIS_LIBBOX_GENERATION_ROOT: component,
      POLARIS_MOBILE_LINK_INPUT: link, POLARIS_MOBILE_LINK_INPUT_SHA256: filePin(link).sha256,
      POLARIS_MOBILE_SCRATCH: privateDir(join(root, 'link-check')), POLARIS_FORMAL_APP_BUILD: '1',
      POLARIS_APP_WORK: root, POLARIS_APP_RUST_TARGET: rustTarget, PLATFORM_NAME: platform, CONFIGURATION: 'release',
      ARCHS: 'arm64', PATH: join(ROOT, 'ui/node_modules/.bin') + ':' + process.env.PATH };
    env.SDKROOT = (await run(['xcrun', '--sdk', platform, '--show-sdk-path'], { env })).trim();
    const observation = nativeCapacity();
    console.log(JSON.stringify({ phase: 'apple-app-build-capacity-sample', capacity: observation }));
    const applied = nativeEnvironment(env, observation);
    env = applied.env;
    console.log('NATIVE_CAPACITY_OBSERVATION=' + JSON.stringify(applied.observation));
    await timed('rust-target-build', () => run([process.execPath, join(SCRIPT_ROOT, 'apple-rust.mjs'), 'build'], { env,
      log: join(root, 'original-rust-entry.log'), capture: 'none', downstreamCleanupSeconds: 25 }));
    if (!args.simulator) await timed('host-swift-controls', () => hostSwiftControls(root, env, run));
    const tlsRoot = privateDir(join(root, 'tls-link-input'));
    const tlsEnv = { ...env, DERIVED_FILE_DIR: tlsRoot };
    await observeTLS('prepare', tlsEnv, join(root, 'original-tls-prepare.log'), runner, run);
    Object.assign(env, { POLARIS_TLS_PREPARED_ROOT: tlsRoot,
      POLARIS_TLS_PREPARED_SHA256: filePin(join(tlsRoot, 'PolarisTLSFingerprintLinker.json')).sha256 });
    const derived = privateDir(join(root, 'derived-data')), archive = join(root, 'Polaris.xcarchive');
    demand(!lstatSync(archive, { throwIfNoEntry: false }), 'existing actual archive must not be overwritten');
    const command = ['xcodebuild', '-project', join(ROOT, 'src-tauri/gen/apple/polaris.xcodeproj'), '-scheme', 'polaris_iOS',
      '-configuration', 'release', '-derivedDataPath', derived,
      '-jobs', String(applied.observation.appliedEnvironment.CARGO_BUILD_JOBS), '-destination',
      args.simulator ? 'generic/platform=iOS Simulator' : 'generic/platform=iOS',
      '-archivePath', archive, 'archive', 'LD_GENERATE_MAP_FILE=YES'];
    if (args.simulator) command.push('ARCHS=arm64', 'ONLY_ACTIVE_ARCH=YES');
    if (args['signing-team']) {
      demand(!args.simulator && /^[0-9A-Za-z]+$/u.test(args['signing-team']), 'explicit device signing team differs');
      command.push('DEVELOPMENT_TEAM=' + args['signing-team']);
    } else command.push('CODE_SIGNING_ALLOWED=NO', 'CODE_SIGNING_REQUIRED=NO');
    await timed('app-extension-archive', () => run(command,
      { env, log: join(root, 'original-app-extension-link.log'), capture: 'none' }));
    const apps = childrenWithSuffix(join(archive, 'Products/Applications'), '.app');
    demand(apps.length === 1 && lstatSync(apps[0]).isDirectory(), 'actual archived host App required');
    const extensions = childrenWithSuffix(join(apps[0], 'PlugIns'), '.appex');
    demand(extensions.length === 1 && lstatSync(extensions[0]).isDirectory(), 'one actual PacketTunnel archive required');
    const info = await parsePlist(pinned(join(extensions[0], 'Info.plist'), 65536).bytes,
      { run, scratch: privateDir(join(root, 'carrier-plist')) });
    const executable = info.CFBundleExecutable;
    demand(typeof executable === 'string' && executable.length > 0 && basename(executable) === executable,
      'PacketTunnel executable name differs');
    await observeTLS('verify', { ...env, DERIVED_FILE_DIR: tlsRoot,
      TARGET_BUILD_DIR: extensions[0], EXECUTABLE_PATH: executable },
    join(root, 'original-tls-final-verify.log'), runner, run);
    const ipa = join(root, 'polaris-box-' + value.version + '-app-link-probe.ipa');
    demand(!lstatSync(ipa, { throwIfNoEntry: false }), 'existing carrier must not be overwritten');
    await zipApp(apps[0], root, ipa, runner);
    const maps = linkmaps(derived);
    demand(maps.length === 1, 'one actual extension link map required');
    const final = await timed('final-carrier-inspection', () => inspectAppleCarrier({
      ipa, linkPath: link, linkSha: filePin(link).sha256, linkmap: maps[0], receiptPath,
      scratch: privateDir(join(root, 'final-check')), derived, simulatorArch: args.simulator ? 'arm64' : undefined,
      run, runner,
    }));
    const rustReceipts = childrenWithSuffix(root, '.json').filter(path => basename(path).startsWith('rust-'));
    demand(rustReceipts.length === 1, 'one actual Rust target receipt required');
    const rust = readReceipt(rustReceipts[0]);
    const { fingerprint, ...body } = rust;
    demand(same(rust.App, final.App) && rust.devicePassed === false && fingerprint === receiptDigest(body),
      'actual Rust/App source receipt differs');
    final.rustInput = { receipt: rustReceipts[0], ...filePin(rustReceipts[0]) };
    final.fingerprint = receiptDigest(final);
    const output = join(root, 'app-carrier.json');
    demand(!lstatSync(output, { throwIfNoEntry: false }), 'final carrier receipt already frozen');
    writeJson(output, final);
    console.log('Actual App/PacketTunnel carrier/source predicate completed; device/signature trust remains separate');
  };
  await lease(root, cache, componentCache === cache ? build
    : () => runner.withSlotLocks(component, componentCache, build));
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try { await main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
