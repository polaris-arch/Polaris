#!/usr/bin/env node
/**
 * Android 启动图标生成器 —— 唯一真值是 `src-tauri/icons/polaris-logo.svg`。
 *
 * # 为什么要有这个脚本
 *
 * `gen/android` 是入库的真实 Gradle 工程，`res/` 下原本装的是 Android Studio 模板货
 * （`#3DDC84` 绿底 + 机器人）。把它换成品牌资产，如果靠人手导出一次性拍进去，就没有任何东西
 * 能保证「树里这 20 张位图确实是当前那份 SVG 渲染出来的」——SVG 改了没重导、或有人直接用图形
 * 软件改了某一档，都不会有人发现。所以位图**必须由脚本产出**，并由
 * `ui/src/contracts/android-launcher-icon.test.ts` 反过来钉住。
 *
 * # 构图：两个面的对差（数字都是量出来的，不是拍的）
 *
 * 面 A（`polaris-logo.svg` 的实际构图，1024 画布）：
 *   - 磁贴 `rect(100,100,824,824,rx=185)` ⇒ 边长 **824**（画布的 80.47%），四边留白 100（9.77%）。
 *   - 星标：512 空间的品牌 mark 经 `translate(174.08,174.08) scale(1.32)` 落进磁贴，
 *     几何中心恰好是 (512,512)（本脚本 assert 之）。
 *     其 ink 外接半径由 `pl-star` 的路径极值给出：顶尖 y=18 距中心 (256,256) 为 **238**（512 空间），
 *     ×1.32 = **314.16**（1024 空间）⇒ 外接圆直径 **628.32**。
 *     实测校核：rsvg 渲染后 alpha≥8 的 bbox 为 623×628、外接直径 627.0，与几何值一致（差值是 AA 阈值）。
 *   - 星标外接直径 / 磁贴边长 = 628.32 / 824 = **0.76252**。这是桌面图标的品牌比例。
 *
 * 面 B（Android 自适应图标规格）：108dp 画布，只有中心 **72dp** 保证可见（各厂商裁切形状不同），
 *   前景层还会被系统做视差位移，业界安全区按 **66dp 直径**算。
 *
 * 对差的结论（也是本脚本前景层变换的全部内容）：
 *   **把面 A 的磁贴（824 源单位）等比映射到面 B 的 72dp 可见视口**，即 `k = 72 / 824 = 0.0873786`。
 *   于是星标在 108dp 画布上的外接直径 = 628.32 × k = **54.90dp**，
 *   既 1:1 复刻了桌面上的「星占磁贴 76.25%」，又落在 66dp 安全圆内（每边余量 5.55dp 吃视差）。
 *   —— 内边距不是拍的，是这条等式的结果；把 1024 的 logo 直接铺满 108 画布会得到 82.3dp，超安全区。
 *
 * # 层的划分
 *
 *   background = 纯色 `--logo-tile`（浅色档 `0 0% 100%` = #FFFFFF，见 `ui/src/styles/tokens.resolved.css`）。
 *     取浅色值而非深色值的理由与 SVG 自己的注释一致：应用图标是固定资产，不跟随系统主题。
 *     不复刻磁贴的圆角与 1px 描边——那两样在 Android 上由系统 mask 提供，画进背景层会变成「贴中贴」。
 *   foreground = 星标本体（透明底），按上面的 k 定位。
 *   monochrome = 同一份几何，把 `fill/stroke="url(#pl-*)"` 换成 #000 得到纯 alpha 剪影。
 *     剪影后仍认得出：轮廓是八角星 + 外圈轨道环 + 环上两处缺口里的圆点，与彩色版逐像素同形。
 *
 * # 确定性
 *
 * 同输入同字节输出：渲染器是 rsvg-convert（cairo 的 PNG 写出不含时间戳），派生 SVG 由固定字符串
 * 拼接、浮点一律 `toFixed(6)`，manifest 的键排序固定、不含时间戳与绝对路径。
 * 跨渲染器版本**不保证**逐字节一致（AA 实现会变），所以 manifest 记下渲染器身份，门据此决定
 * 是否跑逐字节腿（见测试文件头注）。
 *
 * 用法：
 *   node scripts/gen-android-icons.mjs              # 写进工作树
 *   node scripts/gen-android-icons.mjs --out <dir>  # 写进 <dir>/<仓库相对路径>，工作树不动（门用）
 */
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, readFileSync, rmdirSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = fileURLToPath(new URL('..', import.meta.url));
const SVG_REL = 'src-tauri/icons/polaris-logo.svg';
const RES_REL = 'src-tauri/gen/android/app/src/main/res';
const MANIFEST_REL = 'scripts/android-icons.manifest.json';

