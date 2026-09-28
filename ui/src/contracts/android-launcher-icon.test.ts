/**
 * Android 启动图标的**来源契约门**（K8，2026-09-04）。
 *
 * # 背景：图标是最容易「绿而无信息量」的一类资产
 *
 * `gen/android/app/src/main/res/` 入库前装的是 Android Studio 模板货（`#3DDC84` 绿底 + 机器人），
 * 且**没有** `mipmap-anydpi-v26/` —— Android 8+ 上根本没有自适应图标，走的是 legacy 方图/圆图，
 * 其中 `mipmap-hdpi/ic_launcher.png` 还是模板自带的 49×49（正确值 72）。这些都不会让任何构建变红：
 * APK 照打，安装照跑，只是桌面上顶着别人的图标。
 *
 * 所以这道门的判据不是「有没有文件」，而是**这些位图是不是当前那份品牌 SVG 渲染出来的**。
 * 落法是两条腿：
 *
 *  - **哈希腿（无工具链依赖，CI 上照跑）**：`scripts/android-icons.manifest.json` 记着
 *    `src-tauri/icons/polaris-logo.svg` 的 sha256 与每个产物的 sha256。品牌 SVG 改了没重跑生成器 ⇒ 红；
 *    有人用图形软件改了某一档位图 ⇒ 红；往 `res/` 里塞一张没登记的 `ic_launcher*` ⇒ 红（集合双向相等）。
 *  - **逐字节腿（需要与 manifest 同一版渲染器）**：把 `scripts/gen-android-icons.mjs` 重跑进
 *    `/var/tmp` 下的临时目录，与工作树逐字节比。这条比哈希腿强一档：连「改了位图**并**同步改了
 *    manifest」这种蓄意伪造也挡得住。
 *
 * # 为什么逐字节腿是条件执行的（如实登记，不藏）
 *
 * 位图由 `rsvg-convert` 渲染，**抗锯齿实现随 librsvg 版本变**，跨版本不可能逐字节一致；
 * 而 `ui.yml` 跑在 ubuntu-22.04，镜像里没有 librsvg2-bin。若无条件跑这条腿，CI 只会得到一条
 * 恒红且没人能修的断言（本门管不到 workflow）。故：渲染器身份与 `manifest.renderer` 一致时才跑，
 * 不一致时**跳过并把原因写进用例标题**（vitest 的 skip 行会原样打出来），而不是静默消失。
 * 开发机上两条腿都跑；CI 上哈希腿单独在守——它已经覆盖了「来源」与「没人手改过」这两件事。
 *
 * # 这门抓不到什么（如实登记）
 *
 * - **好不好看**。它只保证位图 = SVG 的确定性渲染，不保证构图对。构图的推导（面 A 的实测数字 →
 *   面 B 的 108/72/66dp 规格）写在生成器头注与 `~/docs/polaris/design/polaris-android-launcher-icon-2026-09-04.md`，
 *   由 review 覆盖，不由本门覆盖。
 * - **CI 上的蓄意伪造**：同时改位图与 manifest，只有逐字节腿挡得住，而它在 CI 上跳过。
 * - **`#3DDC84` 那条否定断言的取材面只有 `res/` 树**（下面 `scanFace()` 自断言了这一点）。
 *   本文件自己写了这个字面量，但它住在 `ui/src/contracts/`，不在取材面内 —— 本仓踩过「判据被自己
 *   污染」的坑（断言字符串出现在自己的取材面里，删掉断言照样绿），故取材面显式收窄并加了正对照。
 */
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const REPO = fileURLToPath(new URL('../../../', import.meta.url));
const RES_REL = 'src-tauri/gen/android/app/src/main/res';
const RES = join(REPO, RES_REL);
const MANIFEST_REL = 'scripts/android-icons.manifest.json';
const GENERATOR_REL = 'scripts/gen-android-icons.mjs';
const APP_MANIFEST_REL = 'src-tauri/gen/android/app/src/main/AndroidManifest.xml';

/** 密度阶梯与 dp→px 惯例。**本表是门自己的期望值**，不从生成器 import（否则判据被产出方定义）。 */
const DENSITIES: ReadonlyArray<readonly [string, number]> = [
  ['mdpi', 1],
  ['hdpi', 1.5],
  ['xhdpi', 2],
  ['xxhdpi', 3],
  ['xxxhdpi', 4],
];
/** legacy 启动图标 48dp；自适应画布 108dp。 */
const LEGACY_DP = 48;
const ADAPTIVE_DP = 108;
/** Android Studio 模板的绿底。 */
const TEMPLATE_GREEN = '#3DDC84';

type IconManifest = {
  renderer: string;
  source: { path: string; sha256: string };
  files: Record<string, string>;
};

