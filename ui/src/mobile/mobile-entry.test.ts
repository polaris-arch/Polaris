/**
 * `mobile-entry` —— 守住移动端入口的那道门。
 *
 * 落地设计：`~/docs/polaris/design/polaris-mobile-shell-2026-09-04.md`。
 * 相邻的门：`../styles/mobile-token-parity.test.ts` 验的是**入口文件的内容**对不对
 * （token 取值与桌面终值同一份）；本门验的是**入口这条链路**对不对 ——
 * 文档在不在、有没有人装载它、它的 CSS 有没有偷偷绕回桌面层叠链、五个目的地是不是一个不少、
 * 导航表与路由表是不是同一份。两道门的射程不重叠。
 *
 * ── 判据都带正面断言 ────────────────────────────────────────────────────────
 * 「不许出现 X」这种写法会被「什么都没发生」骗过：入口文件被删、模块图解析器坏掉、正则一个字母敲错，
 * 都会让纯否定式判据一路绿灯。故每一组都先有一条**自检**（取材面非空、量级合理），
 * 再有一条**正面等式**（集合恰等于，不是"包含"）。
 *
 * ── 为什么目的地表是 checked-in 快照而不是去读设计包 ────────────────────────
 * 真值源是设计交接包 `~/docs/polaris/design/mobile-kit/ui-manifest.json` 的
 * `productScope.primaryDestinations`。那份包**不在仓里**（设计资产留本地，CI 上不存在），
 * 门若去读它，本机绿、CI 上直接读不到文件 —— 一道只在某台机器上成立的门比没有更坏。
 * 故这里放一份快照 `REGISTERED`，与 `mobile-token-parity` 的 `BASELINE` 同一套做法：
 * **设计包改了目的地，就要同时改这里和 `destinations.ts`，两处对不上时本门红。**
 */

import { describe, it, expect } from 'vitest';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, relative, resolve as resolvePath } from 'node:path';

/** …/ui/ */
const UI_ROOT = fileURLToPath(new URL('../../', import.meta.url));
/** 仓根（`src-tauri` 与 `ui` 的共同父目录）。 */
const REPO_ROOT = resolvePath(UI_ROOT, '..');

const abs = (relToUi: string) => resolvePath(UI_ROOT, relToUi);
const read = (relToUi: string) => readFileSync(abs(relToUi), 'utf8');
/** 相对 ui/ 的 POSIX 风格路径，用于集合比较与人话报错。 */
const rel = (absPath: string) => relative(UI_ROOT, absPath).split('\\').join('/');

/**
 * 剥注释。**这一步不是洁癖**：本目录几乎每个文件的头注里都写着
 * `import './styles/index.css'`、`@import './tokens.css'` 这类反面例子 ——
 * 不剥就会把注释里的反例当成真实 import 抓进模块图，门于是对着自己的文档说明报红。
 *
 * ⚠️ **必须识别字符串**，不能用 `/\/\*[\s\S]*?\*\//g` 那种正则。实测踩过：`vite.config.ts` 里的
 * `ignored: ['**\/src-tauri/**']` 这个 glob 同时含 `*​/` 与 `/​*`，正则版会把它当成块注释起点，
 * 一路吃到文件末尾 —— 本门于是"找不到 `input` 块"，而真正的原因是判据把自己的取材面吃掉了。
 * `⓪` 那条自检用的正是这个真实输入。
 */
function strip(src: string): string {
  let out = '';
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    const next = src[i + 1];
    if (c === '/' && next === '*') {
      const end = src.indexOf('*/', i + 2);
      i = end === -1 ? src.length : end + 2;
    } else if (c === '/' && next === '/') {
      const end = src.indexOf('\n', i);
      i = end === -1 ? src.length : end;
    } else if (c === "'" || c === '"' || c === '`') {
      // 字符串整体原样保留：里面的 `/*` `//` 不是注释。
      let j = i + 1;
      while (j < src.length && src[j] !== c) j += src[j] === '\\' ? 2 : 1;
      out += src.slice(i, Math.min(j + 1, src.length));
      i = j + 1;
    } else {
      out += c;
      i += 1;
    }
  }
  return out;
}

// ── 被守的对象 ───────────────────────────────────────────────────────────────

/** 移动端文档（Vite 独立入口）。 */
const ENTRY_HTML = 'mobile.html';
/** 文档唯一加载的模块。 */
const ENTRY_MODULE = 'src/mobile/MobileMain.tsx';
/**
 * 契约 A1 允许出现在入口链路上的**全部** CSS，一个不多一个不少。
 *
 * 屏级样式各自成文件、各归各屏，并逐个登记在这里。两条理由：
 * 屏是**并行落地**的，让多条线往同一份 CSS 里写会把「谁的样式」变成一次持续的合并冲突；
 * 而外壳的 `mobile.css` 已定稿，屏不许往里塞。
 *
 * 判据形态**不放宽**：仍是集合**恰等**，加一份就要在这里显式记一笔 —— 不许改成
 * 「`src/mobile/` 下的都算」，那会让一份误加的 CSS 自动过关；也正因为是恰等，
 * 某一屏 import 回桌面层叠链时本门才能指出**是哪一份**多出来的，而不只是说「多了一份」。
 */
