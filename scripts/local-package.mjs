#!/usr/bin/env node
// Local acceptance installer for one desktop platform, so a change can be installed on
// test machines without waiting for package.yml. Light build (lto=off, codegen-units=16):
// the output is for acceptance only, never for distribution, and replaces no CI gate.
//   node scripts/local-package.mjs --platform=linux|win   Linux host (win cross-builds via cargo-xwin)
//   node scripts/local-package.mjs --platform=mac-arm64   Apple silicon macOS host
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { delimiter, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { verifyPackagedRelease } from './desktop-core/release-consumer.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const legs = {
  linux: { host: 'linux', conf: 'linux', bundle: 'deb', output: 'deb', helper: 'polaris-helper' },
  win: { host: 'linux', conf: 'windows', bundle: 'nsis', output: 'nsis', helper: 'polaris-helper.exe', target: 'x86_64-pc-windows-msvc' },
  'mac-arm64': { host: 'darwin', conf: 'macos-arm64', bundle: 'app', output: 'macos', helper: 'polaris-helper', target: 'aarch64-apple-darwin' },
};
const platform = /^--platform=(.+)$/.exec(process.argv[2] ?? '')?.[1];
const leg = legs[platform];
if (process.argv.length !== 3 || leg?.host !== process.platform) {
  throw Error('Usage: --platform=linux|win on a Linux host, --platform=mac-arm64 on a macOS host');
}
const env = { ...process.env, CARGO_PROFILE_RELEASE_LTO: 'off', CARGO_PROFILE_RELEASE_CODEGEN_UNITS: '16' };
const run = (command, args) => execFileSync(command, args, { cwd: root, env, stdio: 'inherit' });
const read = (command, args) => execFileSync(command, args, { cwd: root, env, encoding: 'utf8' }).trim();
// package.yml injects one github.sha into helper, cleaner and App alike.
env.POLARIS_BUILD_ID = read('git', ['rev-parse', 'HEAD']);
const protoc = /^bin 目录：(.+)$/m.exec(read('node', ['scripts/fetch-protoc.mjs']))?.[1];
if (!protoc) throw Error('fetch-protoc.mjs did not report its bin directory');
env.PATH = protoc + delimiter + env.PATH;

run('pnpm', ['--dir', 'ui', 'install', '--frozen-lockfile']);
const res = `resources/${platform}`;
// fetch-core spends ten anonymous GitHub API requests (60/hour per IP) even when resources are current,
// so it only runs when the offline receipt and pinned-byte check that identity preparation repeats fails.
try {
  verifyPackagedRelease(root, JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json'), 'utf8')), platform,
    join(root, res, platform === 'win' ? 'sing-box.exe' : 'sing-box'));
} catch { run('node', ['scripts/fetch-core.mjs', `--platform=${platform}`]); }
run('node', ['scripts/fetch-dashboard.mjs']);
mkdirSync(join(root, 'resources/data'), { recursive: true });
// An earlier local package leaves its identity and helpers here; package.yml starts without them.
for (const name of ['core-manifest.json', leg.helper, 'polaris-helper-setup.exe']) rmSync(join(root, res, name), { force: true });
const target = leg.target ? ['--target', leg.target] : [];
const out = leg.target ? `target/${leg.target}/release` : 'target/release';
const cargo = (crate, bin) => run('cargo', [...(platform === 'win' ? ['xwin'] : []), 'build', '--release', '-p', crate, '--bin', bin, ...target]);

cargo('polaris-helper', 'polaris-helper');
copyFileSync(join(root, out, leg.helper), join(root, res, leg.helper));
if (platform === 'mac-arm64') {
  const helper = `${res}/${leg.helper}`;
  try { run('codesign', ['--verify', '--strict', helper]); } catch { run('codesign', ['--force', '--sign', '-', '--timestamp=none', helper]); }
  run('codesign', ['--verify', '--strict', helper]);
}
if (platform === 'win') {
  env.POLARIS_WINDOWS_INSTALL_ANCHOR = read('node', ['scripts/prepare-core-identity.mjs', '--platform=win', '--mode=maintenance-anchor', `--helper-binary=${res}/${leg.helper}`]);
  cargo('polaris-windows-cleaner', 'polaris-helper-setup');
  copyFileSync(join(root, out, 'polaris-helper-setup.exe'), join(root, res, 'polaris-helper-setup.exe'));
}
run('node', ['scripts/prepare-core-identity.mjs', `--platform=${platform}`]);
env.POLARIS_CORE_IDENTITY_FILE = join(root, res, 'core-manifest.json');
const cli = `@tauri-apps/cli@${readFileSync(join(root, 'scripts/tauri-cli.version'), 'utf8').trim()}`;
// cargo-xwin and Tauri's Windows build scripts suppress opposite default UCRT libraries.
// Pass the static archive explicitly and let cargo-xwin resolve it in the selected SDK.
const windowsCargoArgs = platform === 'win'
  ? ['--', '--config', `target.${leg.target}.rustflags=["-C","link-arg=libucrt.lib"]`]
  : [];
run('npm', ['exec', '--yes', `--package=${cli}`, '--', 'tauri', 'build', ...(platform === 'win' ? ['--runner', 'cargo-xwin'] : []), ...target,
  '--config', `src-tauri/tauri.${leg.conf}.conf.json`, '--bundles', leg.bundle, ...windowsCargoArgs]);
run('node', ['scripts/prepare-core-identity.mjs', `--platform=${platform}`, '--mode=verify']);

const bundle = `${out}/bundle/${leg.output}`;
if (platform === 'win') {
  // package.yml PKG-3: every shipped executable links the CRT statically.
  for (const exe of [`${out}/polaris.exe`, `${res}/${leg.helper}`, `${res}/polaris-helper-setup.exe`]) {
    if (/msvcp140|vcruntime140/i.test(readFileSync(join(root, exe), 'latin1'))) throw Error(`${exe} imports the dynamic VC++ runtime`);
  }
}
if (platform === 'mac-arm64') {
  const app = `${bundle}/Polaris.app`;
  run('bash', ['scripts/macos-nested-code.sh', 'seal', app, `${res}/sing-box`]);
  run('codesign', ['--force', '--deep', '--sign', '-', '--timestamp=none', app]);
  run('codesign', ['--verify', '--deep', '--strict', app]);
  run('bash', ['scripts/macos-nested-code.sh', 'verify', app, `${res}/sing-box`]);
}
for (const name of readdirSync(join(root, bundle))) console.log(`${bundle}/${name}`);
