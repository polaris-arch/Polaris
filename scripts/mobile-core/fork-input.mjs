// Formal mobile input declarations and exact native binding arguments.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, realpathSync, rmdirSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, dirname, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { bindingPlans, canonical, decode as sourceDecode, gitEnvironment,
  pinned, readMobileSource, validateSourceBytes } from './source-admission.mjs';

export const ROOT = resolve(fileURLToPath(new URL('../..', import.meta.url)));
export const SOURCE_ADMISSION = fileURLToPath(new URL('source-admission.mjs', import.meta.url));
const CONTRACT_BYTES = 65536;
export { canonical, gitEnvironment };

export function require(condition, message) {
  if (!condition) throw new Error(message);
}

export const sha = bytes => createHash('sha256').update(bytes).digest('hex');
export const decode = (raw, maximum = CONTRACT_BYTES) => sourceDecode(raw, maximum);
export const load = path => path === undefined ? readMobileSource() : validateSourceBytes(pinned(path, CONTRACT_BYTES).bytes);
export const validate = value => { validateSourceBytes(canonical(value)); return value; };

export function requireGitRoot(source) {
  const actual = execFileSync('git', ['-C', source, 'rev-parse', '--show-toplevel'], {
    env: gitEnvironment(), encoding: 'utf8', timeout: 10000, maxBuffer: CONTRACT_BYTES,
    stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
  require(realpathSync(actual) === realpathSync(source), 'Git worktree differs from actual source root');
}

async function checkSource(source, value, { run, prepare, generatorOutputs }) {
  // Keep all Git children inside the supervised runner's original owned group.
  const execute = run ?? (await import('./native-runner.mjs')).createNativeRunner().run;
  const raw = canonical(value);
  require(raw.length <= CONTRACT_BYTES, 'mobile input contract is too large');
  const parent = prepare ? dirname(resolve(source)) : realpathSync(tmpdir());
  require(realpathSync(parent) === parent, 'mobile declaration slot parent is redirected');
  const directory = mkdtempSync(join(parent, '.mobile-source-declaration-'));
  const declaration = join(directory, 'declaration.json');
  try {
    writeFileSync(declaration, raw, { flag: 'wx', mode: 0o600 });
    const command = [process.execPath, SOURCE_ADMISSION,
      prepare ? '--prepare-checkout' : '--verify-checkout-file', source, declaration];
    if (generatorOutputs) command.push('--generator-outputs');
    const output = await execute(command, {
      env: gitEnvironment(),
      ...(prepare ? {} : { timeoutSeconds: 60 }),
    });
    return decode(Buffer.from(output, 'utf8'));
  } finally {
    try { unlinkSync(declaration); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
    rmdirSync(directory);
  }
}

export const verifySource = (source, value, { run, generatorOutputs } = {}) =>
  checkSource(source, value, { run, generatorOutputs });
export const prepareSource = (source, value, { run } = {}) =>
  checkSource(source, value, { run, prepare: true });

function authority(value) {
  return bindingPlans(validateSourceBytes(canonical(value)));
}

export function declaredPlan(platform, value) {
  require(platform === 'android' || platform === 'apple', 'unknown formal mobile platform');
  const plan = authority(value).platforms[platform];
  return { ...plan, facts: { ...plan.facts, source: value } };
}

export function bindCommand(platform, value, gomobile, output, fingerprint, bootclasspath) {
  require(platform === 'android' || platform === 'apple', 'unknown formal mobile platform');
  require(typeof fingerprint === 'string' && /^[0-9a-f]{64}$/.test(fingerprint), 'actual binding input fingerprint required');
  const plans = authority(value);
  const inputs = { gomobile, output, ldflags: plans.linkerFlagsPrefix + fingerprint };
  if (platform === 'android') {
    require(bootclasspath != null, 'explicit Android bootclasspath required');
    inputs.minimumAPI = canonical(value.android.minimumAPI).toString();
    inputs.bootclasspath = bootclasspath;
  }
  return plans.platforms[platform].argv.map(arg => typeof arg === 'string' ? arg : String(inputs[arg.input]));
}

export function toolEnvironment(go, gomobile, gobind, cache, scratch, {
  sdk, ndk, jdk, developer,
} = {}) {
  const env = gitEnvironment();
  for (const key of ('GOWORK GOFLAGS GOTOOLCHAIN GOROOT GOOS GOARCH GOARM GOARM64 GOBIN GOPATH CGO_CFLAGS CGO_CPPFLAGS ' +
    'CGO_LDFLAGS CC CXX ANDROID_HOME ANDROID_SDK_ROOT ANDROID_NDK_HOME ANDROID_NDK_ROOT DEVELOPER_DIR SDKROOT ' +
    'MACOSX_DEPLOYMENT_TARGET IPHONEOS_DEPLOYMENT_TARGET').split(' ')) delete env[key];
  Object.assign(env, {
    PATH: [...[go, gomobile, gobind].map(dirname), env.PATH ?? ''].join(delimiter),
    GOWORK: 'off', GOFLAGS: '-mod=readonly', GOTOOLCHAIN: 'local', CGO_ENABLED: '1',
    GOCACHE: join(cache, 'go-build'), GOMODCACHE: join(cache, 'go-mod'),
    GOPATH: join(cache, 'go-path'), TMPDIR: scratch,
  });
  if (sdk != null) {
    require(ndk != null && jdk != null && developer == null, 'explicit Android tools required');
    Object.assign(env, { ANDROID_HOME: sdk, ANDROID_SDK_ROOT: sdk, ANDROID_NDK_HOME: ndk,
      ANDROID_NDK_ROOT: ndk, JAVA_HOME: jdk, PATH: join(jdk, 'bin') + delimiter + env.PATH });
  } else {
    require(developer != null && ndk == null && jdk == null, 'explicit Apple developer directory required');
    delete env.PKG_CONFIG_PATH;
    env.DEVELOPER_DIR = developer;
  }
  return env;
}

async function showPlan() {
  if (process.argv.length === 3 && process.argv[2] === '--help') {
    console.log('usage: fork-input.mjs android|apple --source ROOT --gomobile PATH --output PATH [--bootclasspath PATH]');
    return;
  }
  const { positionals, values } = parseArgs({
    allowPositionals: true,
    options: Object.fromEntries(['source', 'gomobile', 'output', 'bootclasspath'].map(name => [name, { type: 'string' }])),
  });
  require(positionals.length === 1, 'one formal mobile platform required');
  const [platform] = positionals;
  require(values.source && values.gomobile && values.output, 'explicit source, gomobile and output required');
  const value = load();
  const sourceObservation = await verifySource(values.source, value);
  const { schema, facts, summary } = declaredPlan(platform, value);
  const fingerprint = sha(canonical(facts));
  const command = bindCommand(platform, value, values.gomobile, values.output, fingerprint, values.bootclasspath);
  console.log(JSON.stringify({ schema, facts, fingerprint, command, ...summary, sourceObservation }, null, 2));
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try { await showPlan(); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
