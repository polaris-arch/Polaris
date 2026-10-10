import assert from 'node:assert/strict';
import test from 'node:test';
import { chmodSync, copyFileSync, cpSync, linkSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { execFileSync } from 'node:child_process';
import { dirname, join } from 'node:path';

import { cronetFixture, fixtureManifest } from './lib/cronet-payload.test.mjs';

import { APPIMAGE_CORE_MEMBER, APPIMAGE_HOST_WAYLAND_LIBS, appImageRuntimeViolations, patchGtkHook, restoreCoreSeed, verifyAppImageMembers } from './postprocess-appimage.mjs';

// 判据与修复同源：`appImageRuntimeViolations` 由 postprocess-appimage.mjs 导出、被
// verify-packaging.mjs 的 payload 门 import（`verify-packaging.mjs:71` / `:1454`），即
// **动手的人和判分的人是同一段代码**。谓词写歪一点，重封和门会同时瞎，产物照样出厂。
// 故本文件的承重面是**反向对照**：把 2026-08-24 那个真实坏 AppDir 的形态原样喂进去，
// 断言它必须报满 5 项 —— 谓词退化成恒绿时这里立刻红。
//
// 历史真缺陷（Ubuntu 26.04 / GNOME Wayland，AppImage `45058e3`）：linuxdeploy 随包带入
// 构建机的四个 libwayland-*，与宿主 Mesa/EGL 混用 ⇒ WebKitWebProcess 以 EGL_BAD_PARAMETER
// 连崩（apport 记录 6 次 SIGABRT）；GTK hook 只设 GIO_EXTRA_MODULES ⇒ bundled 旧 GLib 仍
// 装载宿主 GVfs module ⇒ 主进程 undefined symbol 致命退出（2 次 SIGTRAP）。修复为 `e64e6f1`。

const GIO_RELATIVE = 'usr/lib/x86_64-linux-gnu/gio/modules';
const BUNDLED_GIO = `"$APPDIR/${GIO_RELATIVE}"`;

/**
 * 造一个 AppDir 夹具。默认参数**刻意就是坏形态**（四个 wayland 库俱在、hook 没有
 * GIO_MODULE_DIR），即 2026-08-24 真机上那个 AppDir 的结构切片。
 */
function makeAppDir({
  waylandLibs = APPIMAGE_HOST_WAYLAND_LIBS,
  gioExtra = [BUNDLED_GIO],
  gioModuleDir = [],
  hook = true,
  modulesDir = true,
  extraDirs = [],
} = {}) {
  const appDir = mkdtempSync(join(tmpdir(), 'polaris-appdir-'));
  mkdirSync(join(appDir, 'usr', 'lib'), { recursive: true });
  if (modulesDir) mkdirSync(join(appDir, GIO_RELATIVE), { recursive: true });
  for (const relative of extraDirs) mkdirSync(join(appDir, relative), { recursive: true });
  for (const name of waylandLibs) {
    writeFileSync(join(appDir, 'usr', 'lib', name), 'ELF-stub');
  }
  if (hook) {
    mkdirSync(join(appDir, 'apprun-hooks'), { recursive: true });
    const lines = [
      '#! /usr/bin/env bash',
      'export GSETTINGS_SCHEMA_DIR="$APPDIR/usr/share/glib-2.0/schemas"',
      ...gioExtra.map((rhs) => `export GIO_EXTRA_MODULES=${rhs}`),
      ...gioModuleDir.map((rhs) => `export GIO_MODULE_DIR=${rhs}`),
    ];
    writeFileSync(join(appDir, 'apprun-hooks', 'linuxdeploy-plugin-gtk.sh'), `${lines.join('\n')}\n`);
  }
  return appDir;
}

/** 每个用例自带清理：夹具落在 tmpdir，留下来会攒垃圾。 */
function withAppDir(t, options) {
  const appDir = makeAppDir(options);
  t.after(() => rmSync(appDir, { recursive: true, force: true }));
  return appDir;
}

test('反向对照：2026-08-24 那个坏 AppDir 必须报满 5 项（四个 wayland 库 + 缺 GIO_MODULE_DIR）', (t) => {
  const appDir = withAppDir(t);
  const violations = appImageRuntimeViolations(appDir);
  assert.deepEqual(violations, [
    'AppDir 仍捆绑 libwayland-client.so.0（会与新宿主 Mesa/EGL 混用）',
    'AppDir 仍捆绑 libwayland-cursor.so.0（会与新宿主 Mesa/EGL 混用）',
    'AppDir 仍捆绑 libwayland-egl.so.1（会与新宿主 Mesa/EGL 混用）',
    'AppDir 仍捆绑 libwayland-server.so.0（会与新宿主 Mesa/EGL 混用）',
    'GIO_MODULE_DIR export 应恰有 1 条，实为 0 条',
  ]);
});

test('正面断言：后处理修好的 AppDir 违反为空', (t) => {
  const appDir = withAppDir(t, { waylandLibs: [], gioModuleDir: [BUNDLED_GIO] });
  assert.deepEqual(appImageRuntimeViolations(appDir), []);
});

test('四个冲突库逐个独立成条，不是「有没有 wayland」一把梭', (t) => {
  for (const name of APPIMAGE_HOST_WAYLAND_LIBS) {
    const appDir = withAppDir(t, { waylandLibs: [name], gioModuleDir: [BUNDLED_GIO] });
    assert.deepEqual(appImageRuntimeViolations(appDir), [
      `AppDir 仍捆绑 ${name}（会与新宿主 Mesa/EGL 混用）`,
    ]);
  }
});

test('GIO_MODULE_DIR 与 GIO_EXTRA_MODULES 指向不同目录即红', (t) => {
  // 目标目录**造出来**，好让这一格只剩「两侧不一致」这一条违反：一次变异只应点亮一盏灯，
  // 否则「存在性」那条会顺带遮住「一致性」这条是死是活。
  const appDir = withAppDir(t, {
    waylandLibs: [],
    gioModuleDir: ['"$APPDIR/usr/lib/gio/modules"'],
    extraDirs: ['usr/lib/gio/modules'],
  });
  assert.deepEqual(appImageRuntimeViolations(appDir), [
    `GIO_MODULE_DIR 必须与 bundled GIO_EXTRA_MODULES 指向同一目录："$APPDIR/usr/lib/gio/modules" != ${BUNDLED_GIO}`,
  ]);
});

test('GIO_MODULE_DIR 逃出 $APPDIR 即红 —— 那正是加载宿主 GVfs module 的原病理', (t) => {
  const hostPath = '"/usr/lib/x86_64-linux-gnu/gio/modules"';
  const appDir = withAppDir(t, {
    waylandLibs: [],
    gioExtra: [hostPath],
    gioModuleDir: [hostPath],
  });
  assert.deepEqual(appImageRuntimeViolations(appDir), [
    `GIO_MODULE_DIR 必须锚在 $APPDIR 内，实为 ${hostPath}`,
  ]);
});

test('GIO_MODULE_DIR 锚对了但 bundled 目录不存在即红（路径写对≠东西在包里）', (t) => {
  const appDir = withAppDir(t, {
    waylandLibs: [],
    gioModuleDir: [BUNDLED_GIO],
    modulesDir: false,
  });
  assert.deepEqual(appImageRuntimeViolations(appDir), [
    `GIO_MODULE_DIR 指向的 bundled 目录不存在：${join(appDir, GIO_RELATIVE)}`,
  ]);
});

test('重复 export 行不被当成「有一条」放过', (t) => {
  const appDir = withAppDir(t, {
    waylandLibs: [],
    gioModuleDir: [BUNDLED_GIO, BUNDLED_GIO],
  });
  assert.deepEqual(appImageRuntimeViolations(appDir), [
    'GIO_MODULE_DIR export 应恰有 1 条，实为 2 条',
  ]);
});

test('缺 GTK hook 时短路返回，不再声称 GIO 契约成立', (t) => {
  const appDir = withAppDir(t, { waylandLibs: [], hook: false });
  assert.deepEqual(appImageRuntimeViolations(appDir), [
    `缺 GTK AppRun hook：${join(appDir, 'apprun-hooks', 'linuxdeploy-plugin-gtk.sh')}`,
  ]);
});

test('冲突库清单本身是判据的一部分：四项且逐字锁定', () => {
  assert.deepEqual([...APPIMAGE_HOST_WAYLAND_LIBS], [
    'libwayland-client.so.0',
    'libwayland-cursor.so.0',
    'libwayland-egl.so.1',
    'libwayland-server.so.0',
  ]);
});

// Current official Tauri GTK template uses MODULE_DIR directly; it can render a
// doubled slash because gio's giomoduledir is absolute. Both forms stay in AppDir.
for (const rhs of [BUNDLED_GIO, `"$APPDIR//${GIO_RELATIVE}"`]) {
  test(`module-only upstream hook is valid and remains unchanged: ${rhs}`, (t) => {
    const appDir = withAppDir(t, { waylandLibs: [], gioExtra: [], gioModuleDir: [rhs] });
    const hook = join(appDir, 'apprun-hooks', 'linuxdeploy-plugin-gtk.sh');
    const before = readFileSync(hook, 'utf8');
    assert.deepEqual(appImageRuntimeViolations(appDir), []);
    assert.equal(patchGtkHook(appDir), false);
    assert.equal(readFileSync(hook, 'utf8'), before);
  });
}

test('legacy extra-only hook gains MODULE_DIR exactly once and is idempotent', (t) => {
  const appDir = withAppDir(t, { waylandLibs: [] });
  assert.equal(patchGtkHook(appDir), true);
  assert.deepEqual(appImageRuntimeViolations(appDir), []);
  const hook = join(appDir, 'apprun-hooks', 'linuxdeploy-plugin-gtk.sh');
  const patched = readFileSync(hook, 'utf8');
  assert.equal(patchGtkHook(appDir), false);
  assert.equal(readFileSync(hook, 'utf8'), patched);
  assert.equal(patched.split('export GIO_MODULE_DIR=').length - 1, 1);
});

for (const [label, options] of [
  ['missing both declarations', { gioExtra: [], gioModuleDir: [] }],
  ['duplicate extra', { gioExtra: [BUNDLED_GIO, BUNDLED_GIO], gioModuleDir: [BUNDLED_GIO] }],
  ['duplicate module dir', { gioExtra: [], gioModuleDir: [BUNDLED_GIO, BUNDLED_GIO] }],
  ['module-only host path', { gioExtra: [], gioModuleDir: ['"/usr/lib/gio/modules"'] }],
  ['module-only absent directory', { gioExtra: [], gioModuleDir: [BUNDLED_GIO], modulesDir: false }],
  ['legacy absent directory', { modulesDir: false }],
  ['inconsistent declarations', { gioModuleDir: ['"$APPDIR/usr/lib/gio/modules"'], extraDirs: ['usr/lib/gio/modules'] }],
]) {
  test(`refuses unsafe GTK hook without changing it: ${label}`, (t) => {
    const appDir = withAppDir(t, { waylandLibs: [], ...options });
    const hook = join(appDir, 'apprun-hooks', 'linuxdeploy-plugin-gtk.sh');
    const before = readFileSync(hook, 'utf8');
    assert.notDeepEqual(appImageRuntimeViolations(appDir), []);
    assert.throws(() => patchGtkHook(appDir), /无法安全修补 GTK hook/);
    assert.equal(readFileSync(hook, 'utf8'), before);
  });
}

test('dot-dot and symlink paths cannot escape AppDir even if the outside directory exists', (t) => {
  const outside = mkdtempSync(join(tmpdir(), 'polaris-host-gio-'));
  t.after(() => rmSync(outside, { recursive: true, force: true }));
  for (const mode of ['dot-dot', 'symlink']) {
    const appDir = withAppDir(t, { waylandLibs: [], gioExtra: [], gioModuleDir: [], modulesDir: false });
    const rhs = mode === 'dot-dot'
      ? `"$APPDIR/../${outside.split('/').at(-1)}"`
      : '"$APPDIR/usr/lib/gio/modules"';
    if (mode === 'symlink') {
      mkdirSync(join(appDir, 'usr/lib/gio'), { recursive: true });
      symlinkSync(outside, join(appDir, 'usr/lib/gio/modules'));
    }
    const hook = join(appDir, 'apprun-hooks', 'linuxdeploy-plugin-gtk.sh');
    writeFileSync(hook, `export GIO_MODULE_DIR=${rhs}\n`);
    assert.deepEqual(appImageRuntimeViolations(appDir), [
      `GIO_MODULE_DIR 必须锚在 $APPDIR 内，实为 ${rhs}`,
    ]);
    assert.throws(() => patchGtkHook(appDir), /必须锚在 \$APPDIR 内/);
  }
});

test('CLI repairs both hook layouts and reaches repack only after the runtime contract passes', (t) => {
  for (const layout of ['legacy', 'module-only']) {
    const root = mkdtempSync(join(tmpdir(), 'polaris-repack-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const original = makeAppDir(layout === 'legacy' ? {} : {
      gioExtra: [], gioModuleDir: [`"$APPDIR//${GIO_RELATIVE}"`],
    });
    const appDir = join(root, 'Polaris.AppDir');
    renameSync(original, appDir);
    const artifact = join(root, 'Polaris.AppImage');
    writeFileSync(artifact, 'unprocessed');
    seedCoreFixture(root, appDir);
    mkdirSync(join(root, 'scripts'));
    const script = join(root, 'scripts/postprocess-appimage.mjs');
    copyFileSync(new URL('./postprocess-appimage.mjs', import.meta.url), script);
    cpSync(new URL('./lib', import.meta.url), join(root, 'scripts/lib'), { recursive: true });
    // The plugin is a fixture, but its output is a real SquashFS behind a dummy
    // runtime header. The verifier never executes that header or the core.
    const tool = join(root, 'fixture-output-plugin');
    writeFileSync(tool, fixtureOutputPlugin());
    chmodSync(tool, 0o700);
    execFileSync(process.execPath, [script, '--root', root, '--tool', tool, '--arch', 'x86_64'], {
      stdio: 'pipe',
    });
    assert.deepEqual(appImageRuntimeViolations(appDir), []);
    assert.ok(statSync(artifact).size > 1024 * 1024);
    assert.equal(readFileSync(artifact)[0], 7);
    assert.equal(readFileSync(join(appDir, APPIMAGE_CORE_MEMBER), 'utf8'), 'verified-core-seed');
  }
});

function seedCoreFixture(root, appDir) {
  const source = join(root, 'resources/linux/sing-box');
  const target = join(appDir, APPIMAGE_CORE_MEMBER);
  mkdirSync(dirname(source), { recursive: true });
  mkdirSync(dirname(target), { recursive: true });
  writeFileSync(source, 'verified-core-seed', { mode: 0o755 });
  writeFileSync(target, 'linuxdeploy-changed-core', { mode: 0o700 });
  const cronet = cronetFixture('linux');
  writeFileSync(join(dirname(source), 'libcronet.so'), cronet, { mode: 0o755 });
  writeFileSync(join(dirname(target), 'libcronet.so'), cronet, { mode: 0o755 });
  mkdirSync(join(root, 'src-tauri'), { recursive: true });
  writeFileSync(join(root, 'src-tauri/core-manifest.json'), JSON.stringify(fixtureManifest(cronet)));
  return { source, target };
}

function fixtureOutputPlugin(tamper = false) {
  return `#!/usr/bin/env node
const fs = require('node:fs');
const cp = require('node:child_process');
const dir = process.argv[2].slice('--appdir='.length);
${tamper === 'cronet' ? `const target = require('node:path').join(dir, ${JSON.stringify(dirname(APPIMAGE_CORE_MEMBER) + '/libcronet.so')}); const bytes = fs.readFileSync(target); bytes[0x210] ^= 1; fs.writeFileSync(target, bytes);` : tamper ? `fs.writeFileSync(require('node:path').join(dir, ${JSON.stringify(APPIMAGE_CORE_MEMBER)}), 'untrusted-core-xxx');` : ''}
const squash = process.env.LDAI_OUTPUT + '.squashfs';
cp.execFileSync('mksquashfs', [dir, squash, '-noappend', '-processors', '1', '-no-progress'], {stdio:'ignore'});
fs.writeFileSync(process.env.LDAI_OUTPUT, Buffer.concat([Buffer.alloc(1024*1024, 7), fs.readFileSync(squash)]));
fs.unlinkSync(squash);
`;
}

function coreFixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'polaris-core-seed-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const appDir = join(root, 'Polaris.AppDir');
  mkdirSync(appDir);
  return { root, appDir, ...seedCoreFixture(root, appDir) };
}

