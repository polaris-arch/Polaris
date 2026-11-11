#!/usr/bin/env node
// Import a Core release contract and regenerate everything derived from it:
// the `desktopRelease` selection and the kernel paragraphs of NOTICE.
//   node scripts/desktop-core/import-release.mjs [<new release-contract.json>]
//   node scripts/desktop-core/import-release.mjs --check
import { copyFileSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { digest, requireGraph as demand } from './source-graph.mjs';
import { validateReleaseContract } from './release-consumer.mjs';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
export const CONTRACT_PATH = 'scripts/desktop-core/release-contract.json';
const MOBILE_PATH = 'scripts/mobile-core/source-contract.json';
const COMPONENTS_START = '  - 桌面 sing-box ';
const COMPONENTS_END = '  - cronet-go ';
const SOURCE_START = '════ 对应源码 / Corresponding Source ════';

export const releaseSelection = (contractSha256) => ({
  schema: 'polaris-app-release-selection-v1', contractPath: CONTRACT_PATH, contractSha256 });

// 1.15.0-polaris-alpha.12.19 -> 1.15.0-alpha.12 (the SagerNet baseline tag).
const upstream = (version) => version.replace(/-polaris-([a-z]+\.[0-9]+)(?:\.[0-9]+)?$/, '-$1');

function components(desktop, mobile, contractSha256) {
  const repo = `https://github.com/${desktop.repository}`;
  return [
    `  - 桌面 sing-box v${desktop.version} (GPLv3) — ${repo}`,
    `        上游基线源码：https://github.com/SagerNet/sing-box/tree/v${upstream(desktop.version)}`,
    `        固定源码：${repo}/tree/${desktop.sourceCommit}`,
    `        桌面发布：${repo}/releases/tag/${desktop.tag}`,
    `        消费合同 SHA-256：${contractSha256}`,
    '        Linux SO / Windows DLL 随核同包；macOS 静态链接，无独立 sidecar。',
    `  - 移动端 libbox v${mobile.version} (GPLv3) — ${mobile.sourceURL}`,
    `        上游基线源码：https://github.com/SagerNet/sing-box/tree/v${upstream(mobile.version)}`,
    `        固定源码：${mobile.sourceURL}/tree/${mobile.sourceCommit}`,
    `        源码标签：${mobile.sourceURL}/tree/${mobile.sourceTag}`,
  ];
}

function correspondingSource(desktop, mobile) {
  const repo = `https://github.com/${desktop.repository}`;
  return [
    SOURCE_START,
    '',
    `桌面 sing-box 源码：${repo}/tree/${desktop.tag}（commit ${desktop.sourceCommit}）`,
    `桌面 sing-box 源码归档：${desktop.sourceArchive.url}`,
    `移动端 libbox 源码：${mobile.sourceURL}/tree/${mobile.sourceTag}（commit ${mobile.sourceCommit}）`,
    '对 Android 产物，完整对应源码还包括 Polaris 自身：https://github.com/polaris-arch/Polaris',
    '安装包不附带内核源码包；以上公开链接即对应源码的获取方式。',
    '',
    `Desktop sing-box source: ${repo}/tree/${desktop.tag} (commit ${desktop.sourceCommit})`,
    `Desktop sing-box source archive: ${desktop.sourceArchive.url}`,
    `Mobile libbox source: ${mobile.sourceURL}/tree/${mobile.sourceTag} (commit ${mobile.sourceCommit})`,
    'For Android, Corresponding Source also includes Polaris\'s own complete source',
    'at https://github.com/polaris-arch/Polaris. Installers do not bundle the kernel',
    'source archive; the public links above are how to obtain the Corresponding',
    'Source. If a source link stops working, please open a project issue.',
    '',
  ];
}

// Replace only the generated regions; every other NOTICE line is hand-written.
export function renderNotice(notice, desktop, mobile, contractSha256) {
  const lines = notice.split('\n');
  const start = lines.findIndex((line) => line.startsWith(COMPONENTS_START));
  const end = lines.findIndex((line) => line.startsWith(COMPONENTS_END));
  const source = lines.indexOf(SOURCE_START);
  demand(start >= 0 && end > start && source > end, 'NOTICE generated regions missing');
  return [...lines.slice(0, start), ...components(desktop, mobile, contractSha256),
    ...lines.slice(end, source), ...correspondingSource(desktop, mobile)].join('\n');
}

export function generated(root = ROOT) {
  const raw = readFileSync(join(root, CONTRACT_PATH));
  const contract = validateReleaseContract(JSON.parse(raw)), contractSha256 = digest(raw);
  const mobile = JSON.parse(readFileSync(join(root, MOBILE_PATH), 'utf8'));
  demand(mobile.schema === 'polaris-mobile-fork-input-v1', 'Mobile source contract differs');
  const manifest = JSON.stringify({ desktopRelease: releaseSelection(contractSha256) }, null, 2) + '\n';
  const notice = renderNotice(readFileSync(join(root, 'NOTICE'), 'utf8'), contract, mobile, contractSha256);
  return { 'src-tauri/core-manifest.json': manifest, NOTICE: notice };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  try {
    if (args.length === 1 && args[0] === '--check') {
      const stale = Object.entries(generated()).filter(([path, text]) => readFileSync(join(ROOT, path), 'utf8') !== text);
      demand(!stale.length, `Regenerate with import-release.mjs: ${stale.map(([path]) => path).join(', ')}`);
    } else {
      demand(args.length <= 1, 'Usage: import-release.mjs [<release-contract.json>] | --check');
      if (args.length) {
        validateReleaseContract(JSON.parse(readFileSync(args[0], 'utf8')));
        copyFileSync(args[0], join(ROOT, CONTRACT_PATH));
      }
      for (const [path, text] of Object.entries(generated())) writeFileSync(join(ROOT, path), text);
    }
  } catch (error) {
    console.error(`import-release: ${error.message}`);
    process.exitCode = 1;
  }
}