function walk(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? walk(join(dir, entry.name)) : [join(dir, entry.name)],
  );
}

function repoRel(absolute: string): string {
  return relative(REPO, absolute).split(sep).join('/');
}

function sha256(absolute: string): string {
  return createHash('sha256').update(readFileSync(absolute)).digest('hex');
}

function pngSize(rel: string): { width: number; height: number } {
  const buffer = readFileSync(join(REPO, rel));
  expect(buffer.subarray(0, 8).toString('hex'), `${rel} 不是 PNG`).toBe('89504e470d0a1a0a');
  return { width: buffer.readUInt32BE(16), height: buffer.readUInt32BE(20) };
}

/** 期望的位图清单：`[仓库相对路径, 期望边长]`。 */
function expectedBitmaps(basename: string, dp: number): Array<[string, number]> {
  return DENSITIES.map(([density, factor]): [string, number] => [
    `${RES_REL}/mipmap-${density}/${basename}.png`,
    dp * factor,
  ]);
}

const manifest = JSON.parse(readFileSync(join(REPO, MANIFEST_REL), 'utf8')) as IconManifest;

function localRendererVersion(): string | null {
  try {
    return execFileSync('rsvg-convert', ['--version'], { encoding: 'utf8' }).split('\n')[0].trim();
  } catch {
    return null;
  }
}
const localRenderer = localRendererVersion();
const byteExact = localRenderer !== null && localRenderer === manifest.renderer;