function packFixture(root, appDir, { falseMagic = false, duplicateFilesystem = false } = {}) {
  const squash = join(root, 'payload.squashfs');
  execFileSync('mksquashfs', [appDir, squash, '-noappend', '-processors', '1', '-no-progress'], { stdio: 'ignore' });
  const header = Buffer.alloc(1024, 7);
  if (falseMagic) header.write('hsqs', 12);
  const bytes = readFileSync(squash);
  const artifact = join(root, 'fixture.AppImage');
  writeFileSync(artifact, Buffer.concat([header, bytes, ...(duplicateFilesystem ? [bytes] : [])]));
  return artifact;
}

test('restores seed SHA/mode atomically without modifying a linked former target', (t) => {
  const { root, appDir, source, target } = coreFixture(t);
  const linked = join(root, 'previous-core-hardlink');
  linkSync(target, linked);
  const expected = restoreCoreSeed(appDir, root);
  assert.deepEqual(readFileSync(target), readFileSync(source));
  assert.equal(lstatSync(target).mode & 0o7777, 0o755);
  assert.equal(readFileSync(linked, 'utf8'), 'linuxdeploy-changed-core');
  verifyAppImageMembers(packFixture(root, appDir, { falseMagic: true }), expected);
});

for (const fault of ['source symlink', 'target symlink', 'parent symlink', 'duplicate core', 'nonexecutable source', 'missing Cronet']) {
  test(`refuses unsafe core seed layout: ${fault}`, (t) => {
    const { root, appDir, source, target } = coreFixture(t);
    if (fault === 'source symlink' || fault === 'target symlink') {
      const path = fault === 'source symlink' ? source : target;
      renameSync(path, `${path}.original`);
      symlinkSync(`${path}.original`, path);
    } else if (fault === 'parent symlink') {
      renameSync(dirname(target), `${dirname(target)}.original`);
      symlinkSync(`${dirname(target)}.original`, dirname(target));
    } else if (fault === 'duplicate core') {
      writeFileSync(join(appDir, 'sing-box'), 'unexpected-core');
    } else if (fault === 'nonexecutable source') {
      chmodSync(source, 0o644);
    } else {
      rmSync(join(dirname(target), 'libcronet.so'));
    }
    assert.throws(() => restoreCoreSeed(appDir, root));
  });
}

