#!/usr/bin/env node
/**
 * fetch-dashboard.mjs — 拉取 sing-box 官方面板（SagerNet/sing-box-dashboard 的 gh-pages 构建产物）到
 * resources/dashboard/，供 Tauri bundle resources 随安装包打包。
 *
 * 用法：node scripts/fetch-dashboard.mjs [--force]
 *
 * 为什么随包内置：面板原走 sing-box services[].dashboard.enabled 首次联网下载，慢启动 + 离线不可用。改随包后
 * 核以 dashboard:{enabled,path:<bundledDir>} 直接 serve 本地文件（零联网）。运行时「刷新面板资源」下载新版
 * 覆盖至 <userData>/dashboard（serve 优先用覆盖版、否则回落内置）。
 *
 * 机制：下载固定 commit zipball → 校验摘要 → 解压 → **剔掉 web-only 残留**（见 [`pruneWebOnlyArtifacts`]）
 * → 把含 index.html 的目录平铺到 resources/dashboard/。
 * 先解到临时目录、校验完整 inventory 并拒绝符号链接，再替换目标。
 *
 * 直移自 上游 scripts/fetch-dashboard.mjs（语言无关）。
 */
import { createHash } from 'node:crypto';
import { lstatSync } from 'node:fs';
import { boundedDownload } from './lib/bounded-download.mjs';
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  renameSync,
  writeFileSync,
} from 'fs';
import { tmpdir } from 'os';
import { dirname, join, resolve } from 'path';
import { fileURLToPath } from 'url';

import { extractZip } from './lib/extract-zip.mjs';