describe('Android 启动图标', () => {
  it('两份 anydpi-v26 都是 adaptive-icon，且 background/foreground/monochrome 三层引用都落在磁盘上', () => {
    for (const name of ['ic_launcher', 'ic_launcher_round']) {
      const rel = `${RES_REL}/mipmap-anydpi-v26/${name}.xml`;
      expect(existsSync(join(REPO, rel)), `${rel} 不存在 ⇒ Android 8+ 上没有自适应图标`).toBe(true);
      const xml = readFileSync(join(REPO, rel), 'utf8');
      expect(/<adaptive-icon\b/.test(xml), `${rel} 的根元素不是 <adaptive-icon>`).toBe(true);

      for (const layer of ['background', 'foreground', 'monochrome'] as const) {
        const matched = new RegExp(`<${layer}\\s+android:drawable="([^"]+)"`).exec(xml);
        expect(matched, `${rel} 缺 <${layer}> 层`).not.toBeNull();
        const ref = matched![1];

        if (ref.startsWith('@mipmap/')) {
          const base = ref.slice('@mipmap/'.length);
          const missing = DENSITIES.map(([density]) => `${RES_REL}/mipmap-${density}/${base}.png`).filter(
            (target) => !existsSync(join(REPO, target)),
          );
          expect(missing, `${rel} 的 <${layer}> 指向 ${ref}，但这些档缺位图`).toEqual([]);
        } else if (ref.startsWith('@color/')) {
          const colorName = ref.slice('@color/'.length);
          const declaring = readdirSync(RES, { withFileTypes: true })
            .filter((entry) => entry.isDirectory() && entry.name.startsWith('values'))
            .flatMap((entry) => walk(join(RES, entry.name)))
            .filter((file) => new RegExp(`<color\\s+name="${colorName}"`).test(readFileSync(file, 'utf8')));
          expect(declaring.map(repoRel), `${rel} 的 <${layer}> 指向 ${ref}，但没有 values/ 声明它`).not.toEqual([]);
        } else if (ref.startsWith('@drawable/')) {
          const base = ref.slice('@drawable/'.length);
          const hits = readdirSync(RES, { withFileTypes: true })
            .filter((entry) => entry.isDirectory() && entry.name.startsWith('drawable'))
            .flatMap((entry) => readdirSync(join(RES, entry.name)))
            .filter((file) => file.replace(/\.[^.]+$/, '') === base);
          expect(hits, `${rel} 的 <${layer}> 指向 ${ref}，但 drawable*/ 下没有它`).not.toEqual([]);
        } else {
          throw new Error(`${rel} 的 <${layer}> 引用形态无法解析：${ref}`);
        }
      }
    }
  });

  it('五档 legacy 方图/圆图齐全，像素尺寸精确等于 48dp 换算值', () => {
    const wrong = [...expectedBitmaps('ic_launcher', LEGACY_DP), ...expectedBitmaps('ic_launcher_round', LEGACY_DP)]
      .map(([rel, side]) => {
        if (!existsSync(join(REPO, rel))) return `${rel}: 缺失`;
        const { width, height } = pngSize(rel);
        return width === side && height === side ? null : `${rel}: ${width}×${height} ≠ ${side}×${side}`;
      })
      .filter((entry): entry is string => entry !== null);
    expect(wrong, 'API 24/25 没有自适应图标，这五档是唯一兜底').toEqual([]);
  });

  it('五档前景层/单色层齐全，像素尺寸精确等于 108dp 换算值', () => {
    const wrong = [
      ...expectedBitmaps('ic_launcher_foreground', ADAPTIVE_DP),
      ...expectedBitmaps('ic_launcher_monochrome', ADAPTIVE_DP),
    ]
      .map(([rel, side]) => {
        if (!existsSync(join(REPO, rel))) return `${rel}: 缺失`;
        const { width, height } = pngSize(rel);
        return width === side && height === side ? null : `${rel}: ${width}×${height} ≠ ${side}×${side}`;
      })
      .filter((entry): entry is string => entry !== null);
    expect(wrong).toEqual([]);
  });

  it('AndroidManifest 的 icon / roundIcon 指向这两份自适应图标', () => {
    const xml = readFileSync(join(REPO, APP_MANIFEST_REL), 'utf8');
    expect(/android:icon="@mipmap\/ic_launcher"/.test(xml), 'application 没有指向 @mipmap/ic_launcher').toBe(true);
    expect(
      /android:roundIcon="@mipmap\/ic_launcher_round"/.test(xml),
      'application 没有 roundIcon ⇒ 圆形启动器拿不到圆图，白做',
    ).toBe(true);
  });

  it('启动图标资源里不再出现模板绿（附取材面正对照）', () => {
    const face = walk(RES);
    expect(face.every((file) => file.startsWith(RES + sep)), '取材面越界了').toBe(true);
    expect(face.length, '取材面是空的 ⇒ 下面的否定断言没有信息量').toBeGreaterThan(20);

    const read = (file: string): string => readFileSync(file).toString('latin1');
    // 正对照：同一个扫描器必须能在取材面里找到确实存在的字符串，否则「没找到绿」只是扫了个寂寞。
    const white = face.filter((file) => read(file).includes('#FFFFFF'));
    expect(white.map(repoRel), '扫描器在取材面里找不到 #FFFFFF ⇒ 它根本没读到东西').toContain(
      `${RES_REL}/values/ic_launcher_background.xml`,
    );

    const green = face.filter((file) => read(file).toUpperCase().includes(TEMPLATE_GREEN));
    expect(green.map(repoRel), 'Android Studio 模板绿还在启动图标资源里').toEqual([]);
  });

  it('位图与 XML 全部出自生成脚本：manifest 逐个哈希对得上，且绑定当前品牌 SVG', () => {
    expect(sha256(join(REPO, manifest.source.path)), `${manifest.source.path} 变了但没重跑 ${GENERATOR_REL}`).toBe(
      manifest.source.sha256,
    );

    const onDisk = walk(RES)
      .map(repoRel)
      .filter((rel) => rel.split('/').pop()!.startsWith('ic_launcher'))
      .sort();
    expect(onDisk, 'res/ 里的 ic_launcher* 资源与 manifest 登记的集合不一致').toEqual(
      Object.keys(manifest.files).sort(),
    );

    const drifted = onDisk.filter((rel) => sha256(join(REPO, rel)) !== manifest.files[rel]);
    expect(drifted, `这些文件的内容不是 ${GENERATOR_REL} 产出的（被手改过，或改了源没重跑）`).toEqual([]);
  });

  it.skipIf(!byteExact)(
    byteExact
      ? '重跑生成脚本到临时目录，产物与工作树逐字节相等'
      : `重跑生成脚本到临时目录，产物与工作树逐字节相等【本机跳过：渲染器 ${localRenderer ?? '缺失'} ≠ manifest.renderer ${manifest.renderer}；本轮由哈希腿单独在守】`,
    () => {
      const root = existsSync('/var/tmp') ? '/var/tmp' : tmpdir();
      const out = mkdtempSync(join(root, 'polaris-icon-gate-'));
      try {
        execFileSync('node', [join(REPO, GENERATOR_REL), '--out', out], { stdio: 'pipe' });
        const produced = walk(out)
          .map((file) => relative(out, file).split(sep).join('/'))
          .sort();
        expect(produced.length, '生成脚本什么都没产出').toBeGreaterThan(20);
        expect(produced, '重跑产出的文件集合与 manifest 登记的不一致').toEqual(
          [...Object.keys(manifest.files), MANIFEST_REL].sort(),
        );
        const differing = produced.filter(
          (rel) => !readFileSync(join(out, rel)).equals(readFileSync(join(REPO, rel))),
        );
        expect(differing, '重跑产物与工作树逐字节不等 ⇒ 树里的图标不是当前 SVG + 当前脚本的产物').toEqual([]);
      } finally {
        rmSync(out, { recursive: true, force: true });
      }
    },
    180_000,
  );
});
