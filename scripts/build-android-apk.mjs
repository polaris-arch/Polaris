#!/usr/bin/env node
// Standard Android build entry: binds every Android build tool to the pinned Rust NDK, requires the
// pinned global Tauri CLI, and runs one `tauri android build` with the caller's arguments.
//   node scripts/build-android-apk.mjs --apk --split-per-abi --target aarch64 --ci
import { spawnSync } from 'node:child_process';
import { readFileSync, realpathSync, statSync } from 'node:fs';
import { constants, homedir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = realpathSync(resolve(dirname(fileURLToPath(import.meta.url)), '..'));
const pinned = (name) => readFileSync(join(root, 'scripts', name), 'utf8').replace(/\n+$/, '');
const fail = (...lines) => {
  for (const line of lines) console.error(line);
  process.exit(1);
};
const env = process.env;
const sdk = env.ANDROID_HOME || env.ANDROID_SDK_ROOT || join(homedir(), 'Android/Sdk');
const rustNdkVersion = pinned('android-rust-ndk.version');

// Tauri selects the compiler from NDK_HOME; btls-sys selects its CMake
// toolchain from ANDROID_NDK_HOME. A split pair mixes incompatible NDKs.
let selected = env.ANDROID_NDK_HOME || env.NDK_HOME || join(sdk, 'ndk', rustNdkVersion);
if (!['source.properties', 'build/cmake/android.toolchain.cmake']
  .every((file) => statSync(join(selected, file), { throwIfNoEntry: false })?.isFile())) {
  fail(`Rust NDK is missing or incomplete: ${selected}`,
    `Install ndk;${rustNdkVersion} with sdkmanager, or point both NDK variables to an installed copy of that version`);
}
selected = realpathSync(selected);
for (const name of ['ANDROID_NDK_HOME', 'NDK_HOME', 'ANDROID_NDK_ROOT']) {
  const value = env[name];
  if (value && (!statSync(value, { throwIfNoEntry: false })?.isDirectory() || realpathSync(value) !== selected)) {
    fail(`${name} conflicts with the selected Rust NDK; use one NDK for all Android build tools`);
  }
}
const revision = /^Pkg.Revision[^\S\n]*=[^\S\n]*([^\n]*)/m
  .exec(readFileSync(join(selected, 'source.properties'), 'utf8'))?.[1];
if (revision !== rustNdkVersion) {
  fail(`Rust NDK Pkg.Revision must be ${rustNdkVersion} (found: ${revision || 'missing'})`);
}
Object.assign(env, { ANDROID_NDK_HOME: selected, NDK_HOME: selected, ANDROID_NDK_ROOT: selected });

const cliVersion = pinned('tauri-cli.version');
const found = spawnSync('which', ['tauri'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] });
if (found.status !== 0) fail(`Global Tauri CLI is missing; run npm install -g @tauri-apps/cli@${cliVersion}`);
const located = found.stdout.replace(/\n+$/, '');
const cli = join(realpathSync(dirname(located)), basename(located));
const reported = spawnSync(cli, ['--version'], { encoding: 'utf8', stdio: ['inherit', 'pipe', 'inherit'] });
if (reported.stdout?.replace(/\n+$/, '') !== `tauri-cli ${cliVersion}`) {
  fail(`Global Tauri CLI must be ${cliVersion}; run npm install -g @tauri-apps/cli@${cliVersion}`);
}
env.POLARIS_TAURI_CLI = cli;

// Bind the default mobile config at the shared entry point, including calls outside CI.
const args = process.argv.slice(2);
const end = args.indexOf('--');
const explicitConfig = (end < 0 ? args : args.slice(0, end))
  .some((arg) => arg.startsWith('-c') || arg === '--config' || arg.startsWith('--config='));
if (!explicitConfig) args.unshift('--config', join(root, 'src-tauri/tauri.android.conf.json'));
const build = spawnSync(cli, ['android', 'build', ...args], { cwd: root, env, stdio: 'inherit' });
if (build.error) throw build.error;
// A signal-terminated build reports the shell's 128+N status, as the former `exec` did to bash callers.
process.exit(build.status ?? 128 + constants.signals[build.signal]);
