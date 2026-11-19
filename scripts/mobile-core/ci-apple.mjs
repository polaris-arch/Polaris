#!/usr/bin/env node
// Hosted unsigned native Apple component and App chain.
import { appendFileSync, lstatSync, realpathSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { canonical, load, prepareSource, require as demand, ROOT, sha } from './fork-input.mjs';
import { filePin, readReceipt } from './component-admission.mjs';
import { materializeAppleResources } from './mobile-consumption.mjs';
import { createNativeRunner, nativeCapacity, nativeEnvironment, privateDir } from './native-runner.mjs';
import { producerCode, writeJson } from './producer.mjs';
import { carrierSummary, context, freeze, location, requiredSlots, scopeFields, transportPin,
  verifyImport } from './apple-handoff.mjs';
import { admitResolvedSettings } from './apple-admission.mjs';
import { evidence, retireSlot } from './apple-evidence.mjs';

const SCRIPT = join(ROOT, 'scripts/mobile-core');
const SLOTS = ['device-arm64', 'sim-arm64'];
const TOOL_NAMES = ['source', 'go', 'gomobile', 'gobind', 'developer'];
const same = (left, right) => Buffer.from(canonical(left)).equals(Buffer.from(canonical(right)));
const exists = path => lstatSync(path, { throwIfNoEntry: false }) !== undefined;
const isFile = path => statSync(path, { throwIfNoEntry: false })?.isFile() === true;
const markerPath = (root, slot) => join(root, 'staging-preservation-' + slot + '.json');
const evidencePath = (root, slot) => join(root, 'apple-original-' + (slot ? slot + '-' : '') + 'evidence.zip');
const writeEnv = lines => appendFileSync(process.env.GITHUB_ENV, lines);

async function phaseTiming(phase, action) {
  const started = performance.now();
  const elapsed = () => Math.round(performance.now() - started) / 1000;
  const report = event => console.log('APPLE_PHASE_PROGRESS=' + JSON.stringify({ phase, event,
    elapsedSeconds: elapsed(), scope: 'parent wait observation only; not compiler liveness or carrier acceptance' }));
  let completed = false;
  report('start');
  const timer = setInterval(() => report('waiting'), 30000);
  try {
    const result = await action();
    completed = true;
    return result;
  } finally {
    clearInterval(timer);
    report('end');
    console.log('APPLE_PHASE_TIMING=' + JSON.stringify({ phase, elapsedSeconds: elapsed(), completed,
      scope: 'elapsed observation only' }));
  }
}

async function selectedAppleTools(value, run) {
  // The hosted runner label does not grant a fallback Xcode/SDK.
  const developer = realpathSync('/Applications/Xcode_27.1.app/Contents/Developer');
  const env = { ...process.env, DEVELOPER_DIR: developer };
  const xcode = (await run(['xcodebuild', '-version'], { env })).trim();
  demand(xcode === 'Xcode ' + value.apple.xcodeVersion + '\nBuild version ' + value.apple.xcodeBuild,
    'runner does not provide the exact selected Xcode; no fallback/skip');
  for (const [sdk, key] of [['iphoneos', 'iPhoneSDK'], ['iphonesimulator', 'simulatorSDK']]) {
    demand((await run(['xcrun', '--sdk', sdk, '--show-sdk-version'], { env })).trim() === value.apple[key],
      'runner does not provide the exact selected SDK');
  }
  return { developer, env, xcode };
}

async function requireCandidate(run) {
  const expected = process.env.POLARIS_APP_CANDIDATE;
  demand((await run(['git', 'rev-parse', 'HEAD'], { cwd: ROOT })).trim() === expected, 'hosted App candidate differs');
  return expected;
}

async function admit(runner) {
  const value = load();
  const candidate = await requireCandidate(runner.run);
  const { developer, xcode } = await selectedAppleTools(value, runner.run);
  const facts = { schema: 'polaris-apple-runner-tool-admission-v1', ...scopeFields(), candidate, xcode, developer,
    iPhoneSDK: value.apple.iPhoneSDK, simulatorSDK: value.apple.simulatorSDK, compilerByteIdentityChecked: false,
    carrierAccepted: false, devicePassed: false,
    scope: 'exact version/build/SDK admission only; original compiler byte verification still mandatory' };
  writeJson(join(location(), 'runner-tool-admission.json'), facts);
  console.log('APPLE_RUNNER_TOOL_ADMISSION=' + JSON.stringify(facts));
}

async function prepare(runner) {
  const { run } = runner;
  const value = load(), identity = await producerCode(run), root = location();
  await requireCandidate(run);
  const { developer, env, xcode } = await selectedAppleTools(value, run);
  // The Node admission owner waits for its own Xcode process group.
  await admitResolvedSettings(ROOT, root, env);
  const tools = privateDir(join(root, 'tools')), toolCache = privateDir(join(root, 'tool-cache'));
  const go = realpathSync((await run(['/usr/bin/which', 'go'])).trim());
  demand((await run([go, 'version'])).startsWith('go version go' + value.goVersion + ' '), 'actual Go differs');
  Object.assign(env, { GOBIN: tools, GOTOOLCHAIN: 'local', GOWORK: 'off', GOFLAGS: '-mod=readonly',
    GOPATH: join(toolCache, 'go-path'), GOCACHE: join(toolCache, 'go-build'), GOMODCACHE: join(toolCache, 'go-mod') });
  const observation = nativeCapacity();
  console.log(JSON.stringify({ phase: 'apple-tool-prepare-capacity-sample', capacity: observation }));
  const applied = nativeEnvironment(env, observation);
  console.log('NATIVE_CAPACITY_OBSERVATION=' + JSON.stringify(applied.observation));
  await run([go, 'install', '-trimpath', 'github.com/sagernet/gomobile/cmd/gomobile@' + value.gomobileVersion,
    'github.com/sagernet/gomobile/cmd/gobind@' + value.gobindVersion], { env: applied.env,
    log: join(root, 'original-generator-prepare.log'), capture: 'none', observationFd: 1, observationPhase: 'tool-prepare' });
  const paths = { go, gomobile: join(tools, 'gomobile'), gobind: join(tools, 'gobind'), developer,
    source: join(root, 'fork-source'), 'component-work': privateDir(join(root, 'component')),
    cache: privateDir(join(root, 'cache')), 'rust-target': privateDir(join(root, 'cache/rust-target')) };
  await prepareSource(paths.source, value, { run });
  demand(same(await producerCode(run), identity), 'App producer source changed during preparation');
  const facts = { ...scopeFields(), App: identity, paths, xcode,
    tools: Object.fromEntries(['go', 'gomobile', 'gobind'].map(name => [name, filePin(paths[name])])),
    rustc: await run(['rustc', '-vV']), devicePassed: false };
  facts.fingerprint = sha(canonical(facts));
  writeJson(join(root, 'tools.json'), facts);
  writeEnv('POLARIS_FORMAL_APPLE_ROOT=' + root + '\nDEVELOPER_DIR=' + developer + '\n');
  console.log('Exact Apple source/tool slots prepared; no compiler, signing or device action');
}

async function build(runner) {
  const root = location(), tools = readReceipt(join(root, 'tools.json'));
  demand(Object.entries(scopeFields()).every(([key, value]) => same(tools[key], value)),
    'prepared Apple carrier scope differs before build');
  const { paths } = tools, { run } = runner;
  await phaseTiming('frontend-resources', () => runner.withLease(privateDir(join(root, 'frontend-work')), paths.cache,
    load(), async () => {
      await run([process.execPath, join(ROOT, 'scripts/fetch-dashboard.mjs')], { cwd: ROOT,
        log: join(root, 'original-dashboard.log'), capture: 'none', observationFd: 1, observationPhase: 'frontend-dashboard' });
      await run(['pnpm', '--dir', join(ROOT, 'ui'), 'build'], { cwd: ROOT, log: join(root, 'original-frontend.log'),
        capture: 'none', observationFd: 1, observationPhase: 'frontend-build' });
      await materializeAppleResources();
    }));
  await phaseTiming('shared-component', () => run([process.execPath, join(SCRIPT, 'producer.mjs'), 'apple',
    ...TOOL_NAMES.flatMap(name => ['--' + name, paths[name]]), '--work', paths['component-work'], '--cache', paths.cache,
    '--progress-fd', '3'], { cwd: ROOT, log: join(root, 'original-component-producer.log'), capture: 'none',
    progressFd: { fd: 1, childFd: 3 }, observationPhase: 'component-build-or-revalidate', downstreamCleanupSeconds: 25 }));
  await phaseTiming('component-handoff-freeze', () => freeze(root, runner));
}

async function importLocal(runner) {
  const root = location(), value = load();
  await selectedAppleTools(value, runner.run);
  const tools = readReceipt(join(root, 'tools.json'));
  const { paths } = tools;
  await phaseTiming('component-local-verification', () => runner.withLease(privateDir(join(root, 'import-work')),
    paths.cache, value, () => verifyImport(paths['component-work'], paths.cache, ...TOOL_NAMES.map(name => paths[name]), runner)));
  demand(await runner.run(['rustc', '-vV']) === tools.rustc, 'actual consumer Rust toolchain differs');
  writeJson(join(root, 'import-observation.json'), { schema: 'polaris-apple-import-observation-v1',
    context: await context(runner), transport: readReceipt(join(root, 'component-transport.json')),
    completed: true, devicePassed: false });
}

async function carrier(slot, runner) {
  const root = location();
  demand(isFile(join(root, 'component-handoff.json')), 'verified component import required');
  const imported = readReceipt(join(root, 'import-observation.json'));
  const scope = await context(runner);
  demand(imported.completed === true && same(imported.context, scope), 'same-run complete component verification required');
  // Both formal targets overwrite the one checkout Externals/arm64 staging path.
  for (const previous of SLOTS) {
    if (previous === slot) break;
    const marker = markerPath(root, previous);
    if (exists(marker)) {
      const facts = readReceipt(marker);
      demand(facts.slot === previous && same(facts.context, scope) && facts.complete === true
        && same(facts.archive, transportPin(evidencePath(root, previous))),
      'previous shared staging not safely preserved; replacement blocked');
    }
  }
  const { paths } = readReceipt(join(root, 'tools.json'));
  const argv = [process.execPath, join(SCRIPT, 'build-apple-app.mjs'),
    ...Object.entries(paths).flatMap(([name, path]) => ['--' + name, path]),
    '--work', privateDir(join(root, slot)), '--slot', slot, '--imported-component', '--progress-fd', '3'];
  if (slot === 'sim-arm64') argv.push('--simulator', '--simulator-arch', 'arm64');
  const marker = markerPath(root, slot);
  demand(!exists(marker), 'carrier staging scope already started');
  writeJson(marker, { slot, context: scope, complete: false });
  await phaseTiming(slot, () => runner.run(argv, { cwd: ROOT, log: join(root, 'original-' + slot + '.log'),
    capture: 'none', progressFd: { fd: 1, childFd: 3 }, observationPhase: 'carrier-command', observationSlot: slot,
    downstreamCleanupSeconds: 43 }));
  await carrierSummary(root, slot, runner);
}

async function main() {
  const { positionals, values } = parseArgs({ allowPositionals: true, options: { slot: { type: 'string' } } });
  const allowed = requiredSlots();
  const [action] = positionals;
  demand(positionals.length === 1 && (values.slot === undefined || allowed.includes(values.slot)),
    'exact hosted Apple action and requested slot required');
  const runner = createNativeRunner();
  const handlers = { admit, prepare, build, 'import-local': importLocal };
  if (Object.hasOwn(handlers, action)) {
    demand(values.slot === undefined, 'unexpected Apple carrier slot');
    await handlers[action](runner);
  } else if (action === 'evidence') {
    await evidence({ slot: values.slot, publishEnv: true }, runner);
  } else {
    demand(values.slot !== undefined && (action === 'carrier' || action === 'retire-slot'), 'exact slot required');
    await (action === 'carrier' ? carrier(values.slot, runner) : retireSlot(values.slot, runner));
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try { await main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
