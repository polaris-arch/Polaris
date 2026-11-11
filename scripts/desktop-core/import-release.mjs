#!/usr/bin/env node
// Import a published Core release and regenerate everything derived from it:
// the desktop consumer contract, the Core source fields of the mobile input
// contract, the `desktopRelease` selection and the kernel paragraphs of NOTICE.
//   node scripts/desktop-core/import-release.mjs --tag <Core tag>
//   node scripts/desktop-core/import-release.mjs --check
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { inflateRawSync } from 'node:zlib';
import { digest, requireGraph as demand } from './source-graph.mjs';
import { RELEASE_TARGETS, validateReleaseContract } from './release-consumer.mjs';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
export const CONTRACT_PATH = 'scripts/desktop-core/release-contract.json';
const MOBILE_PATH = 'scripts/mobile-core/source-contract.json';
const REPO = 'polaris-arch/sing-box-polaris';
// Maintainer import through the logged-in gh CLI: Actions artifact downloads require a token.
const gh = (path) => execFileSync('gh', ['api', `repos/${REPO}/${path}`], { maxBuffer: 256 * 1024 * 1024 });
const api = (path) => JSON.parse(gh(path));
const json = (value) => JSON.stringify(value, null, 2) + '\n';
const COMPONENTS_START = '  - 桌面 sing-box ';
const COMPONENTS_END = '  - cronet-go ';
const SOURCE_START = '════ 对应源码 / Corresponding Source ════';

export const releaseSelection = (contractSha256) => ({
  schema: 'polaris-app-release-selection-v1', contractPath: CONTRACT_PATH, contractSha256 });

// 1.15.0-polaris-alpha.12.22 -> 1.15.0-alpha.12 (the SagerNet baseline tag).
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

