#!/usr/bin/env node
// Prepare the hosted Android source/tool slots for the formal component build.
import { appendFileSync, realpathSync, statSync, writeSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { ROOT, load, prepareSource, require } from './fork-input.mjs';
import { createNativeRunner, nativeCapacity, nativeEnvironment, privateDir } from './native-runner.mjs';

export function emitAndroidInputs(env, paths, artifactName) {
  const settings = {
    POLARIS_ANDROID_GO: paths.go,
    POLARIS_ANDROID_GOMOBILE: paths.gomobile,
    POLARIS_ANDROID_GOBIND: paths.gobind,
    POLARIS_ANDROID_NDK: paths.ndk,
    POLARIS_ANDROID_JDK: paths.jdk,
    POLARIS_ANDROID_SDK: paths.sdk,
    POLARIS_ANDROID_SOURCE: paths.source,
    POLARIS_ANDROID_WORK: paths.work,
    POLARIS_ANDROID_CACHE: paths.cache,
    POLARIS_ANDROID_CONSUME: paths.consume,
    POLARIS_ANDROID_VERIFY: paths.verify,
    POLARIS_ANDROID_CARRIER: paths.carrier,
    POLARIS_ANDROID_COMPONENT: join(paths.work, artifactName),
    POLARIS_ANDROID_RECEIPT: join(paths.work, 'component-receipt.json'),
    POLARIS_ANDROID_READELF: join(paths.ndk, 'toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf'),
  };
  for (const value of Object.values(settings)) {
    require(typeof value === 'string' && value.length > 0 && !/[\r\n]/.test(value),
      'output paths contain line controls or are missing');
  }
  require(env.GITHUB_ENV, 'actual hosted environment file required');
  appendFileSync(env.GITHUB_ENV, Object.entries(settings).map(([key, value]) => key + '=' + value + '\n').join(''));
}

export async function prepareAndroid({
  env = process.env, home = homedir(), run = createNativeRunner({ env }).run,
  prepare = prepareSource, observeCapacity = nativeCapacity,
  report = row => writeSync(1, JSON.stringify(row) + '\n'),
} = {}) {
  const source = load();
  require(env.GITHUB_ACTIONS === 'true', 'hosted Android entry only; local builds use reserved explicit slots');
  const root = privateDir(join(home, 'Code/polaris-mobile-ci'));
  const sdk = realpathSync(env.ANDROID_SDK_ROOT || env.ANDROID_HOME || '');
  if (env.ANDROID_HOME) {
    require(realpathSync(env.ANDROID_HOME) === sdk, 'conflicting Android SDK roots');
  }
  const installer = ['node', join(ROOT, 'scripts/mobile-core/prepare-ndk-linux.mjs'),
    '--profile', join(ROOT, source.android.ndkArchivePath), '--sdk', sdk];
  await run([...installer, '--check-license-only']);
  const tools = privateDir(join(root, 'tools'));
  const go = (await run(['which', 'go'])).trim();
  require((await run([go, 'version'])).startsWith('go version go' + source.goVersion + ' '), 'actual Go differs');
  const toolCache = privateDir(join(root, 'tool-cache'));
  const toolEnv = { ...env, GOBIN: tools, GOTOOLCHAIN: 'local', GOWORK: 'off', GOFLAGS: '-mod=readonly',
    GOPATH: join(toolCache, 'go-path'), GOCACHE: join(toolCache, 'go-build'),
    GOMODCACHE: join(toolCache, 'go-mod') };
  const sample = observeCapacity();
  report({ phase: 'android-tool-prepare-capacity-sample', capacity: sample });
  const capacity = nativeEnvironment(toolEnv, sample);
  report({ phase: 'android-tool-prepare-capacity', capacity: capacity.observation });
  await run([go, 'install', 'github.com/sagernet/gomobile/cmd/gomobile@' + source.gomobileVersion,
    'github.com/sagernet/gomobile/cmd/gobind@' + source.gobindVersion], { env: capacity.env });
  const ndk = (await run([...installer, root])).trim();
  require(statSync(ndk, { throwIfNoEntry: false })?.isDirectory(), 'actual NDK preparation failed');
  const jdk = realpathSync(env.JAVA_HOME_17_X64);
  const paths = { go, gomobile: join(tools, 'gomobile'), gobind: join(tools, 'gobind'), ndk, jdk, sdk,
    work: privateDir(join(root, 'android-work')), cache: privateDir(join(root, 'android-cache')),
    source: join(root, 'fork-source'), consume: privateDir(join(root, 'consume-work')),
    verify: privateDir(join(root, 'verify-work')), carrier: privateDir(join(root, 'final-carrier-work')) };
  await prepare(paths.source, source);
  emitAndroidInputs(env, paths, source.android.outputName);
  console.log('Formal Android source/tool slots prepared without binding the component');
}

if (import.meta.main) {
  try {
    require(process.argv.length === 2, 'usage: android-ci.mjs');
    await prepareAndroid();
  } catch (error) {
    console.error('Formal Android preparation failed: ' + error.message);
    process.exitCode = 1;
  }
}