for (const fault of ['same-size wrong SHA', 'wrong mode', 'core symlink', 'missing member', 'multiple filesystems', 'missing tool']) {
  test(`actual SquashFS member gate rejects ${fault}`, (t) => {
    const { root, appDir, target } = coreFixture(t);
    const expected = restoreCoreSeed(appDir, root);
    if (fault === 'same-size wrong SHA') writeFileSync(target, 'untrusted-core-xxx');
    if (fault === 'wrong mode') chmodSync(target, 0o700);
    if (fault === 'core symlink') {
      renameSync(target, `${target}.original`);
      symlinkSync('sing-box.original', target);
    }
    if (fault === 'missing member') rmSync(target);
    const artifact = packFixture(root, appDir, { duplicateFilesystem: fault === 'multiple filesystems' });
    assert.throws(() => verifyAppImageMembers(artifact, expected, fault === 'missing tool' ? join(root, 'absent-unsquashfs') : 'unsquashfs'));
  });
}

for (const component of ['core', 'cronet']) test(`CLI refuses a repacker changing ${component} bytes and leaves the previous artifact intact`, (t) => {
  const { root, appDir } = coreFixture(t);
  const graphical = makeAppDir({ waylandLibs: [], gioExtra: [], gioModuleDir: [BUNDLED_GIO] });
  t.after(() => rmSync(graphical, { recursive: true, force: true }));
  renameSync(join(graphical, 'apprun-hooks'), join(appDir, 'apprun-hooks'));
  mkdirSync(join(appDir, GIO_RELATIVE), { recursive: true });
  mkdirSync(join(root, 'scripts'));
  const script = join(root, 'scripts/postprocess-appimage.mjs');
  copyFileSync(new URL('./postprocess-appimage.mjs', import.meta.url), script);
  cpSync(new URL('./lib', import.meta.url), join(root, 'scripts/lib'), { recursive: true });
  const artifact = join(root, 'Polaris.AppImage');
  writeFileSync(artifact, 'previous-artifact');
  const tool = join(root, 'fixture-output-plugin');
  writeFileSync(tool, fixtureOutputPlugin(component === 'cronet' ? 'cronet' : true), { mode: 0o700 });
  assert.throws(() => execFileSync(process.execPath, [script, '--root', root, '--tool', tool, '--arch', 'x86_64'], { stdio: 'pipe' }), /AppImage 实际成员 SHA\/权限不符/);
  assert.equal(readFileSync(artifact, 'utf8'), 'previous-artifact');
});

