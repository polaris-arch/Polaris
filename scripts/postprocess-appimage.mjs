#!/usr/bin/env node
/**
 * 修正 Tauri 2.11 AppImage 图形栈兼容性、恢复已验证核心种子的原字节，再用官方输出插件重封。
 *
 * 根因（Ubuntu 26.04 + Mesa 25 真机复现）：linuxdeploy 把构建机的 libwayland-* 一并塞进 AppImage，
 * 运行时却加载宿主 Mesa/EGL；新 Mesa 与旧 Wayland ABI 混用后 WebKitWebProcess 以 EGL_BAD_PARAMETER
 * 退出。与此同时 GTK hook 只设置 GIO_EXTRA_MODULES，GLib 仍会加载宿主 GVfs module，旧 bundled
 * GLib 遇到新 GVfs 符号会报 undefined symbol。
 *
 * 最小修复：只让四个 Wayland 基础库回到宿主版本，并把 GIO module 搜索根锁在 AppDir 内。
 * bundled WebKitGTK/GTK/GLib 其余部分全部保留；不把 AppImage 退化成依赖宿主 WebKitGTK 的第二份 deb。
 * linuxdeploy 加工后的核心可能既改变字节又没有 GNU Build ID；重封前恢复 fetch-core 原种子，
 * 重封后仅用宿主 unsquashfs 读取实际核心/Cronet 成员并校验 SHA/权限，不执行 AppImage 或内核。
 *
 * 用法：
 *   node scripts/postprocess-appimage.mjs \
 *     --root target/release/bundle/appimage \
 *     --tool "$HOME/.cache/tauri/linuxdeploy-plugin-appimage.AppImage" \
 *     --arch x86_64
 */

import {
  chmodSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from 'fs';
import { spawnSync } from 'child_process';
import { createHash } from 'crypto';
import { tmpdir } from 'os';
import { basename, dirname, join, relative, resolve, sep } from 'path';
import { fileURLToPath } from 'url';
import { verifyCronetPayload } from './lib/cronet-payload.mjs';

export const APPIMAGE_HOST_WAYLAND_LIBS = Object.freeze([
  'libwayland-client.so.0',
  'libwayland-cursor.so.0',
  'libwayland-egl.so.1',
  'libwayland-server.so.0',
]);

const GIO_EXTRA_PREFIX = 'export GIO_EXTRA_MODULES=';
const GIO_DIR_PREFIX = 'export GIO_MODULE_DIR=';
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const APPIMAGE_CORE_MEMBER = 'usr/lib/Polaris/_up_/resources/linux/sing-box';
const CRONET_MEMBER = 'usr/lib/Polaris/_up_/resources/linux/libcronet.so';

function regularMember(root, member) {
  let path = resolve(root);
  if (!lstatSync(path).isDirectory()) throw new Error(`拒绝非目录或软链根：${path}`);
  const parts = member.split('/');
  if (parts.some((part) => !part || part === '.' || part === '..')) throw new Error(`非法成员路径：${member}`);
  for (const [index, part] of parts.entries()) {
    path = join(path, part);
    const stat = lstatSync(path);
    if (index < parts.length - 1 ? !stat.isDirectory() : !stat.isFile()) {
      throw new Error(`拒绝非普通文件/目录或软链成员：${path}`);
    }
  }
  return path;
}

function identity(path) {
  return { sha256: createHash('sha256').update(readFileSync(path)).digest('hex'), mode: lstatSync(path).mode & 0o7777 };
}

// Restore both canonical seeds before repacking. Cronet's fixed source library
// has only system NEEDED entries and no RPATH/RUNPATH; no transform is allowed.
// Validate all inputs before replacement and bind final SquashFS bytes to these
// source identities, never to a processed AppDir or an unchanged GNU Build ID.
export function restoreCoreSeed(appDir, sourceRoot = ROOT) {
  const source = regularMember(sourceRoot, 'resources/linux/sing-box');
  const target = regularMember(appDir, APPIMAGE_CORE_MEMBER);
  const cronetSource = regularMember(sourceRoot, 'resources/linux/libcronet.so');
  const cronetTarget = regularMember(appDir, CRONET_MEMBER);
  const manifest = JSON.parse(readFileSync(regularMember(sourceRoot, 'src-tauri/core-manifest.json'), 'utf8'));
  const verifiedCronet = verifyCronetPayload(cronetSource, cronetSource, 'linux', manifest);
  const matches = new Map([['sing-box', []], ['libcronet.so', []]]);
  const visit = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (matches.has(entry.name)) matches.get(entry.name).push(path);
      if (entry.isDirectory()) visit(path);
    }
  };
  visit(appDir);
  for (const [name, wanted] of [['sing-box', target], ['libcronet.so', cronetTarget]]) {
    const found = matches.get(name);
    if (found.length !== 1 || found[0] !== wanted) throw new Error(`AppDir ${name} 成员必须唯一且位于 ${wanted}`);
  }
  const expected = identity(source);
  if ((expected.mode & 0o111) === 0) throw new Error('源内核不可执行');
  const cronetIdentity = { sha256: verifiedCronet.sha256, mode: verifiedCronet.mode };
  const seeds = [[source, target, expected], [cronetSource, cronetTarget, cronetIdentity]];
  const staging = mkdtempSync(join(dirname(target), '.core-seed-'));
  try {
    for (const [input, output, wanted] of seeds) {
      const copy = join(staging, basename(output));
      copyFileSync(input, copy); chmodSync(copy, wanted.mode);
      if (JSON.stringify(identity(copy)) !== JSON.stringify(wanted)) throw new Error('源种子副本 SHA/权限不符');
    }
    for (const [, output, wanted] of seeds) {
      renameSync(join(staging, basename(output)), output); // no write through former hardlinks
      if (JSON.stringify(identity(output)) !== JSON.stringify(wanted)) throw new Error('AppDir 种子 SHA/权限不符');
    }
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
  verifyCronetPayload(cronetSource, cronetTarget, 'linux', manifest);
  return { [APPIMAGE_CORE_MEMBER]: expected, [CRONET_MEMBER]: cronetIdentity };
}