// Native rows, legal texts and producer come from the native asset index kept in
// the publisher audit artifact named by the release body; asset identities from
// the release API; source facts from the tagged tree. App-owned fields stay as committed.
async function importRelease(tag) {
  const release = api(`releases/tags/${tag}`);
  const run = /^Audit: https:\/\/github\.com\/polaris-arch\/sing-box-polaris\/actions\/runs\/([0-9]+)$/m.exec(release.body)?.[1];
  const audit = run && api(`actions/runs/${run}/artifacts`).artifacts.filter((a) => a.name === 'github-release-publication-audit');
  demand(audit?.length === 1, 'Release publisher audit artifact missing');
  // Loaded here so verify-packaging's import of generated() keeps its dependency set.
  const { parseZipEntries } = await import('../verify-apk.mjs'), zip = gh(`actions/artifacts/${audit[0].id}/zip`);
  const member = parseZipEntries(zip).get('polaris-upstream-native-assets.json');
  demand(member?.method === 8, 'Publisher audit native asset index missing');
  const raw = inflateRawSync(zip.subarray(member.dataOffset, member.dataOffset + member.compressedSize));
  const index = JSON.parse(raw), { version, tagObject, sourceCommit, sourceTree } = index;
  demand(index.schema === 'polaris-upstream-native-assets-v1' && index.repository === REPO && index.tag === tag,
    'Native asset index differs from the release');
  const sourceURL = `https://github.com/${REPO}/archive/${sourceCommit}.tar.gz`, response = await fetch(sourceURL);
  demand(response.ok, `Source archive HTTP ${response.status}`);
  const archive = Buffer.from(await response.arrayBuffer());
  const platforms = Object.fromEntries(Object.entries(RELEASE_TARGETS).map(([key, target]) => {
    const row = index.platforms.find((r) => r.id === `sing-box-${version}-${target.os}-${target.arch}`);
    const assets = release.assets.filter((a) => a.digest === `sha256:${row?.archive.sha256}`);
    demand(assets.length === 1, `Release asset for ${key} missing`);
    const [{ name, size, id, browser_download_url: url }] = assets;
    row.archive.releaseName = name;
    return [key, { ...row, releaseAsset: { name, bytes: size, sha256: row.archive.sha256, assetId: id, releaseId: release.id, url } }];
  }));
  const contract = validateReleaseContract({ schema: 'polaris-upstream-native-release-consumer-v1',
    providerContract: 'polaris-upstream-native-release-v1', repository: REPO, repositoryId: index.repositoryId,
    releaseId: release.id, version, tag, tagObject, sourceCommit, sourceTree,
    sourcePolicy: { goVersion: platforms.linux.goVersion.slice(2) },
    sourceArchive: { name: `sing-box-polaris-${sourceCommit}.tar.gz`, url: sourceURL, bytes: archive.length, sha256: digest(archive) },
    assetIndex: { sha256: digest(raw) }, producer: index.producer, legalFiles: index.legalFiles, platforms });

  const tree = api(`git/trees/${sourceTree}?recursive=1`);
  demand(!tree.truncated, 'Source tree listing truncated');
  const entry = (path, type) => {
    const found = tree.tree.find((e) => e.path === path && e.type === type);
    demand(found, `Source ${path} missing`);
    return found;
  };
  const blob = (path) => Buffer.from(api(`git/blobs/${entry(path, 'blob').sha}`).content, 'base64');
  const goMod = blob('go.mod'), text = goMod.toString(), toolchain = /^toolchain (\S+)$/m.exec(text)?.[1];
  const vendors = Object.fromEntries([...text.matchAll(/^replace (\S+) => (\.\/third_party\/\S+)$/gm)].map(([, module, path]) => [module, {
    version: new RegExp(`^\\t${module.replaceAll('.', '\\.')} (\\S+)`, 'm').exec(text)?.[1], replacementPath: path,
    replacementTree: entry(path.slice(2), 'tree').sha,
    replacementFileCount: tree.tree.filter((e) => e.type === 'blob' && e.path.startsWith(path.slice(2) + '/')).length }]));
  const bindingPath = 'release/dependency-sources/metacubex-utls-replacement.json', binding = blob(bindingPath), meta = JSON.parse(binding);
  const sourcePolicy = { schema: 'polaris-reviewed-kernel-source-v4', sourceCommit, sourceTree, goVersion: toolchain?.slice(2),
    goModSha256: digest(goMod), goSumSha256: digest(blob('go.sum')), goDirective: /^go (\S+)$/m.exec(text)?.[1], toolchain,
    metacubex: { module: meta.module, ...vendors[meta.module], bindingPath, bindingSha256: digest(binding),
      patchPath: meta.patch, patchSha256: meta.patchSHA256, upstreamCommit: meta.upstreamCommit,
      upstreamH1: meta.upstreamH1, upstreamZipSha256: meta.upstreamModuleZipSHA256 }, vendors };
  const mobile = JSON.parse(readFileSync(join(ROOT, MOBILE_PATH), 'utf8'));
  // The android/apple tag lists are reviewed against this Core file, never parsed from it.
  demand(digest(blob(mobile.declaredTagsSource.path)) === mobile.declaredTagsSource.sha256,
    `Core libbox tags changed: review android/apple tags and declaredTagsSource in ${MOBILE_PATH}`);
  for (const platform of [mobile.android, mobile.apple]) platform.outputName = platform.outputName.replace(mobile.version, version);
  Object.assign(mobile, { version, sourceTag: tag, sourceTagObject: tagObject, sourceCommit, sourceTree,
    goModSha256: sourcePolicy.goModSha256, goSumSha256: sourcePolicy.goSumSha256, goVersion: sourcePolicy.goVersion, sourcePolicy });
  writeFileSync(join(ROOT, CONTRACT_PATH), json(contract));
  writeFileSync(join(ROOT, MOBILE_PATH), json(mobile));
  // Mobile input admission belongs to its Python consumer.
  execFileSync('python3', ['-B', join(ROOT, 'scripts/mobile-core/fork_input.py'), '--go-version']);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  try {
    if (args.length === 1 && args[0] === '--check') {
      const stale = Object.entries(generated()).filter(([path, text]) => readFileSync(join(ROOT, path), 'utf8') !== text);
      demand(!stale.length, `Regenerate with import-release.mjs: ${stale.map(([path]) => path).join(', ')}`);
    } else {
      demand(args.length === 2 && args[0] === '--tag', 'Usage: import-release.mjs --tag <Core tag> | --check');
      await importRelease(args[1]);
      for (const [path, text] of Object.entries(generated())) writeFileSync(join(ROOT, path), text);
    }
  } catch (error) {
    console.error(`import-release: ${error.message}`);
    process.exitCode = 1;
  }
}