function linuxPayloadFixture(t) {
  const { root, appDir } = coreFixture(t);
  restoreCoreSeed(appDir, root);
  writeFileSync(join(root, 'resources/linux/polaris-helper'), 'helper');
  // The canonical structured Cronet seed and pin were installed by coreFixture.
  copyFileSync(join(root, 'resources/linux/polaris-helper'), join(dirname(join(appDir, APPIMAGE_CORE_MEMBER)), 'polaris-helper'));
  const graphical = makeAppDir({ waylandLibs: [], gioExtra: [], gioModuleDir: [BUNDLED_GIO] });
  t.after(() => rmSync(graphical, { recursive: true, force: true }));
  renameSync(join(graphical, 'apprun-hooks'), join(appDir, 'apprun-hooks'));
  mkdirSync(join(appDir, GIO_RELATIVE), { recursive: true });
  mkdirSync(join(root, 'resources/data'));
  mkdirSync(join(appDir, 'usr/lib/Polaris/_up_/resources/data'));
  mkdirSync(join(appDir, 'usr/lib/Polaris/_up_/resources/dashboard'));
  writeFileSync(join(appDir, 'usr/lib/Polaris/_up_/resources/dashboard/index.html'), 'fixture dashboard');
  const bundle = join(root, 'bundle');
  mkdirSync(join(bundle, 'appimage'), { recursive: true });
  mkdirSync(join(bundle, 'deb'), { recursive: true });
  cpSync(appDir, join(bundle, 'deb/package/data'), { recursive: true });
  const packedDir = join(bundle, 'appimage/Polaris.AppDir');
  renameSync(appDir, packedDir);
  mkdirSync(join(root, 'scripts'));
  mkdirSync(join(root, 'src-tauri'), { recursive: true });
  for (const name of ['verify-packaging.mjs', 'postprocess-appimage.mjs', 'release-assets.mjs']) {
    copyFileSync(new URL(`./${name}`, import.meta.url), join(root, 'scripts', name));
  }
  cpSync(new URL('./desktop-core', import.meta.url), join(root, 'scripts/desktop-core'), { recursive: true });
  // verify-packaging.mjs 静态 import `./lib/` 下的判据模块；只拷入口不拷它们，CLI 在模块解析期就退出。
  cpSync(new URL('./lib', import.meta.url), join(root, 'scripts/lib'), { recursive: true });
  writeFileSync(join(root, 'src-tauri/core-manifest.json'), JSON.stringify({ ...fixtureManifest(cronetFixture('linux')), coreArchiveSha256: { linux: 'fixture' } }));
  writeFileSync(join(root, 'src-tauri/tauri.conf.json'), JSON.stringify({ productName: 'Polaris' }));
  const args = [join(root, 'scripts/verify-packaging.mjs'), 'payload', '--label', 'linux', '--root', bundle];
  return { root, args, target: join(packedDir, APPIMAGE_CORE_MEMBER) };
}