/** Android 密度阶梯：mipmap 后缀 → 相对 mdpi 的倍率。 */
const DENSITIES = [
  ['mdpi', 1],
  ['hdpi', 1.5],
  ['xhdpi', 2],
  ['xxhdpi', 3],
  ['xxxhdpi', 4],
];
/** legacy 启动图标的 dp 基线（API 24/25 没有自适应图标，走这五档方图/圆图）。 */
const LEGACY_DP = 48;
/** 自适应图标画布的 dp 基线（前景层与单色层都画在 108dp 上）。 */
const ADAPTIVE_DP = 108;
/** 只有中心 72dp 保证可见——前景层的构图基准。 */
const VIEWPORT_DP = 72;
/** 业界安全圆直径；星标外接圆必须落在它里面。 */
const SAFE_CIRCLE_DP = 66;
/** 背景层颜色：`--logo-tile` 浅色档（`0 0% 100%`）解析成 hex。 */
const BACKGROUND_COLOR = '#FFFFFF';
/** 模板残留：换成品牌资产后这两份必须消失（`#3DDC84` 绿底与机器人前景）。 */
const TEMPLATE_RESIDUE = [
  `${RES_REL}/drawable/ic_launcher_background.xml`,
  `${RES_REL}/drawable-v24/ic_launcher_foreground.xml`,
];

function fail(what) {
  throw new Error(
    `[gen-android-icons] ${SVG_REL} 的结构与本脚本的取材面不符：${what}。\n` +
      '图标生成器与品牌资产强绑定；请对齐两侧后再跑，不要把断言删掉了事。',
  );
}

function must(re, src, what) {
  const m = re.exec(src);
  if (!m) fail(what);
  return m;
}

/** 从 `at` 处的 `<g …>` 起，按 g 的开闭配平取出整个元素（星标组内有嵌套 g）。 */
function extractGroup(src, at) {
  const re = /<g\b|<\/g>/g;
  re.lastIndex = at;
  let depth = 0;
  let m;
  while ((m = re.exec(src)) !== null) {
    depth += m[0] === '</g>' ? -1 : 1;
    if (depth === 0) return src.slice(at, m.index + m[0].length);
  }
  return fail('星标组的 <g> 没有配平的 </g>');
}

function numbers(text) {
  return (text.match(/-?\d+(?:\.\d+)?/g) ?? []).map(Number);
}

function f(n) {
  return n.toFixed(6);
}

// ── 取材：把几何从 SVG 里读出来，一个常数都不在这里拍 ───────────────────────────
const svg = readFileSync(join(REPO, SVG_REL), 'utf8');

const viewBox = must(/viewBox="0 0 (\d+(?:\.\d+)?) (\d+(?:\.\d+)?)"/, svg, 'svg 的 viewBox');
const canvas = Number(viewBox[1]);
if (Number(viewBox[2]) !== canvas) fail('画布不是正方形');

const defs = must(/<defs>[\s\S]*?<\/defs>/, svg, '<defs> 块')[0];

const tile = must(
  /<rect x="([\d.]+)" y="([\d.]+)" width="([\d.]+)" height="([\d.]+)" rx="([\d.]+)" ry="([\d.]+)" fill="url\(#tilebg\)"\/>/,
  svg,
  '磁贴 rect（fill=url(#tilebg)）',
);
const tileX = Number(tile[1]);
const tileY = Number(tile[2]);
const tileW = Number(tile[3]);
const tileH = Number(tile[4]);
if (tileW !== tileH) fail('磁贴不是正方形');