// Fixed official codeload bytes and independently enumerated post-prune inventory.
export const DASHBOARD_PIN = Object.freeze({
  commit: 'd77c2f016a7681aa1704e9f83c69139edb230c97',
  sha256: 'fa3afee26f13fb5a980a8df7598d996c2dba2d5bf9aa4e1ce0b63c46c757511f',
  bytes: 6717667,
  inventorySha256: 'd2cca077fc5495b783c501000c39b1a1bf0190c445c5707fd8a093cec19acd67',
});
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
export function verifyDashboardArchive(bytes, pin = DASHBOARD_PIN) {
  if (bytes.length !== pin.bytes || digest(bytes) !== pin.sha256) throw new Error('Dashboard archive identity mismatch');
}
export function dashboardInventory(dir) {
  const rows = [];
  const walk = (base, prefix = '') => {
    const stat = lstatSync(base);
    if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('Dashboard directory redirected');
    for (const name of readdirSync(base).sort()) {
      const path = join(base, name), relative = prefix + name, info = lstatSync(path);
      if (info.isSymbolicLink()) throw new Error('Dashboard symlink forbidden');
      if (info.isDirectory()) walk(path, relative + '/');
      else if (info.isFile()) { const bytes = readFileSync(path); rows.push([relative, bytes.length, digest(bytes)]); }
      else throw new Error('Dashboard non-regular member');
    }
  };
  walk(dir); rows.sort((a, b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0);
  return digest(JSON.stringify(rows));
}
export function dashboardFresh(dir) {
  try { return dashboardInventory(dir) === DASHBOARD_PIN.inventorySha256; } catch { return false; }
}

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const DEST = join(ROOT, 'resources', 'dashboard');
const FORCE = process.argv.includes('--force');
/** 只清洗盘上已有的 resources/dashboard/（不下载）—— 供一次性收拾旧产物用。 */
const PRUNE_ONLY = process.argv.includes('--prune');

/**
 * 上游 gh-pages 产物里**只对网页部署有意义**的文件，随包分发时是纯负重与噪声。
 * 剔在**解压之后、落盘之前**（治本：下次重新拉取也不会回来），而不是打包时过滤（治标：
 * 换一条打包路径就漏，且工作树里仍然躺着这些文件）。
 *
 *   - `.nojekyll`：告诉 GitHub Pages 别跑 Jekyll。包里没有 GitHub Pages。
 *   - `sw.js` / `registerSW.js` / `workbox-*.js`：vite-plugin-pwa 的 service worker。面板由核以
 *     `dashboard.path` 从本地目录 serve，注册 SW 最好的情况是无用、坏的情况是把旧资源缓存住
 *     （面板更新后仍显示旧版）。
 *   - `manifest.webmanifest` + `pwa-*.png` / `maskable-icon-*.png`：PWA 安装元数据与它专属的图标。
 *     嵌入式 webview 不会被「安装到桌面」。**这几个图标只被 manifest 与 sw.js 引用**
 *     （2026-08-29 实测：`grep -rl pwa-192 resources/dashboard` 只命中这两个文件），
 *     故它们与 manifest 必须同进同退，否则不是留下悬空引用就是留下无人引用的死重。
 *
 * 保留（别顺手删）：`index.html` 直接引用的 `favicon.ico` / `favicon.svg` /
 * `apple-touch-icon-180x180.png`，以及 `assets/**`、`licenses/**`。
 */
export const DASHBOARD_WEB_ONLY_FILES = Object.freeze([
  '.nojekyll',
  'sw.js',
  'registerSW.js',
  'manifest.webmanifest',
  'pwa-64x64.png',
  'pwa-192x192.png',
  'pwa-512x512.png',
  'maskable-icon-512x512.png',
]);

/** `workbox-<hash>.js` 的哈希随上游构建变，按前缀认。 */
const WORKBOX_RE = /^workbox-[0-9a-f]+\.js$/;

/**
 * `index.html` 里由 vite-plugin-pwa 注入、指向上面那些文件的两个标签。
 * 删文件不删引用 = 两个 404 + 一次 SW 注册失败，故两件事必须一起做。
 */
const INDEX_HTML_PWA_TAGS = [
  /<link\s+rel="manifest"\s+href="\.\/manifest\.webmanifest"\s*>/,
  /<script\s+id="vite-plugin-pwa:register-sw"[^>]*><\/script>/,
];

/**
 * 剔除 web-only 残留并同步摘掉 index.html 里的引用。返回被删文件名。
 *
 * **不因「没找到」而硬失败**：上游哪天不再产 PWA，这里就该是空操作。真正兜底的是
 * `scripts/verify-packaging.mjs inventory` —— 它按白名单清点包内容，本函数漏掉的任何形态
 * （换了名字的 workbox、新增的 web-only 文件）都会在那里以「未登记」转红。
 * 但**删了文件却没摘掉引用**是本函数自己的错，那种情况必须当场炸（见末尾自检）。
 */
export function pruneWebOnlyArtifacts(dir) {
  const removed = [];
  for (const name of readdirSync(dir)) {
    if (!DASHBOARD_WEB_ONLY_FILES.includes(name) && !WORKBOX_RE.test(name)) continue;
    rmSync(join(dir, name), { recursive: true, force: true });
    removed.push(name);
  }
  const indexHtml = join(dir, 'index.html');
  if (removed.length > 0 && existsSync(indexHtml)) {
    let html = readFileSync(indexHtml, 'utf8');
    for (const re of INDEX_HTML_PWA_TAGS) html = html.replace(re, '');
    writeFileSync(indexHtml, html);
    // 自检：删掉的文件名不得再出现在 index.html 里。上游换了注入写法（属性顺序/引号变了）
    // 会让上面的替换静默落空，而那正是「删了文件、留下 404」的形状 —— 必须炸，不能静默。
    const dangling = removed.filter((n) => readFileSync(indexHtml, 'utf8').includes(n));
    if (dangling.length > 0) {
      throw new Error(
        `已删除 ${dangling.join(', ')}，但 index.html 里仍引用它们 —— 上游的注入写法变了，` +
          `INDEX_HTML_PWA_TAGS 需同步更新（留着就是包里的 404）`
      );
    }
  }
  return removed;
}

/**
 * 抓取流程本体。包成函数 + 下面那道「是否被当脚本直接调用」的判据，是为了让本文件**可被 import**：
 * 上面那两个导出（[`DASHBOARD_WEB_ONLY_FILES`] / [`pruneWebOnlyArtifacts`]）要能被测试与其它脚本直接调，
 * 而此前顶层就有 `process.exit(0)`（「index.html 已存在 ⇒ skip」那条）—— 一 import 整个进程就退了，
 * 剪枝逻辑因此**没法被单独验证**，只能靠肉眼读。同 scripts/postprocess-appimage.mjs 的写法。
 */
function main() {
  if (PRUNE_ONLY) {
    if (!existsSync(DEST)) {
      console.error(`FAILED: ${DEST} 不存在 —— --prune 只清洗已在盘上的面板产物`);
      process.exit(1);
    }
    const removed = pruneWebOnlyArtifacts(DEST);
    console.log(`ok: prune resources/dashboard/ —— 剔除 ${removed.length} 个 web-only 残留：${removed.join(', ') || '（无）'}`);
    process.exit(0);
  }

  const ZIP_URL = `https://codeload.github.com/SagerNet/sing-box-dashboard/zip/${DASHBOARD_PIN.commit}`;
  if (!FORCE && dashboardFresh(DEST)) {
    console.log('skip: dashboard matches pinned final inventory');
    return;
  }

  const work = mkdtempSync(join(tmpdir(), 'polaris-dashboard-'));
  const zipPath = join(work, 'dashboard.zip');
  const extractDir = join(work, 'extracted');

  try {
    console.log(`downloading sing-box-dashboard (gh-pages) → ${zipPath} ...`);
    boundedDownload(ZIP_URL, { output: zipPath, stdio: 'inherit' });

    verifyDashboardArchive(readFileSync(zipPath));
    mkdirSync(extractDir, { recursive: true });
    console.log('extracting ...');
    extractZip(zipPath, extractDir);

    const top = readdirSync(extractDir).map((n) => join(extractDir, n));
    let uiRoot = top.find((p) => existsSync(join(p, 'index.html')));
    if (!uiRoot && existsSync(join(extractDir, 'index.html'))) uiRoot = extractDir;
    if (!uiRoot) {
      throw new Error('解压产物中未找到 index.html（gh-pages 结构可能变化）');
    }

    // 剔 web-only 残留：**在临时目录里做**，落盘的那一份从一开始就是干净的
    // （下次重新拉取照样走这一步 ⇒ 不会回潮；打包期不需要任何过滤）。
    const removed = pruneWebOnlyArtifacts(uiRoot);
    console.log(`pruned ${removed.length} web-only file(s): ${removed.join(', ') || '(none)'}`);

    // 原子替换：先拷到 DEST.tmp 再 rename 顶替。
    if (!dashboardFresh(uiRoot)) throw new Error('Dashboard final inventory differs');
    const tmpDest = `${DEST}.tmp`;
    rmSync(tmpDest, { recursive: true, force: true });
    mkdirSync(dirname(DEST), { recursive: true });
    cpSync(uiRoot, tmpDest, { recursive: true, dereference: true });
    if (!dashboardFresh(tmpDest)) throw new Error('Dashboard staging inventory differs');
    rmSync(DEST, { recursive: true, force: true });
    renameSync(tmpDest, DEST);

    console.log(`ok: resources/dashboard/ ready (index.html present).`);
  } catch (e) {
    console.error(`FAILED: ${e.message}`);
    process.exitCode = 1;
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

const invoked = process.argv[1] ? resolve(process.argv[1]) : '';
if (invoked === fileURLToPath(import.meta.url)) main();
