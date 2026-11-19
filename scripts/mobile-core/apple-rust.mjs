// Formal Apple Cargo code is compiled under the archive parent's lease.
import { constants, closeSync, createReadStream, createWriteStream, existsSync, fsyncSync, linkSync, lstatSync, mkdirSync,
  openSync, readFileSync, readSync, realpathSync, renameSync, statSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { pipeline } from 'node:stream/promises';
import { fileURLToPath } from 'node:url';
import { container } from '../apple-carrier.mjs';
import { canonical, load, require as demand, ROOT, sha } from './fork-input.mjs';
import { AppleBuildInfoBatch, filePin, readReceipt } from './component-admission.mjs';
import { appIdentity, verifyLink } from './mobile-consumption.mjs';
import { createNativeRunner, privateDir } from './native-runner.mjs';
import { writeJson } from './producer.mjs';
import { pinned } from './source-admission.mjs';
import { appleNativeTextDefinition } from './apple-admission.mjs';

const BRIDGES = ['_init_plugin_dialog', '_init_plugin_notification', '_init_plugin_polaris_ios',
  '_init_plugin_shell', '_log_stdout', '_on_webview_created', '_register_plugin', '_run_plugin_command'];
const EXPORT_POLICY = ['Cargo.lock', 'crates/tauri-plugin-polaris-ios/ios/Sources/VpnPlugin.swift'];
const MAX_ARCHIVE = 1024 ** 3;
export const CARGO_TIMING_BYTES = 4 * 1024 ** 2;
const CARGO_LOCK = join(ROOT, 'Cargo.lock'), CONFIG = join(ROOT, 'src-tauri/tauri.ios.conf.json');
const XCODE_PROJECT = join(ROOT, 'src-tauri/gen/apple');
const STATIC_ARCHIVE = Buffer.from('!<arch>\n');
const TARGETS = new Map([
  ['aarch64-apple-ios', { platform: 'iphoneos', slice: 'ios-arm64', platformCode: 2 }],
  ['aarch64-apple-ios-sim', { platform: 'iphonesimulator', slice: 'ios-arm64-simulator', platformCode: 7 }],
]);

const equal = (left, right) => Buffer.from(canonical(left)).equals(Buffer.from(canonical(right)));
const withPath = (path, pin = filePin(path)) => ({ path, ...pin });
const exportPolicy = () => Object.fromEntries(EXPORT_POLICY.map(path => [path, filePin(join(ROOT, path))]));
const links = (env, scratch, runner) => verifyLink({ path: env.POLARIS_MOBILE_LINK_INPUT,
  expectedSha: env.POLARIS_MOBILE_LINK_INPUT_SHA256, scratch, runner });
const toolPinsCurrent = tools => Object.values(tools).every(({ path, ...pin }) => equal(filePin(path), pin));

export function targetFor(platform, arches, configuration) {
  demand(configuration === 'release', 'formal App hook requires release configuration');
  const target = [...TARGETS].find(([, row]) => row.platform === platform);
  demand(arches === 'arm64' && target, 'unsupported actual App platform/architecture');
  return target[0];
}

export async function selectedWireBytes(raw, triple) {
  const target = TARGETS.get(triple);
  demand(target && raw.length <= MAX_ARCHIVE, 'bounded selected wire target required');
  const batch = new AppleBuildInfoBatch();
  const observed = container(raw, batch.observe);
  demand(observed.kind === 'bsd-ar' || observed.kind === 'fat-ar', 'actual static wire input required');
  const slices = observed.kind === 'fat-ar' ? observed.slices : [{ offset: 0, bytes: raw.length, archive: observed }];
  const matching = [];
  for (const row of slices) {
    const code = row.archive.members.filter(member => member.classification === 'code').map(member => member.machO);
    demand(code.length && new Set(code.map(obj => obj.cpu)).size === 1, 'mixed or missing wire architecture');
    const { cpu, subCPU } = code[0];
    demand(row.cpu === undefined || (row.cpu === cpu && row.subCPU === subCPU),
      'fat header/payload architecture differs');
    demand(code.every(obj => obj.cpu === cpu && obj.subCPU === subCPU), 'mixed wire CPU/subtype');
    const platforms = new Set(code.flatMap(obj => obj.platformCommands.map(command => command.platform)));
    demand(platforms.size === 1, 'mixed or missing wire platform');
    if (cpu === 0x100000c && platforms.has(target.platformCode)) matching.push(row);
  }
  demand(matching.length === 1, 'exact Cargo architecture/platform wire slice required');
  const { offset, bytes } = matching[0];
  const thin = raw.subarray(offset, offset + bytes);
  demand(thin.subarray(0, STATIC_ARCHIVE.length).equals(STATIC_ARCHIVE), 'selected wire slice is not a static archive');
  await batch.verify();
  return [thin, { target: triple, cpu: 0x100000c, platform: target.platformCode, offset, bytes,
    container: observed.kind }];
}

async function selectedLibrary(link, triple, target) {
  const selected = link.componentOutput.slices[target.slice];
  const library = realpathSync(join(link.framework, selected.libraryIdentifier, 'Libbox.framework/Libbox'));
  const original = pinned(library, MAX_ARCHIVE);
  demand(equal(original.pin, selected.binary), 'actual selected wire archive differs');
  const [thin, selection] = await selectedWireBytes(original.bytes, triple);
  return { library, pin: original.pin, thin, selection };
}

function noRedirect(path) {
  for (let parent = path; ; parent = dirname(parent)) {
    demand(!lstatSync(parent, { throwIfNoEntry: false })?.isSymbolicLink(), 'Rust App link output redirected');
    if (parent === ROOT) break;
  }
}

async function stageArchive(archive, stage) {
  demand(!lstatSync(stage, { throwIfNoEntry: false }), 'unaccepted Rust link stage retained');
  const fd = openSync(stage, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try {
    await pipeline(createReadStream(archive, { flags: constants.O_RDONLY | constants.O_NOFOLLOW }),
      createWriteStream(stage, { fd, autoClose: false }));
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
}

export async function withCargoTimingReport(target, root, triple, compile) {
  const report = join(target, 'cargo-timings/cargo-timing.html');
  let previous;
  try { previous = lstatSync(report, { bigint: true, throwIfNoEntry: false })?.mtimeNs; }
  catch { previous = false; }
  try {
    return await compile();
  } finally {
    try {
      demand(previous !== false && realpathSync(report) === report, 'Cargo timing redirected or starting state unknown');
      const before = lstatSync(report, { bigint: true });
      demand(before.isFile() && before.mtimeNs !== previous, 'fresh bounded Cargo timing diagnostic required');
      const { bytes } = pinned(report, CARGO_TIMING_BYTES);
      const after = lstatSync(report, { bigint: true });
      demand(after.dev === before.dev && after.ino === before.ino && after.mtimeNs === before.mtimeNs
        && after.size === before.size, 'Cargo timing diagnostic changed during read');
      const output = join(root, 'diagnostic-cargo-timing-' + triple + '.html');
      const partial = output + '.part';
      writeFileSync(partial, bytes, { flag: 'wx', mode: 0o600 });
      linkSync(partial, output);
      unlinkSync(partial);
    } catch {
      // Timing is a diagnostic; it cannot replace the original Cargo failure.
      try { process.stderr.write('Cargo timing diagnostic unavailable; not carrier acceptance\n'); } catch {}
    }
  }
}

async function buildReserved(env, runner, value, root) {
  demand(process.platform === 'darwin' && env.POLARIS_FORMAL_APP_BUILD === '1',
    'formal Apple hook must be called by the admitted App builder');
  const { run } = runner;
  const triple = targetFor(env.PLATFORM_NAME, env.ARCHS, env.CONFIGURATION), target = TARGETS.get(triple);
  const targetDir = privateDir(env.POLARIS_APP_RUST_TARGET);
  demand(statSync(targetDir).dev === statSync(root).dev, 'App Rust target must share the reserved filesystem');
  const status = phase => appIdentity({ statusLog: join(root, 'source-status-' + phase + '-rust.json'), runner });
  const identity = await status('before');
  const link = await links(env, privateDir(join(root, 'rust-link-check')), runner);
  demand(equal(link.App, identity), 'formal Rust source differs from frozen App input');
  const native = command => run(command, { env });
  const sdk = realpathSync((await native(['xcrun', '--sdk', target.platform, '--show-sdk-path'])).trim());
  demand(realpathSync(env.SDKROOT) === sdk, 'Xcode SDKROOT differs from selected SDK');
  const sdkVersion = value.apple[target.platform === 'iphoneos' ? 'iPhoneSDK' : 'simulatorSDK'];
  demand((await native(['xcrun', '--sdk', target.platform, '--show-sdk-version'])).trim() === sdkVersion,
    'actual App SDK version differs');
  demand((await native(['xcodebuild', '-version'])).trim()
    === 'Xcode ' + value.apple.xcodeVersion + '\nBuild version ' + value.apple.xcodeBuild,
  'actual Xcode version/build differs');
  const macSdk = (await native(['xcrun', '--sdk', 'macosx', '--show-sdk-path'])).trim();
  const cargoEnv = { ...env, CARGO_TARGET_DIR: targetDir, IPHONEOS_DEPLOYMENT_TARGET: '17.0',
    TAURI_CONFIG: JSON.stringify(JSON.parse(readFileSync(CONFIG, 'utf8'))), TAURI_ENV_TARGET_TRIPLE: triple,
    TAURI_ENV_ARCH: 'aarch64', TAURI_ENV_PLATFORM: 'ios', TAURI_ENV_FAMILY: 'unix', TAURI_IOS_PROJECT_PATH: XCODE_PROJECT };
  const suffix = triple.replaceAll('-', '_');
  const { library, pin: originalPin, thin, selection } = await selectedLibrary(link, triple, target);
  const wire = join(root, 'wire-' + triple + '.a');
  demand(!lstatSync(wire, { throwIfNoEntry: false }), 'actual selected wire output already retained');
  writeFileSync(wire, thin, { flag: 'wx' });
  demand(equal(filePin(library), originalPin), 'original wire archive changed during selection');
  cargoEnv.POLARIS_APPLE_WIRE_ARCHIVE = wire;
  for (const name of ['CFLAGS', 'CXXFLAGS']) {
    cargoEnv[name + '_' + suffix] = '-isysroot ' + sdk;
    for (const host of ['aarch64_apple_darwin', 'x86_64_apple_darwin']) cargoEnv[name + '_' + host] = '-isysroot ' + macSdk;
  }
  cargoEnv['OBJC_INCLUDE_PATH_' + suffix] = join(sdk, 'usr/include');
  const tools = {};
  for (const name of ['cargo', 'rustc']) {
    const path = realpathSync((await native(['rustup', 'which', name])).trim());
    tools[name] = withPath(path);
  }
  cargoEnv.RUSTC = tools.rustc.path;
  const command = [tools.cargo.path, 'rustc', '--locked', '--release', '--timings', '--lib', '--package', 'polaris',
    '--target', triple, '--features', 'tauri/custom-protocol', '--crate-type', 'staticlib'];
  await withCargoTimingReport(targetDir, root, triple, () => run(command, { cwd: join(ROOT, 'src-tauri'),
    env: cargoEnv, capture: 'none', log: join(root, 'original-rust-' + triple + '.log') }));
  const archive = join(targetDir, triple, 'release/libpolaris_lib.a');
  const fd = openSync(archive, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const magic = Buffer.alloc(STATIC_ARCHIVE.length);
    demand(readSync(fd, magic) === magic.length && magic.equals(STATIC_ARCHIVE), 'actual Rust static archive required');
  } finally { closeSync(fd); }
  const symbols = await run(['xcrun', 'nm', '--no-llvm-bc', '-gU', archive], { env,
    log: join(root, 'original-rust-symbols-' + triple + '.log') });
  demand(appleNativeTextDefinition(symbols, '_start_app', true), 'actual Rust start_app definition missing');
  for (const bridge of BRIDGES) demand(appleNativeTextDefinition(symbols, bridge),
    'actual global Swift bridge definition missing: ' + bridge);

  const pin = filePin(archive);
  const dest = join(XCODE_PROJECT, 'Externals', env.ARCHS, 'release/libapp.a');
  noRedirect(dest);
  mkdirSync(dirname(dest), { recursive: true });
  const stage = dest + '.formal-stage';
  await stageArchive(archive, stage);
  demand(equal(filePin(stage), pin), 'Rust static archive copy differs');
  renameSync(stage, dest);
  demand(equal(filePin(dest), pin) && equal(await status('after'), identity), 'Rust link bytes/source changed');
  const wirePin = { bytes: thin.length, sha256: sha(thin) };
  const receipt = { schema: 'polaris-formal-apple-rust-v1', App: identity, target: triple, sdk, sdkVersion, command,
    rustc: await native([tools.rustc.path, '-vV']), tools, cargoLock: filePin(CARGO_LOCK), iosConfig: filePin(CONFIG),
    swiftBridgeDefinitions: BRIDGES, swiftExportPolicy: exportPolicy(),
    linkInputSha256: env.POLARIS_MOBILE_LINK_INPUT_SHA256, originalWireArchive: withPath(library),
    wireSelection: selection, wireArchive: withPath(wire), archive: pin, linkedArchive: dest, devicePassed: false };
  receipt.fingerprint = sha(canonical(receipt));
  demand(equal(filePin(wire), wirePin) && equal(filePin(library), originalPin), 'wire input changed during Rust compile');
  demand(toolPinsCurrent(tools), 'actual Cargo/Rust compiler changed during build');
  const output = join(root, 'rust-' + triple + '.json');
  demand(!lstatSync(output, { throwIfNoEntry: false }), 'actual Rust receipt already frozen');
  writeJson(output, receipt);
}

export async function buildAppleRust(env = process.env) {
  const root = privateDir(env.POLARIS_APP_WORK);
  demand(env.POLARIS_BUILD_RESERVATION_FD !== undefined,
    'Rust compile must inherit the admitted App reservation before Xcode');
  const runner = createNativeRunner({ env });
  const value = load();
  await runner.withReservation(root, value, () => buildReserved(env, runner, value, root));
}

export async function verifyAppleRust(originalEnv = process.env) {
  // Xcode's build service does not carry the parent's registered descriptor.
  const env = { ...originalEnv };
  delete env.POLARIS_BUILD_RESERVATION_FD;
  demand(env.POLARIS_FORMAL_APP_BUILD === '1', 'formal App verification entry required');
  const runner = createNativeRunner({ env });
  const root = privateDir(env.POLARIS_APP_WORK);
  const triple = targetFor(env.PLATFORM_NAME, env.ARCHS, env.CONFIGURATION), target = TARGETS.get(triple);
  const link = await links(env, privateDir(join(root, 'rust-link-check')), runner);
  const receipt = readReceipt(join(root, 'rust-' + triple + '.json'));
  const { fingerprint, ...body } = receipt;
  const expected = join(XCODE_PROJECT, 'Externals', env.ARCHS, 'release/libapp.a');
  demand(receipt.schema === 'polaris-formal-apple-rust-v1' && receipt.target === triple
    && fingerprint === sha(canonical(body)) && equal(receipt.App, link.App)
    && receipt.linkInputSha256 === env.POLARIS_MOBILE_LINK_INPUT_SHA256 && receipt.linkedArchive === expected
    && equal(filePin(expected), receipt.archive) && equal(receipt.cargoLock, filePin(CARGO_LOCK))
    && equal(receipt.iosConfig, filePin(CONFIG)) && equal(receipt.swiftBridgeDefinitions, BRIDGES)
    && equal(receipt.swiftExportPolicy, exportPolicy()) && realpathSync(env.SDKROOT) === receipt.sdk
    && receipt.devicePassed === false, 'actual compiled Rust/Xcode input differs');
  const { library, pin, thin, selection } = await selectedLibrary(link, triple, target);
  const wire = join(root, 'wire-' + triple + '.a');
  const wirePin = { bytes: thin.length, sha256: sha(thin) };
  demand(equal(receipt.originalWireArchive, withPath(library, pin)) && equal(filePin(library), pin)
    && equal(receipt.wireSelection, selection) && equal(receipt.wireArchive, withPath(wire, wirePin))
    && equal(filePin(wire), wirePin), 'actual selected Rust wire input differs');
  demand(toolPinsCurrent(receipt.tools), 'actual Cargo/Rust input changed before link');
}

if (process.argv[1] && existsSync(process.argv[1]) && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [, , action, ...extra] = process.argv;
    demand(extra.length === 0 && (action === 'build' || action === 'verify'), 'formal Apple Rust build or verify required');
    await (action === 'build' ? buildAppleRust() : verifyAppleRust());
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