const tileClip = must(
  /<clipPath id="tile"><rect x="([\d.]+)" y="([\d.]+)" width="([\d.]+)" height="([\d.]+)" rx="([\d.]+)" ry="([\d.]+)"\/><\/clipPath>/,
  svg,
  'clipPath#tile 里的 rect',
);

const rim = must(
  /<rect x="([\d.]+)" y="([\d.]+)" width="([\d.]+)" height="([\d.]+)" rx="([\d.]+)" ry="([\d.]+)" fill="none" stroke="(#[0-9A-Fa-f]{6})" stroke-width="([\d.]+)"\/>/,
  svg,
  '磁贴描边 rect（--logo-tile-bd）',
);

const starTransform = must(
  /<g transform="translate\((-?[\d.]+) (-?[\d.]+)\) scale\(([\d.]+)\)">/,
  svg,
  '星标组的 transform',
);
const starGroup = extractGroup(svg, starTransform.index);
const tx = Number(starTransform[1]);
const ty = Number(starTransform[2]);
const starScale = Number(starTransform[3]);

const starPath = must(/<path id="pl-star" d="([^"]+)"/, svg, 'pl-star 的 d');
const starNums = numbers(starPath[1]);
if (starNums.length === 0 || starNums.length % 2 !== 0) fail('pl-star 的坐标不是成对的');

const orbit = must(
  /<circle cx="(\d+)" cy="(\d+)" r="([\d.]+)" fill="none" stroke="url\(#pl-orbit\)" stroke-width="([\d.]+)"/,
  starGroup,
  '轨道环 circle',
);
const markCx = Number(orbit[1]);
const markCy = Number(orbit[2]);

// 星标在 512 空间里离 mark 中心最远的 ink 半径：路径极值 / 轨道环外沿 / 两个圆点，取最大。
let markRadius = 0;
for (let i = 0; i < starNums.length; i += 2) {
  markRadius = Math.max(
    markRadius,
    Math.abs(starNums[i] - markCx),
    Math.abs(starNums[i + 1] - markCy),
  );
}
const pathRadius = markRadius;
markRadius = Math.max(markRadius, Number(orbit[3]) + Number(orbit[4]) / 2);
for (const dot of starGroup.matchAll(/<circle cx="(\d+)" cy="(\d+)" r="(\d+)" fill="url\(#pl-dot\)"\/>/g)) {
  const dx = Math.abs(Number(dot[1]) - markCx) + Number(dot[3]);
  const dy = Math.abs(Number(dot[2]) - markCy) + Number(dot[3]);
  markRadius = Math.max(markRadius, dx, dy);
}
if (markRadius !== pathRadius) fail('星标的最外沿不再是 pl-star 的尖端（构图变了，内边距的推导要重来）');

// 星标 ink 在 1024 源空间里的中心与外接圆。
const starCx = tx + markCx * starScale;
const starCy = ty + markCy * starScale;
if (Math.abs(starCx - canvas / 2) > 0.01 || Math.abs(starCy - canvas / 2) > 0.01) {
  fail(`星标中心 (${starCx}, ${starCy}) 不在画布中心，前景层的居中推导不成立`);
}
const starDiaSrc = 2 * markRadius * starScale;
const starOverTile = starDiaSrc / tileW;

// 面 A → 面 B：磁贴映射到 72dp 可见视口。
const srcToDp = VIEWPORT_DP / tileW;
const starDiaDp = starDiaSrc * srcToDp;
if (!(starDiaDp < SAFE_CIRCLE_DP)) {
  fail(`星标在 108dp 画布上的外接直径 ${starDiaDp.toFixed(2)}dp 超出 ${SAFE_CIRCLE_DP}dp 安全圆`);
}

// ── 派生 SVG ────────────────────────────────────────────────────────────────
const SVG_OPEN = (px) =>
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${ADAPTIVE_DP} ${ADAPTIVE_DP}" width="${px}" height="${px}">`;