test('Linux payload CLI rejects same-size substituted core bytes in a previously passing bundle', (t) => {
  const { args, target } = linuxPayloadFixture(t);
  execFileSync(process.execPath, args, { stdio: 'pipe' });
  const beforeSize = statSync(target).size;
  writeFileSync(target, 'untrusted-core-xxx');
  assert.equal(statSync(target).size, beforeSize);
  assert.throws(() => execFileSync(process.execPath, args, { stdio: 'pipe' }), /Linux 随包原核 SHA-256\/权限与源不符/);
});

for (const fault of ['same-size substitution', 'different size same GNU Build ID', 'wrong source SHA', 'symlink']) {
  test(`actual Linux AppImage payload CLI rejects Cronet ${fault}`, (t) => {
    const { root, args, target } = linuxPayloadFixture(t);
    execFileSync(process.execPath, args, { stdio: 'pipe' });
    const cronet = join(dirname(target), 'libcronet.so'), source = join(root, 'resources/linux/libcronet.so');
    const bytes = readFileSync(cronet), changed = Buffer.from(bytes); changed[0x210] ^= 1;
    if (fault === 'same-size substitution') { writeFileSync(cronet, changed); assert.equal(statSync(cronet).size, bytes.length); }
    if (fault === 'different size same GNU Build ID') {
      const id = execFileSync('readelf', ['-n', source], { encoding: 'utf8' }).match(/Build ID: ([a-f0-9]+)/)?.[1];
      assert.ok(id); writeFileSync(cronet, Buffer.concat([changed, Buffer.from('appended mutation')]));
      assert.equal(execFileSync('readelf', ['-n', cronet], { encoding: 'utf8' }).match(/Build ID: ([a-f0-9]+)/)?.[1], id);
      assert.notEqual(statSync(cronet).size, bytes.length);
    }
    if (fault === 'wrong source SHA') { writeFileSync(source, changed); writeFileSync(cronet, changed); }
    if (fault === 'symlink') { rmSync(cronet); symlinkSync(source, cronet); }
    assert.throws(() => execFileSync(process.execPath, args, { stdio: 'pipe' }), /Cronet source\/final content or static ABI mismatch/);
  });
}