const ALLOWED_CSS = [
  'src/mobile/connections/connections.css',
  'src/mobile/connections/connections-redesign.css',
  /* 表单宿主（`mobile/forms/**`）是**应用级**的一层，不属于任何一屏 —— 它由 `MobileApp` 挂在
     外壳之外，节点屏与首页都开得动它。样式另成一份而不是并进 `mobile.css`：外壳那份已定稿，
     且屏不许往里塞（同上一条理由）。2026-09-06 批 2 登记。 */
  'src/mobile/forms/forms.css',
  'src/mobile/mobile.css',
  'src/mobile/redesign.css',
  // Explicit mobile dark redesign, isolated from the shared desktop token baseline.
  'src/mobile/theme.css',
  'src/mobile/home/home.css',
  'src/mobile/nodes/nodes.css',
  'src/mobile/screens/rules/rules-screen.css',
  'src/mobile/screens/rules/rules-redesign.css',
  'src/mobile/settings/settings.css',
  'src/styles/fonts.css',
  'src/styles/tokens.resolved.css',
];
/** 桌面层叠链的五个文件：任何一个出现在移动端入口链路上都是契约 A1 违约。 */
const DESKTOP_CASCADE = [
  'src/styles/index.css',
  'src/styles/tokens.css',
  'src/styles/components.css',
  'src/styles/screens.css',
  'src/styles/prototype.css',
];

/**
 * 五个主目的地的快照。真值源见文件头注。**顺序即导航从左到右的顺序**。
 */
const REGISTERED: ReadonlyArray<readonly [string, string]> = [
  ['home', '首页'],
  ['nodes', '节点'],
  ['rules', '分流'],
  ['connections', '活动'],
  ['settings', '设置'],
];

// ── 模块图走查 ───────────────────────────────────────────────────────────────