function foregroundSvg(px, monochrome) {
  const mark = monochrome ? starGroup.replace(/\b(fill|stroke)="url\(#pl-[a-z]+\)"/g, '$1="#000000"') : starGroup;
  if (monochrome && mark === starGroup) fail('单色层没有替换到任何 pl-* 渐变引用');
  const half = ADAPTIVE_DP / 2;
  return (
    `${SVG_OPEN(px)}\n${defs}\n` +
    `<g transform="translate(${f(half)} ${f(half)}) scale(${f(srcToDp)}) translate(${f(-starCx)} ${f(-starCy)})">\n` +
    `${mark}\n</g>\n</svg>\n`
  );
}

/** 圆形 legacy 图标：把磁贴的圆角方 → 内切圆（半径 = 磁贴半边长），描边同心缩进。 */
function roundSvg() {
  const cx = tileX + tileW / 2;
  const cy = tileY + tileH / 2;
  let out = svg;
  const subs = [
    [tile[0], `<circle cx="${f(cx)}" cy="${f(cy)}" r="${f(tileW / 2)}" fill="url(#tilebg)"/>`],
    [
      tileClip[0],
      `<clipPath id="tile"><circle cx="${f(cx)}" cy="${f(cy)}" r="${f(tileW / 2)}"/></clipPath>`,
    ],
    [
      rim[0],
      `<circle cx="${f(Number(rim[1]) + Number(rim[3]) / 2)}" cy="${f(Number(rim[2]) + Number(rim[4]) / 2)}" ` +
        `r="${f(Number(rim[3]) / 2)}" fill="none" stroke="${rim[7]}" stroke-width="${rim[8]}"/>`,
    ],
  ];
  for (const [from, to] of subs) {
    if (!out.includes(from)) fail('圆形图标的替换目标不在源里');
    out = out.replace(from, to);
  }
  return out;
}

// ── 渲染 ────────────────────────────────────────────────────────────────────
const args = process.argv.slice(2);
const outAt = args.indexOf('--out');
const OUT_ROOT = outAt >= 0 ? args[outAt + 1] : REPO;
if (outAt >= 0 && !OUT_ROOT) fail('--out 缺参数');
const IN_REPO = OUT_ROOT === REPO;

const rendererVersion = execFileSync('rsvg-convert', ['--version'], { encoding: 'utf8' })
  .split('\n')[0]
  .trim();

const written = new Map();

function emit(rel, buffer) {
  const abs = join(OUT_ROOT, rel);
  mkdirSync(dirname(abs), { recursive: true });
  writeFileSync(abs, buffer);
  written.set(rel, createHash('sha256').update(buffer).digest('hex'));
}

const scratchRoot = existsSync('/var/tmp') ? '/var/tmp' : REPO;
const scratch = join(scratchRoot, `polaris-icon-render-${process.pid}`);
mkdirSync(scratch, { recursive: true });

function render(name, source, px) {
  const src = join(scratch, `${name}.svg`);
  const dst = join(scratch, `${name}-${px}.png`);
  writeFileSync(src, source);
  execFileSync('rsvg-convert', ['--background-color=none', '-w', String(px), '-h', String(px), src, '-o', dst]);
  return readFileSync(dst);
}

for (const [density, factor] of DENSITIES) {
  const legacyPx = LEGACY_DP * factor;
  const adaptivePx = ADAPTIVE_DP * factor;
  if (!Number.isInteger(legacyPx) || !Number.isInteger(adaptivePx)) fail(`${density} 档算出了非整数像素`);
  emit(`${RES_REL}/mipmap-${density}/ic_launcher.png`, render('legacy', svg, legacyPx));
  emit(`${RES_REL}/mipmap-${density}/ic_launcher_round.png`, render('legacy-round', roundSvg(), legacyPx));
  emit(
    `${RES_REL}/mipmap-${density}/ic_launcher_foreground.png`,
    render('foreground', foregroundSvg(adaptivePx, false), adaptivePx),
  );
  emit(
    `${RES_REL}/mipmap-${density}/ic_launcher_monochrome.png`,
    render('monochrome', foregroundSvg(adaptivePx, true), adaptivePx),
  );
}