// Read SquashFS with a host tool, never execute the AppImage runtime. A magic
// occurrence alone is not proof: every candidate is validated, and exactly one
// valid filesystem must exist before extracting the exact non-wildcard members.
export function verifyAppImageMembers(artifact, expected, unsquashfs = 'unsquashfs') {
  if (!lstatSync(artifact).isFile()) throw new Error('AppImage 必须是普通文件，不能为软链');
  const bytes = readFileSync(artifact);
  const offsets = [];
  for (let offset = bytes.indexOf('hsqs'); offset !== -1; offset = bytes.indexOf('hsqs', offset + 4)) {
    const probe = spawnSync(unsquashfs, ['-s', '-offset', String(offset), artifact], { encoding: 'utf8' });
    if (probe.error) throw probe.error;
    if (probe.status === 0) offsets.push(offset);
  }
  if (offsets.length !== 1) throw new Error(`AppImage 应恰有一个有效 SquashFS，实为 ${offsets.length}`);
  const work = mkdtempSync(join(tmpdir(), 'polaris-appimage-payload-'));
  try {
    const extracted = join(work, 'payload');
    const run = spawnSync(unsquashfs, [
      '-no-progress', '-processors', '1', '-no-wildcards', '-offset', String(offsets[0]),
      '-d', extracted, artifact, ...Object.keys(expected),
    ], { encoding: 'utf8', maxBuffer: 1024 * 1024 });
    if (run.error) throw run.error;
    if (run.status !== 0) throw new Error(`AppImage 精确成员提取失败：${run.stderr.trim()}`);
    for (const [member, wanted] of Object.entries(expected)) {
      const actual = identity(regularMember(extracted, member));
      if (actual.sha256 !== wanted.sha256 || actual.mode !== wanted.mode) {
        throw new Error(`AppImage 实际成员 SHA/权限不符：${member}`);
      }
    }
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

function argOf(flag) {
  const index = process.argv.indexOf(flag);
  return index >= 0 && index + 1 < process.argv.length ? process.argv[index + 1] : null;
}

function oneEntry(root, suffix, kind) {
  const matches = readdirSync(root, { withFileTypes: true }).filter((entry) => {
    if (!entry.name.endsWith(suffix)) return false;
    return kind === 'dir' ? entry.isDirectory() : entry.isFile();
  });
  if (matches.length !== 1) {
    throw new Error(`${root} 中应恰有 1 个 ${suffix} ${kind === 'dir' ? '目录' : '文件'}，实为 ${matches.length} 个：${matches.map((e) => e.name).join(', ')}`);
  }
  return join(root, matches[0].name);
}

function exportLines(source, prefix) {
  return source
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.startsWith(prefix));
}

/**
 * 返回 AppDir 运行时兼容契约的全部违反；后处理脚本与 verify-packaging 共用，避免两份判据漂移。
 */
export function appImageRuntimeViolations(appDir) {
  const violations = [];
  const libDir = join(appDir, 'usr', 'lib');
  for (const name of APPIMAGE_HOST_WAYLAND_LIBS) {
    if (existsSync(join(libDir, name))) {
      violations.push(`AppDir 仍捆绑 ${name}（会与新宿主 Mesa/EGL 混用）`);
    }
  }

  const hook = join(appDir, 'apprun-hooks', 'linuxdeploy-plugin-gtk.sh');
  if (!existsSync(hook)) {
    violations.push(`缺 GTK AppRun hook：${hook}`);
    return violations;
  }
  return violations.concat(gtkHookViolations(appDir, readFileSync(hook, 'utf8')));
}

// Tauri's current GTK hook sets MODULE_DIR directly. Legacy hooks only set EXTRA;
// once upgraded, EXTRA remains optional but must agree if present.
function gtkHookViolations(appDir, source) {
  const violations = [];
  const extra = exportLines(source, GIO_EXTRA_PREFIX);
  const moduleDir = exportLines(source, GIO_DIR_PREFIX);
  if (extra.length > 1) violations.push(`GIO_EXTRA_MODULES export 应至多有 1 条，实为 ${extra.length} 条`);
  if (moduleDir.length !== 1) violations.push(`GIO_MODULE_DIR export 应恰有 1 条，实为 ${moduleDir.length} 条`);
  if (moduleDir.length === 1) {
    const moduleRhs = moduleDir[0].slice(GIO_DIR_PREFIX.length);
    if (extra.length === 1) {
      const extraRhs = extra[0].slice(GIO_EXTRA_PREFIX.length);
      if (extraRhs !== moduleRhs) {
        violations.push(`GIO_MODULE_DIR 必须与 bundled GIO_EXTRA_MODULES 指向同一目录：${moduleRhs} != ${extraRhs}`);
      }
    }
    const unquoted = moduleRhs.replace(/^(["'])(.*)\1$/, '$2');
    if (!unquoted.startsWith('$APPDIR/')) {
      violations.push(`GIO_MODULE_DIR 必须锚在 $APPDIR 内，实为 ${moduleRhs}`);
    } else {
      const resolved = resolve(join(appDir, unquoted.slice('$APPDIR/'.length)));
      const inside = (root, path) => {
        const rel = relative(root, path);
        return rel !== '..' && !rel.startsWith(`..${sep}`) && !rel.startsWith(sep);
      };
      if (!inside(resolve(appDir), resolved)) {
        violations.push(`GIO_MODULE_DIR 必须锚在 $APPDIR 内，实为 ${moduleRhs}`);
      } else if (!existsSync(resolved) || !statSync(resolved).isDirectory()) {
        violations.push(`GIO_MODULE_DIR 指向的 bundled 目录不存在：${resolved}`);
      } else if (!inside(realpathSync(appDir), realpathSync(resolved))) {
        violations.push(`GIO_MODULE_DIR 必须锚在 $APPDIR 内，实为 ${moduleRhs}`);
      }
    }
  }
  return violations;
}

export function patchGtkHook(appDir) {
  const hook = join(appDir, 'apprun-hooks', 'linuxdeploy-plugin-gtk.sh');
  if (!existsSync(hook)) throw new Error(`缺 GTK AppRun hook：${hook}`);
  const source = readFileSync(hook, 'utf8');
  const extra = exportLines(source, GIO_EXTRA_PREFIX);
  const current = exportLines(source, GIO_DIR_PREFIX);
  if (current.length === 1) {
    const violations = gtkHookViolations(appDir, source);
    if (violations.length > 0) throw new Error(`无法安全修补 GTK hook：${violations.join(' | ')}`);
    return false;
  }
  if (current.length !== 0 || extra.length !== 1) {
    throw new Error(`无法安全修补 GTK hook：GIO_MODULE_DIR export ${current.length} 条，legacy GIO_EXTRA_MODULES export ${extra.length} 条（需唯一可修补声明）`);
  }
  const desired = `${GIO_DIR_PREFIX}${extra[0].slice(GIO_EXTRA_PREFIX.length)}`;
  const marker = extra[0];
  const occurrences = source.split(marker).length - 1;
  if (occurrences !== 1) {
    throw new Error(`无法安全修补 GTK hook：目标行出现 ${occurrences} 次（期望 1）`);
  }
  const patched = source.replace(marker, `${marker}\n${desired}`);
  const violations = gtkHookViolations(appDir, patched);
  if (violations.length > 0) throw new Error(`无法安全修补 GTK hook：${violations.join(' | ')}`);
  writeFileSync(hook, patched);
  return true;
}

function main() {
  const rootArg = argOf('--root');
  const toolArg = argOf('--tool');
  const arch = argOf('--arch');
  if (!rootArg || !toolArg || !arch) {
    console.error('用法: node scripts/postprocess-appimage.mjs --root <bundle/appimage> --tool <linuxdeploy-plugin-appimage.AppImage> --arch <x86_64>');
    process.exit(2);
  }

  const root = resolve(rootArg);
  const tool = resolve(toolArg);
  if (!existsSync(root) || !statSync(root).isDirectory()) throw new Error(`AppImage 产物目录不存在：${root}`);
  if (!existsSync(tool) || !lstatSync(tool).isFile()) throw new Error(`Tauri AppImage 输出插件不存在：${tool}`);

  const appDir = oneEntry(root, '.AppDir', 'dir');
  const artifact = oneEntry(root, '.AppImage', 'file');
  const libDir = join(appDir, 'usr', 'lib');
  for (const name of APPIMAGE_HOST_WAYLAND_LIBS) {
    const path = join(libDir, name);
    if (existsSync(path)) {
      if (lstatSync(path).isDirectory()) throw new Error(`拒绝删除非文件的冲突项：${path}`);
      unlinkSync(path);
      console.log(`removed: ${path}`);
    }
  }
  const hookChanged = patchGtkHook(appDir);
  console.log(`GTK hook: ${hookChanged ? '已补 GIO_MODULE_DIR' : '已有正确 GIO_MODULE_DIR'}`);
  const gtkHook = readFileSync(join(appDir, 'apprun-hooks', 'linuxdeploy-plugin-gtk.sh'), 'utf8');
  console.log(`GTK hook exports: GIO_MODULE_DIR=${exportLines(gtkHook, GIO_DIR_PREFIX).length}, GIO_EXTRA_MODULES=${exportLines(gtkHook, GIO_EXTRA_PREFIX).length}`);

  const violations = appImageRuntimeViolations(appDir);
  if (violations.length > 0) throw new Error(`AppDir 兼容契约未成立：\n  - ${violations.join('\n  - ')}`);
  const members = restoreCoreSeed(appDir);
  console.log(`AppDir canonical seeds: core SHA256=${members[APPIMAGE_CORE_MEMBER].sha256}; Cronet SHA256=${members[CRONET_MEMBER].sha256}; policy=original-bytes`);

  const temporary = join(dirname(artifact), `.${basename(artifact)}.postprocess.AppImage`);
  if (existsSync(temporary)) throw new Error(`拒绝覆盖上次失败残件：${temporary}`);
  try {
    const run = spawnSync(tool, [`--appdir=${appDir}`], {
      env: {
        ...process.env,
        APPIMAGE_EXTRACT_AND_RUN: '1',
        ARCH: arch,
        LDAI_OUTPUT: temporary,
      },
      stdio: 'inherit',
    });
    if (run.error) throw run.error;
    if (run.status !== 0) throw new Error(`AppImage 重封失败：输出插件退出码 ${run.status}`);
    if (!existsSync(temporary) || !lstatSync(temporary).isFile() || statSync(temporary).size < 1024 * 1024) {
      throw new Error(`AppImage 重封结果缺失或异常小：${temporary}`);
    }
    verifyAppImageMembers(temporary, members);
    console.log('AppImage payload: actual sing-box SHA/mode and adjacent Cronet SHA/mode verified');
    renameSync(temporary, artifact);
  } finally {
    if (existsSync(temporary)) unlinkSync(temporary);
  }
  console.log(`ok: AppImage host graphics compatibility → ${artifact} (${statSync(artifact).size} bytes)`);
}

const invoked = process.argv[1] ? resolve(process.argv[1]) : '';
if (invoked === fileURLToPath(import.meta.url)) main();