/** TS/TSX 的静态与动态 import（含 `export … from`），剥注释后取。 */
const TS_SPEC = /\bfrom\s*['"]([^'"]+)['"]|\bimport\s*\(\s*['"]([^'"]+)['"]|^[ \t]*import\s+['"]([^'"]+)['"]/gm;
/** CSS 的 `@import`。 */
const CSS_SPEC = /@import\s+['"]([^'"]+)['"]/g;

const specifiersOf = (src: string, isCss: boolean): string[] => {
  const out: string[] = [];
  const body = strip(src);
  if (isCss) {
    for (const m of body.matchAll(CSS_SPEC)) out.push(m[1]);
  } else {
    for (const m of body.matchAll(TS_SPEC)) out.push(m[1] ?? m[2] ?? m[3]);
  }
  return out;
};

/** `@/x` → `ui/src/x`；`./x` `../x` → 相对 importer；其余（react 等裸包）返回 null = 不入图。 */
const resolveSpec = (spec: string, importerAbs: string): string | null => {
  const base = spec.startsWith('@/')
    ? resolvePath(UI_ROOT, 'src', spec.slice(2))
    : spec.startsWith('.')
      ? resolvePath(dirname(importerAbs), spec)
      : null;
  if (base === null) return null;
  for (const cand of [
    base,
    `${base}.ts`,
    `${base}.tsx`,
    `${base}.css`,
    `${base}.json`,
    `${base}/index.ts`,
    `${base}/index.tsx`,
  ]) {
    if (existsSync(cand) && statSync(cand).isFile()) return cand;
  }
  throw new Error(`模块图走查解析不到 \`${spec}\`（由 ${rel(importerAbs)} 引入）—— 解析器与真实构建脱节了`);
};

/** 从入口模块 BFS 出整张图（只含仓内文件；裸包不入图）。 */
function moduleGraph(): string[] {
  const seen = new Set<string>();
  const queue = [abs(ENTRY_MODULE)];
  while (queue.length > 0) {
    const cur = queue.shift() as string;
    if (seen.has(cur)) continue;
    seen.add(cur);
    // 只有源码与 CSS 会再引出边；`.json`（语言包）是叶子，按 TS 解析它只会读出噪声。
    if (!/\.(?:tsx?|css)$/.test(cur)) continue;
    for (const spec of specifiersOf(readFileSync(cur, 'utf8'), cur.endsWith('.css'))) {
      const next = resolveSpec(spec, cur);
      if (next !== null && !seen.has(next)) queue.push(next);
    }
  }
  return [...seen].map(rel).sort();
}

const GRAPH = moduleGraph();
const GRAPH_CSS = GRAPH.filter((p) => p.endsWith('.css'));

// ─────────────────────────────────────────────────────────────────────────────

describe('⓪ 自检：判据的取材面是活的（否则下面每条都恒绿）', () => {
  it('剥注释真的把注释里的反面示例挡在图外', () => {
    // 头注里写反例是本目录的常态；不剥就会把它们抓成真实依赖。
    const synthetic = `/* 反例：import './styles/index.css' 会违约 */\nimport { a } from './real';\n`;
    expect(specifiersOf(synthetic, false)).toEqual(['./real']);
    expect(specifiersOf(`/* @import './tokens.css' */\n@import './fonts.css';`, true)).toEqual(['./fonts.css']);
  });

  it('字符串里的 `/*` 不是注释起点（vite.config.ts 的 glob 是真实输入，正则版剥法在这里塌过）', () => {
    const glob = "const ignored = ['**/src-tauri/**'];\nimport { a } from './real';\n";
    expect(specifiersOf(glob, false)).toEqual(['./real']);
    expect(strip(glob)).toContain('src-tauri');
  });

  it('模块图非空且有量级（走查器塌了会退化成"什么都没扫到所以全绿"）', () => {
    expect(GRAPH, `模块图只有 ${GRAPH.length} 个文件 —— 走查器坏了`).toContain(ENTRY_MODULE);
    expect(GRAPH.length).toBeGreaterThan(5);
    expect(GRAPH_CSS.length).toBeGreaterThan(0);
  });
});

describe('① 入口文件在，且 Vite 真的在构建它', () => {
  it(`\`ui/${ENTRY_HTML}\` 存在且非空`, () => {
    expect(existsSync(abs(ENTRY_HTML)), `${ENTRY_HTML} 不见了 —— 移动端没有文档可加载`).toBe(true);
    expect(read(ENTRY_HTML).trim().length).toBeGreaterThan(0);
  });

  it('文档只加载移动端入口模块这一个脚本', () => {
    const srcs = [...read(ENTRY_HTML).matchAll(/<script[^>]*\bsrc=['"]([^'"]+)['"]/g)].map((m) => m[1]);
    expect(srcs, `${ENTRY_HTML} 的脚本集合变了`).toEqual([`/${ENTRY_MODULE}`]);
  });

  it('`viewport-fit=cover` 在 —— 少了它四个 `--safe-*` 会静默恒为 0', () => {
    const viewport = /<meta\s+name=['"]viewport['"][^>]*content=['"]([^'"]*)['"]/.exec(read(ENTRY_HTML));
    expect(viewport, `${ENTRY_HTML} 里没有 viewport meta`).not.toBeNull();
    expect(viewport?.[1]).toContain('viewport-fit=cover');
  });

  it('移动 WebView 固定页面倍率，同时保留设备宽度和安全区', () => {
    const tags = [...read(ENTRY_HTML).matchAll(/<meta\s+name=['"]viewport['"][^>]*content=['"]([^'"]*)['"]/g)];
    expect(tags, '页面必须只有一个 viewport meta，否则 WebView 的实际倍率取决于解析顺序').toHaveLength(1);
    const directives = Object.fromEntries(tags[0][1].split(',').map((part) => {
      const [key, value] = part.trim().split('=');
      return [key, value];
    }));
    expect(directives).toMatchObject({
      width: 'device-width',
      'initial-scale': '1.0',
      'maximum-scale': '1.0',
      'user-scalable': 'no',
      'viewport-fit': 'cover',
    });
  });

  it('`vite.config.ts` 把它登记成了构建入口（不登记 = 打包产物里根本没有这份文档）', () => {
    const input = /input:\s*\{([\s\S]*?)\}/.exec(strip(read('vite.config.ts')));
    expect(input, 'vite.config.ts 的 rollupOptions.input 块找不到了 —— 判据面塌了').not.toBeNull();
    expect(input?.[1]).toMatch(new RegExp(`mobile:[^\\n]*['"]${ENTRY_HTML.replace('.', '\\.')}['"]`));
  });
});

describe('② 装载面：Android / iOS 上真的有人打开这份文档', () => {
  /**
   * 「入口在」与「入口被加载」是两件事。`WindowConfig::url` 缺省是 `index.html`（桌面入口），
   * 移动端要换成本文档，只有 `create_main_window` 里那一行 `#[cfg(mobile)]` 覆写做得到
   * （平台 conf 那条路被 `verify-packaging.mjs confs` 的顶层键集合判据封死，见 lib.rs 注释）。
   * 少了那行，前端这边一切"看起来对"，真机上打开的却是桌面文档 —— 门在但没牙的典型形态。
   *
   * # 本判据的射程为什么**已经**覆盖 iOS（2026-09-06 复核，带收据）
   *
   * 它绑的是 `#[cfg(mobile)]` 这个字面写法，而 `mobile` 是 Tauri 的 cfg 别名，定义在
   * `tauri-build` 的构建脚本里：`let mobile = target_os == "ios" || target_os == "android";`
   * （`tauri-build-2.6.3/src/lib.rs:475-477`，版本由本仓 `Cargo.lock` 钉死）。
   * ⇒ iOS 上同样走 `mobile.html`，**零改动**。
   *
   * 这也是本仓「前端平台探测的单一真值」的所在：`ui/src/mobile/` 里没有任何一处读平台，
   * 平台判断整个发生在编译期的 Rust 侧、就是上面那一行。iOS 的接入点因此不是「加一条
   * `'ios'` 分支」，而是「决定移动包里那些硬编码的 Android 事实哪些对 iOS 也成立」——
   * 那批判断不在本门射程内，见本批报告。
   *
   * ⚠️ **未验证**：上面是源码级事实，不是产物级收据 —— 本仓在 Linux 上构不出 iOS 产物，
   * 「iOS 真机上打开的确实是 mobile.html」没有实测过。
   */
  it('`create_main_window` 里存在 cfg(mobile) 的 url 覆写，且指向同一份文档', () => {
    const libRs = readFileSync(resolvePath(REPO_ROOT, 'src-tauri/src/lib.rs'), 'utf8');
    const fnStart = libRs.indexOf('fn create_main_window(');
    expect(fnStart, '`create_main_window` 不见了 —— 判据面塌了（改名了？）').toBeGreaterThan(-1);
    const body = libRs.slice(fnStart);
    const override = /#\[cfg\(mobile\)\][\s\S]{0,400}?WebviewUrl::App\(\s*"([^"]+)"/.exec(body);
    expect(
      override,
      '`create_main_window` 里找不到 `#[cfg(mobile)]` 的 `WebviewUrl::App(...)` 覆写 —— ' +
        '移动端会去加载缺省的 index.html（桌面入口，带整条桌面 CSS 层叠链）',
    ).not.toBeNull();
    expect(override?.[1], '移动端装载的文档名与前端入口对不上').toBe(ENTRY_HTML);
  });
});

describe('③ 契约 A1：入口链路上只许有移动端那一份 CSS', () => {
  it('模块图里的 CSS 集合恰等于允许集（多一份少一份都红）', () => {
    expect(GRAPH_CSS.sort()).toEqual([...ALLOWED_CSS].sort());
  });

  it('桌面五层层叠链一个都不在图上（与上一条互为人话版，报错时直接指名道姓）', () => {
    for (const desktop of DESKTOP_CASCADE) {
      expect(
        GRAPH,
        `移动端入口链路上出现了桌面层叠链文件 ${desktop} —— 契约 A1 违约：` +
          'token 终值会被中间层污染（语义色丢无障碍校准），且整条桌面组件 CSS 进移动包',
      ).not.toContain(desktop);
    }
  });

  it('入口 CSS 自身不写任何 @import（唯一那条在 tokens.resolved.css 里，指向随包字体面）', () => {
    expect(specifiersOf(read('src/mobile/mobile.css'), true)).toEqual([]);
    expect(specifiersOf(read('src/styles/tokens.resolved.css'), true)).toEqual(['./fonts.css']);
  });
});

describe('④ 五个目的地一个不少，且与设计包的登记表逐值相等', () => {
  const table = [...strip(read('src/mobile/destinations.ts')).matchAll(
    /\{\s*id:\s*'([a-z]+)'\s*,\s*labelKey:\s*'([^']+)'\s*\}/g,
  )].map((m) => [m[1], m[2]] as const);

  /** 从 zh-CN 语言包按 `a.b` 路径取值。取不到返回 undefined ⇒ 下面那条会指名道姓报出来。 */
  const zhCn = JSON.parse(read('src/i18n/locales/zh-CN.json')) as Record<string, unknown>;
  const lookup = (key: string): unknown =>
    key.split('.').reduce<unknown>((cur, seg) => (cur as Record<string, unknown>)?.[seg], zhCn);

  it('自检：`DESTINATIONS` 真的解析出来了（正则失配会让下面那条对着空数组恒绿）', () => {
    expect(table.length, 'destinations.ts 里一条目的地都没解析出来 —— 表的写法变了，判据面塌了').toBe(
      REGISTERED.length,
    );
  });

  it('id 逐值逐序等于登记表', () => {
    expect(table.map(([id]) => id)).toEqual(REGISTERED.map(([id]) => id));
  });

  it('每个 labelKey 在 zh-CN 里解析出的文案 = 设计登记表的原文（三方对齐：登记表 / 代码 / locale）', () => {
    expect(table.map(([, key]) => lookup(key))).toEqual(REGISTERED.map(([, label]) => label));
  });

  it('五语种都有 `mobileNav`，且键集合一致（只加中文 = 另外四语种回落成 key）', () => {
    const keys = REGISTERED.map(([id]) => id).sort();
    for (const lang of ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa']) {
      const ns = (JSON.parse(read(`src/i18n/locales/${lang}.json`)) as Record<string, unknown>)['mobileNav'];
      expect(ns, `${lang}.json 缺 mobileNav 命名空间`).toBeDefined();
      expect(Object.keys(ns as object).sort(), `${lang}.json 的 mobileNav 键集合与目的地对不上`).toEqual(keys);
    }
  });

  it('IA 裁定 #7：不得出现「按应用分流」的目的地或任何暗示它以后会有的文案', () => {
    // 取材面**两块**：外壳源码里的可见字面量（剥注释后，故代码注释不算文案），
    // 以及五语种 `mobileNav` 的取值 —— 标签自从进了 locale，文案面就有一半在那边。
    const code = [
      'destinations.ts',
      'Screens.tsx',
      'BottomNavigation.tsx',
      'MobileApp.tsx',
      'MobileShell.tsx',
      // 屏级源码同样在面内：裁定 #7 说的是「UI 上不出现、文案不得暗示」，那句话的射程是整个
      // 移动端，不是外壳五个文件。每落一个真屏就把它的文件加进来。
      'nodes/MobileNodesScreen.tsx',
      'nodes/NodesScreenView.tsx',
      'nodes/view-model.ts',
      'nodes/absence-register.ts',
      'connections/MobileConnectionsScreen.tsx',
      'connections/ConnectionsView.tsx',
      'connections/view-model.ts',
      // 表单宿主（批 2）：裁定 #7 的射程是整个移动端，表单里的文案同样不许暗示按应用分流。
      'forms/form-store.ts',
      'forms/MobileFormHost.tsx',
      'forms/FormSheet.tsx',
      'forms/FormFields.tsx',
      'forms/NodeFormPanel.tsx',
      'forms/SubFormPanel.tsx',
      'forms/ImportFormPanel.tsx',
      'forms/MeshJoinPanel.tsx',
      'forms/TsExitPanel.tsx',
      'forms/ConfirmPanel.tsx',
    ]
      .map((f) => strip(read(`src/mobile/${f}`)))
      .join('\n');
    const copy = ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa']
      .map((lang) => JSON.stringify((JSON.parse(read(`src/i18n/locales/${lang}.json`)) as Record<string, unknown>)['mobileNav']))
      .join('\n');
    const surface = `${code}\n${copy}`;
    for (const banned of ['app-routing', '按应用分流', '哪些应用', 'App Policy', '应用分流']) {
      expect(surface, `移动端外壳出现了「${banned}」—— 裁定 #7 定为不做、UI 上不出现、文案不得暗示`).not.toContain(
        banned,
      );
    }
  });
});

describe('⑤ 导航表 / 图标表 / 路由表同源', () => {
  const ids = REGISTERED.map(([id]) => id);
  const keysOf = (file: string, mapName: string): string[] => {
    const src = strip(read(`src/mobile/${file}`));
    const start = src.indexOf(mapName);
    expect(start, `${file} 里找不到 \`${mapName}\` —— 判据面塌了`).toBeGreaterThan(-1);
    const block = /\{([\s\S]*?)\n\};/.exec(src.slice(start));
    expect(block, `${file} 的 \`${mapName}\` 块解析不出来`).not.toBeNull();
    return [...(block?.[1] ?? '').matchAll(/^\s{2}([a-z]+):/gm)].map((m) => m[1]).sort();
  };

  it('`Screens.tsx` 的 `SCREENS` 键集合 = 五个目的地（少一屏 = 有目的地进不去）', () => {
    expect(keysOf('Screens.tsx', 'SCREENS')).toEqual([...ids].sort());
  });

  it('`BottomNavigation.tsx` 的 `ICONS` 键集合 = 五个目的地（少一个 = 导航项没图标）', () => {
    expect(keysOf('BottomNavigation.tsx', 'ICONS')).toEqual([...ids].sort());
  });

  it('导航与根组件都不自带第二份标签表（两张表是这类外壳最典型的静默漂移）', () => {
    for (const file of ['BottomNavigation.tsx', 'MobileApp.tsx']) {
      const src = strip(read(`src/mobile/${file}`));
      for (const [, label] of REGISTERED) {
        expect(src, `${file} 里出现了目的地标签字面量「${label}」—— 标签只许来自 destinations.ts`).not.toContain(
          label,
        );
      }
    }
    // 正面：导航确实是**读**那张表画出来的，而不是恰好没写字面量。
    expect(strip(read('src/mobile/BottomNavigation.tsx'))).toMatch(/DESTINATIONS\.map\(/);
  });
});

/**
 * ⑥ 外壳 CSS 不许留下"没有消费者却仍在生效"的类。
 *
 * 这条不是洁癖，它有一次真实事故：五个屏落地后占位屏被删，`mobile.css` 里的 `.m-section`
 * 留了下来，而节点屏根元素还挂着 `className="m-section mn"`。两者同为单类选择器 ⇒ 同特异度 ⇒
 * **谁靠后谁赢**，而 `MobileMain.tsx` 里 `./MobileApp`（连带各屏 CSS）先于 `./mobile.css` 求值，
 * 于是 `nodes.css` 那条"取消卡片外观"的 `.mn` 输掉，整个节点屏被套进一张卡：内容顶到状态栏根下、
 * 满幅吸顶块抵不掉卡内边距。2026-09-05 首轮真机验收才看见，而此前 8/8 静态门全绿。
 *
 * 判据形态选「死类」而不是「级联顺序」是刻意的：顺序正确与否要看打包产物，而**一条外壳规则只要
 * 还有人挂着，它就不是死的**；真正能自动判定的不变式是"外壳 CSS 的每个类都得有 .tsx 消费它"。
 * 消费面取模块图（本文件已有的 `GRAPH`），不是目录树 —— 图外的文件本来就进不了包。
 */
describe('⑥ 外壳 CSS 没有无人消费的类（死规则不会变成噪声，它会安静地赢下级联）', () => {
  const SHELL_CSS = 'src/mobile/mobile.css';

  /** 只扫「`{` 之前」那段选择器：声明体里的 `.5rem` 之类不是类名。 */
  const classSelectorsOf = (css: string): string[] => {
    const out = new Set<string>();
    for (const rule of strip(css).matchAll(/(?:^|[{};])\s*([^{};@][^{}]*)\{/g)) {
      for (const cls of rule[1].matchAll(/\.(-?[_a-zA-Z][\w-]*)/g)) out.add(cls[1]);
    }
    return [...out].sort();
  };

  const SHELL_CLASSES = classSelectorsOf(read(SHELL_CSS));
  /** 消费面 = 模块图里的 TS/TSX，剥注释（头注里写满了类名，不剥会把注释当消费）。 */
  const CONSUMERS = GRAPH.filter((p) => /\.tsx?$/.test(p))
    .map((p) => `${p}\n${strip(read(p))}`)
    .join('\n');
  /** `m-nav` 是 `m-nav-item` 的前缀，`\b` 会误判 —— 边界必须排除 `-`。 */
  const usedIn = (cls: string, hay: string) => new RegExp(`(?<![\\w-])${cls}(?![\\w-])`).test(hay);

  it('自检：类名与消费面都真的解析出来了（任一为空会让下面那条恒绿）', () => {
    expect(SHELL_CLASSES.length, `${SHELL_CSS} 里一个类选择器都没扫到 —— 判据面塌了`).toBeGreaterThan(5);
    expect(CONSUMERS.length).toBeGreaterThan(1000);
    // 双向对照：真在的找得到，不在的找不出来（只做前者会被"永远返回 true"的搜索骗过）。
    expect(usedIn('m-nav-item', CONSUMERS)).toBe(true);
    expect(usedIn('m-section-that-never-existed', CONSUMERS)).toBe(false);
    // 前缀不算消费：`m-nav-item` 在场不等于 `m-nav-item-x` 在场。
    expect(usedIn('m-nav-item-x', CONSUMERS)).toBe(false);
  });

  it('每个外壳类都有 .tsx 挂它', () => {
    const orphans = SHELL_CLASSES.filter((c) => !usedIn(c, CONSUMERS));
    expect(
      orphans,
      `${SHELL_CSS} 里这些类没有任何 .tsx 消费：${orphans.join(', ')} —— ` +
        '外壳规则与屏规则同特异度，无人消费的外壳类不是死代码，而是一条会按进包顺序' +
        '压过屏样式的活规则（2026-09-05 `.m-section` 把整个节点屏套进了卡片）',
    ).toEqual([]);
  });
});

/**
 * ⑦ 安全区的**生产端**在原生侧，且四个方向一起接管。
 *
 * `tokens.resolved.css` 那四条 `env(safe-area-inset-*)` 在 Android 上**不是真值源**：
 * WebView 只把 display cutout 映射成 `env()`，系统栏一律不映射。2026-09-05 Pixel 6 实测——
 * 手势条 `navigationBars` = 63 设备像素，而 `env(safe-area-inset-bottom)` 恒 `0px`，
 * 底部导航因此被压在手势条底下（五个屏全中）。顶部之所以"看起来是对的"纯属巧合：
 * 那台机器的挖孔高度恰好等于状态栏高度；换一台没有 cutout 的机器，`--safe-t` 同样是 0。
 *
 * 故判据要的不是"底部有人补了一下"，而是**四个方向都由 `WindowInsetsCompat` 供值**，
 * 且两条投递通道都在（当前文档 + 今后每次文档加载 —— 后者管冷启动竞态与
 * `renderer-recovery` 的 `location.reload()`，少了它安全区会在一次自愈重载后永久消失）。
 */
describe('⑦ Android 把系统栏 inset 送进了 web 层（`env()` 在这里只是回落）', () => {
  const MAIN_ACTIVITY = 'src-tauri/gen/android/app/src/main/java/com/polaris2/app/MainActivity.kt';
  const KT_RAW = readFileSync(resolvePath(REPO_ROOT, MAIN_ACTIVITY), 'utf8');
  /**
   * **必须剥注释**，否则判据被文件自己的文档喂饱：那份头注逐条讲了这里断言的每一个 API 名。
   * 实测（写完本节后的第一次变异）：把 `addDocumentStartJavaScript(...)` 那一行整个换掉，
   * 门**照样绿** —— 它读到的是头注里那句解释。`strip()` 与本文件其余判据同一份（它按字符串
   * 状态机剥，Kotlin 的行注释与块注释与 JS 同形）。
   */
  const KT = strip(KT_RAW);

  it('自检：Kotlin 源读得到、剥完注释仍有量级，且注释真的被剥掉了', () => {
    expect(KT_RAW.length, `${MAIN_ACTIVITY} 读到的是空文件 —— 判据面塌了`).toBeGreaterThan(400);
    expect(KT, '剥注释后什么都不剩 —— 剥法把代码一起吃了').toContain('class MainActivity');
    // 正面对照：头注里有、代码里没有的字样，剥完必须消失（否则下面每条都可能是注释在充数）。
    expect(KT_RAW).toContain('navigationBars');
    expect(KT, '`navigationBars` 只出现在头注里，剥完还在 = 注释没剥干净').not.toContain(
      'navigationBars',
    );
  });

  it('值来自 `WindowInsetsCompat` 的系统栏（只吃 cutout 等于没修）', () => {
    expect(KT, '没有装 inset 监听 —— 原生侧根本没有值可送').toContain('setOnApplyWindowInsetsListener');
    expect(
      KT,
      '取的不是 `WindowInsetsCompat.Type.systemBars()` —— 手势条/状态栏的 inset 只有它给得出，' +
        'cutout 那份 WebView 自己已经通过 env() 拿到了，再抄一遍等于没修',
    ).toMatch(/WindowInsetsCompat\.Type\.systemBars\(\)/);
  });

  it('四个方向一个不少地写进根元素（只补 bottom = 无 cutout 的机器上顶部照样被压）', () => {
    for (const token of ['--safe-t', '--safe-r', '--safe-b', '--safe-l']) {
      expect(KT, `${MAIN_ACTIVITY} 没有写 \`${token}\` —— 该方向仍然只剩 env() 那条回落`).toContain(
        `'${token}'`,
      );
    }
  });

  it('两条投递通道都在（少了文档起始那条，一次自愈重载后安全区永久消失）', () => {
    expect(KT, '缺 `evaluateJavascript`：运行期 inset 变化（转屏/切三键导航）打不到已加载的文档').toContain(
      'evaluateJavascript',
    );
    expect(
      KT,
      '缺 `addDocumentStartJavaScript`：冷启动时 URL 还没开始加载，且 `renderer-recovery` 的 ' +
        '`location.reload()` 会连内联样式一起丢掉，安全区从此不再回来',
    ).toContain('addDocumentStartJavaScript');
  });

  it('CSS 侧仍保留 `env()` 回落（iOS / 浏览器直开没有原生生产端）', () => {
    const tokens = read('src/styles/tokens.resolved.css');
    for (const [name, inset] of [
      ['--safe-t', 'top'],
      ['--safe-r', 'right'],
      ['--safe-b', 'bottom'],
      ['--safe-l', 'left'],
    ] as const) {
      expect(tokens, `${name} 的 env() 回落被删了 —— 非 Android 端会恒 0`).toMatch(
        new RegExp(`${name}:\\s*env\\(safe-area-inset-${inset}`),
      );
    }
  });
});

/**
 * ⑧ Dynamic Type 的**生产端**在原生侧，且三种时机一个不少。
 *
 * # 这一节补的是一个"接缝在、生产端整段不存在"的洞
 *
 * `mobile.css` 从第一版起就有 `:root{--font-scale:1}` 与
 * `html{font-size:calc(16px * var(--font-scale))}` —— 接缝、缺省、消费端全都在，文档里也写着
 * "支持 Dynamic Type"。缺的只有一样：**没有任何 Android 代码写这个变量**。
 * 用户在系统设置里把字号调到 1.3x / 2.0x，本应用一动不动，而当时全部静态门绿。
 * 那正是「登记表里没有 = 门看不见」的又一次：没有人问过"这个变量有没有生产端"。
 *
 * # 判据为什么要**按函数体切段**，不能对整份文件 `toContain`
 *
 * ⑦ 已经断言过 `evaluateJavascript` 与 `addDocumentStartJavaScript` 出现在本文件里 ——
 * 那是**安全区**那条路径贡献的。若本节也只对整份文件 `toContain`，那么把字号那条的两条通道
 * 全删掉、只留一句 `resources.configuration.fontScale` 的读取，本节照样绿：两扇门中间的缝
 * 恰好就是"字号有没有真的被投递出去"。故这里按 Kotlin 成员切段，逐段判，
 * 并用一条**反向对照**（安全区那段里不许出现 fontScale）证明切段真的分得开。
 *
 * # 三种时机（缺一条就是"今天看着好、明天悄悄失效"）
 *
 *  · **冷启动** —— `onWebViewCreate`。此刻 `evaluateJavascript` 多半打空（URL 还没开始加载，
 *    见 `MainActivity.kt` 头注），真正兑现的是同一次调用里注册的文档起始脚本。
 *  · **运行期变化** —— 用户切到系统设置改字号再切回来。2026-09-05 模拟器实测：本 Activity 的
 *    `configChanges` 不含 `fontScale` ⇒ 系统**重建** Activity（logcat 的
 *    `finishDrawing of relaunch`），走的是冷启动那条；`onResume` 与 `onConfigurationChanged`
 *    是"重建没发生"时的两条兜底。三条都要在：它们各自对应一种真实的到达方式。
 *  · **重载** —— `renderer-recovery` 的 `location.reload()` 会把内联样式连同文档一起丢掉，
 *    只有 `addDocumentStartJavaScript` 那条能让值在重载后回来。
 */
describe('⑧ Android 把系统字号缩放送进了 web 层（`--font-scale` 的生产端）', () => {
  const MAIN_ACTIVITY = 'src-tauri/gen/android/app/src/main/java/com/polaris2/app/MainActivity.kt';
  const KT_RAW = readFileSync(resolvePath(REPO_ROOT, MAIN_ACTIVITY), 'utf8');
  /** 与 ⑦ 同一份剥法：那份头注逐条讲了这里断言的每一个 API 名，不剥就是判据被文档喂饱。 */
  const KT = strip(KT_RAW);

  /**
   * 取一个 Kotlin 成员的**源码段**：从 `fun <name>(` 起，到下一个 `fun` 声明为止。
   * 找不到返回空串 —— 下面每条都会因为空串而红，而不是因为"整份文件里有这个字样"而绿。
   */
  const member = (name: string): string => {
    const at = KT.search(new RegExp(`\\bfun\\s+${name}\\s*\\(`));
    if (at < 0) return '';
    const rest = KT.slice(at + 1);
    const next = rest.search(/\n\s*(?:private\s+|override\s+|internal\s+|public\s+)*fun\s/);
    return next < 0 ? rest : rest.slice(0, next);
  };

  const PUBLISH = member('publishFontScale');
  const SCRIPT = member('fontScaleScript');

  it('自检：切段真的把成员分开了（切成整份文件 = 退回 ⑦ 那种"文件里出现过"的弱判据）', () => {
    expect(KT_RAW.length, `${MAIN_ACTIVITY} 读到的是空文件 —— 判据面塌了`).toBeGreaterThan(400);
    expect(KT, '剥注释后什么都不剩').toContain('class MainActivity');
    // 剥干净了：`configChanges` 这个字样只出现在 KDoc 里。
    expect(KT_RAW).toContain('configChanges');
    expect(KT, '`configChanges` 只在注释里，剥完还在 = 注释没剥干净').not.toContain('configChanges');
    // 切段非空、且远小于整份文件。
    expect(PUBLISH.length, '`publishFontScale` 切不出来 —— 函数改名了？').toBeGreaterThan(80);
    expect(SCRIPT.length, '`fontScaleScript` 切不出来').toBeGreaterThan(60);
    expect(PUBLISH.length).toBeLessThan(KT.length / 2);
    // 反向对照：不存在的成员切出空串（切段不是"永远返回整份文件"）。
    expect(member('publishSomethingThatNeverExisted')).toBe('');
    // 反向对照之二：安全区那段里没有 fontScale，字号那段里没有 --safe-* ——
    // 两条量真的被分开了，下面的断言不可能被隔壁那条满足。
    expect(member('publishSafeArea')).not.toContain('fontScale');
    expect(PUBLISH + SCRIPT).not.toContain('--safe-');
  });

  it('取值面是 `Configuration.fontScale`（读别的等于没读系统字号）', () => {
    expect(
      PUBLISH,
      '`publishFontScale` 里没有 `resources.configuration.fontScale` —— ' +
        '系统字号的真值只有它给得出，前端拿不到任何等价物',
    ).toContain('resources.configuration.fontScale');
  });

  /**
   * 引擎那份必须同时关掉，否则两套缩放**相乘**。
   *
   * 2026-09-05 实测（Android 16 / WebView Chrome 133）：`WebSettings.textZoom` 缺省不是 100，
   * 而是由 WebView 从 `Configuration.fontScale` 初始化 —— 不碰它时 `html{font-size:16px}` 在
   * 1.3x 下就已经算出 20.8px。于是本类再报一次 `--font-scale:1.300` 后根字号变成
   * **27.04px = 16 × 1.3 × 1.3**，那不是修复是回归（这一条判据就是为那次实测写的）。
   *
   * 判据要的是**恰好 100**：`textZoom = round(100 / scale)` 那种"倒数抵消"写法会留下舍入残差
   * （1.3x 下实测 16.016px 而不是 16px），而 100 是精确的。
   */
  it('同一次调用里把引擎自带的那套关掉（少了它 1.3x 下是 16×1.3×1.3，不是 16×1.3）', () => {
    expect(
      PUBLISH,
      '`publishFontScale` 里没有 `textZoom` —— WebView 缺省已经按系统字号缩过一次，' +
        '再报 `--font-scale` 等于相乘：1.3x 下根字号 27.04px，2.0x 下 64px',
    ).toContain('textZoom');
    expect(
      PUBLISH.replace(/\s+/g, ' '),
      '`textZoom` 不是钉成 100 —— 倒数抵消（`100 / scale`）会留舍入残差，' +
        '实测 1.3x 下算出 16.016px；100 是精确的 1.0 倍',
    ).toMatch(/textZoom\s*=\s*100\b/);
  });

  it('写的是 `--font-scale`，且载荷是**无单位数**（写成 `1.300px` 会让整条 calc() 静默失效）', () => {
    expect(SCRIPT, '脚本里没写 `--font-scale`').toContain("'--font-scale'");
    expect(SCRIPT, '不是写在根元素上 —— 写到别的元素上 `html{font-size}` 读不到').toContain(
      'document.documentElement.style',
    );
    // `html{font-size:calc(16px * var(--font-scale))}` 要的是纯数。
    expect(SCRIPT, '格式串里带了单位').not.toMatch(/%\.\d+f\s*px/);
    expect(SCRIPT).toMatch(/"%\.\d+f"/);
  });

  it('`Locale.US` 钉住（跟随系统语言时 ru/fr 下会给出 `1,300`，CSS 当场解析失败）', () => {
    expect(
      SCRIPT,
      '`String.format` 没钉 `Locale.US` —— 逗号小数点让 `--font-scale` 静默退回缺省 1，' +
        '一个只在部分语言下复现的"字号不生效"（与 `cssPx` 同一个坑）',
    ).toContain('Locale.US');
  });

  it('两条投递通道都在**字号这条路径上**（⑦ 那两条是安全区贡献的，射程不重叠）', () => {
    expect(PUBLISH, '缺 `evaluateJavascript`：运行期字号变化打不到已加载的文档').toContain(
      'evaluateJavascript',
    );
    expect(
      PUBLISH,
      '缺 `addDocumentStartJavaScript`：冷启动时 URL 还没开始加载、`renderer-recovery` 的 ' +
        '`location.reload()` 又会把内联样式丢掉 —— 字号会在一次自愈重载后永久回到 1x',
    ).toContain('addDocumentStartJavaScript');
    // 旧脚本要撤：不撤的话每报一次就多注册一条，同一份文档被灌进多组值，
    // 且最终生效的是**哪一条**取决于注册顺序 —— 一个只在改过几次字号后才复现的错值。
    expect(PUBLISH, '没有撤销上一条文档起始脚本').toMatch(/fontScaleScript\?\.remove\(\)/);
    // 两条量各留各的句柄：共用一个会让任一方的变化把另一方的值一起重写。
    expect(PUBLISH, '字号复用了安全区那条句柄').not.toContain('safeAreaScript');
  });

  it('三种时机一个不少：冷启动 / 回前台 / configuration 回调都报一次', () => {
    for (const [hook, why] of [
      ['onWebViewCreate', '冷启动：WebView 刚建好那一次'],
      ['onResume', '用户去系统设置改完字号切回来（Activity 未被重建的那些路径）'],
      [
        'onConfigurationChanged',
        '`configChanges` 清单一旦加上 `fontScale`，重建路径消失，' +
          '这条是那时唯一还会触发的；今天它对转屏/深浅色就在跑，不是死代码',
      ],
    ] as const) {
      const body = member(hook);
      expect(body.length, `${hook} 不见了 —— ${why}`).toBeGreaterThan(20);
      expect(body, `${hook} 里没有调用 \`publishFontScale\` —— 缺的时机是「${why}」`).toContain(
        'publishFontScale(',
      );
    }
  });

  it('CSS 侧的缺省仍在（iOS / 浏览器直开没有原生生产端，缺了它那边根字号会算不出来）', () => {
    expect(read('src/mobile/mobile.css')).toMatch(/--font-scale:\s*1\s*;/);
    expect(read('src/mobile/mobile.css')).toMatch(/font-size:\s*calc\(\s*16px\s*\*\s*var\(--font-scale\)\s*\)/);
  });
});

describe('⑨ Android WebView 页面缩放与系统字号分离', () => {
  const path = resolvePath(REPO_ROOT, 'src-tauri/gen/android/app/src/main/java/com/polaris2/app/MainActivity.kt');
  const activity = strip(readFileSync(path, 'utf8'));
  const start = activity.indexOf('override fun onWebViewCreate(webView: WebView)');
  const hook = start < 0 ? '' : activity.slice(start).split(/\n\s*override fun /)[0];

  it('在 WebView 创建时关闭手势及内建缩放，不拦截触摸、滚动和输入', () => {
    expect(activity).toMatch(/class\s+MainActivity\s*:\s*TauriActivity\s*\(\s*\)/);
    expect(start, 'onWebViewCreate 不存在，原生 WebView 缩放设置无处生效').toBeGreaterThan(-1);
    expect(hook).toMatch(/webView\.settings\.apply\s*\{\s*setSupportZoom\(false\)\s*setBuiltInZoomControls\(false\)\s*\}/);
    expect(hook).not.toMatch(/setOnTouchListener|onTouchEvent|requestDisallowInterceptTouchEvent/);
  });

  it('仍由原有的字号通道消费系统大字体', () => {
    expect(hook).toContain('publishFontScale(webView)');
    expect(read('src/mobile/mobile.css')).toMatch(/font-size:\s*calc\(\s*16px\s*\*\s*var\(--font-scale\)\s*\)/);
  });
});