const adaptiveXml =
  '<?xml version="1.0" encoding="utf-8"?>\n' +
  '<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">\n' +
  '    <background android:drawable="@color/ic_launcher_background" />\n' +
  '    <foreground android:drawable="@mipmap/ic_launcher_foreground" />\n' +
  '    <monochrome android:drawable="@mipmap/ic_launcher_monochrome" />\n' +
  '</adaptive-icon>\n';
emit(`${RES_REL}/mipmap-anydpi-v26/ic_launcher.xml`, Buffer.from(adaptiveXml, 'utf8'));
emit(`${RES_REL}/mipmap-anydpi-v26/ic_launcher_round.xml`, Buffer.from(adaptiveXml, 'utf8'));

emit(
  `${RES_REL}/values/ic_launcher_background.xml`,
  Buffer.from(
    '<?xml version="1.0" encoding="utf-8"?>\n' +
      // ⚠️ XML 注释里不允许出现连续两个连字符（aapt2 直接报错），所以 token 名写成 `logo-tile`
      //    而不是它在 CSS 里的 `\u002d\u002dlogo-tile` 形态。
      '<!-- 自适应图标的背景层：设计 token `logo-tile` 的浅色档（0 0% 100%），\n' +
      '     见 ui/src/styles/tokens.resolved.css 里的 CSS 变量。应用图标是固定资产，不跟随系统\n' +
      '     主题，所以取浅色值，与 src-tauri/icons/polaris-logo.svg 的磁贴同源。 -->\n' +
      '<resources>\n' +
      `    <color name="ic_launcher_background">${BACKGROUND_COLOR}</color>\n` +
      '</resources>\n',
    'utf8',
  ),
);

const manifest = {
  _: '由 scripts/gen-android-icons.mjs 生成；ui/src/contracts/android-launcher-icon.test.ts 据此钉住位图来源。请勿手改。',
  generator: 'scripts/gen-android-icons.mjs',
  renderer: rendererVersion,
  source: {
    path: SVG_REL,
    sha256: createHash('sha256').update(readFileSync(join(REPO, SVG_REL))).digest('hex'),
  },
  geometry: {
    canvas,
    tileSide: tileW,
    starCircumscribedDiameter: Number(starDiaSrc.toFixed(4)),
    starOverTile: Number(starOverTile.toFixed(5)),
    adaptiveCanvasDp: ADAPTIVE_DP,
    guaranteedViewportDp: VIEWPORT_DP,
    safeCircleDp: SAFE_CIRCLE_DP,
    starDiameterDp: Number(starDiaDp.toFixed(4)),
    srcToDp: Number(srcToDp.toFixed(9)),
  },
  files: Object.fromEntries([...written].sort(([a], [b]) => (a < b ? -1 : 1))),
};
const manifestPath = join(OUT_ROOT, MANIFEST_REL);
mkdirSync(dirname(manifestPath), { recursive: true });
writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

if (IN_REPO) {
  for (const rel of TEMPLATE_RESIDUE) {
    const abs = join(REPO, rel);
    if (existsSync(abs)) {
      unlinkSync(abs);
      const dir = dirname(abs);
      if (readdirSync(dir).length === 0) rmdirSync(dir);
    }
  }
}

for (const entry of readdirSync(scratch)) unlinkSync(join(scratch, entry));
rmdirSync(scratch);

process.stdout.write(
  `[gen-android-icons] 渲染器 ${rendererVersion}\n` +
    `  磁贴 ${tileW}/${canvas} · 星标外接 ${starDiaSrc.toFixed(2)} · 星/贴 ${(starOverTile * 100).toFixed(2)}%\n` +
    `  ⇒ 108dp 画布上星标外接 ${starDiaDp.toFixed(2)}dp（安全圆 ${SAFE_CIRCLE_DP}dp）\n` +
    `  写出 ${written.size} 个资源 + manifest ${OUT_ROOT === REPO ? '（工作树）' : `→ ${OUT_ROOT}`}\n`,
);