test('AppImage restores Cronet from the pinned canonical seed, never trusts linuxdeploy output', (t) => {
  const { root, appDir, source } = coreFixture(t);
  const target = join(appDir, dirname(APPIMAGE_CORE_MEMBER), 'libcronet.so');
  const canonical = readFileSync(join(dirname(source), 'libcronet.so'));
  const changed = Buffer.from(canonical); changed[0x210] ^= 1;
  writeFileSync(target, changed);
  const linked = join(root, 'previous-cronet-hardlink'); linkSync(target, linked);
  const expected = restoreCoreSeed(appDir, root);
  assert.deepEqual(readFileSync(target), canonical);
  assert.deepEqual(readFileSync(linked), changed);
  const hash = expected[dirname(APPIMAGE_CORE_MEMBER) + '/libcronet.so'].sha256;
  assert.equal(hash, fixtureManifest(canonical).cronetLibrarySha256.linux);
  verifyAppImageMembers(packFixture(root, appDir), expected);
  writeFileSync(target, changed);
  assert.throws(() => verifyAppImageMembers(packFixture(root, appDir), expected), /SHA\/权限不符/);
});

for (const fault of ['wrong source SHA', 'missing pin', 'source Cronet symlink', 'target Cronet symlink', 'duplicate Cronet']) {
  test(`AppImage source Cronet authority fails closed: ${fault}`, (t) => {
    const { root, appDir, source } = coreFixture(t);
    const originalCore = readFileSync(join(appDir, APPIMAGE_CORE_MEMBER));
    const cronetSource = join(dirname(source), 'libcronet.so');
    const cronetTarget = join(appDir, dirname(APPIMAGE_CORE_MEMBER), 'libcronet.so');
    if (fault === 'wrong source SHA') { const bytes = readFileSync(cronetSource); bytes[0x210] ^= 1; writeFileSync(cronetSource, bytes); }
    if (fault === 'missing pin') writeFileSync(join(root, 'src-tauri/core-manifest.json'), '{}');
    if (fault.endsWith('symlink')) { const path = fault.startsWith('source') ? cronetSource : cronetTarget; renameSync(path, path + '.real'); symlinkSync(path + '.real', path); }
    if (fault === 'duplicate Cronet') writeFileSync(join(appDir, 'libcronet.so'), 'unexpected-cronet');
    assert.throws(() => restoreCoreSeed(appDir, root));
    assert.deepEqual(readFileSync(join(appDir, APPIMAGE_CORE_MEMBER)), originalCore, 'validate both inputs before replacing either');
  });
}
