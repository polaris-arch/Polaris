/**
 * 系统栏明暗跟随 **Polaris 生效主题** —— 这条腿的门。
 *
 * 规格：`~/docs/polaris/design/mobile-kit/component-specs/platform-status-bar.md`
 * 「Choose light or dark system-bar appearance from the **effective Polaris theme**」。
 * 实现：`src-tauri/gen/android/app/src/main/java/com/polaris2/app/MainActivity.kt`。
 *
 * ── 这道门要挡的是什么 ──────────────────────────────────────────────────────
 * 缺陷原形：`onCreate` 只有一句无参 `enableEdgeToEdge()`，其缺省按 `Configuration` 的夜间位
 * 判明暗 ⇒ 系统浅色 + 应用深色时，深色状态栏图标压在深色页头上，整条状态栏读不出来。
 *
 * 修法是把真值改成根元素的 `data-theme`（前端 `resolveTheme` 与主进程 `theme_boot_script`
 * 的**共同产物**，也正是 `tokens.resolved.css` 选色所依据的那个属性）。于是这道门要证的是
 * 四件事，缺一件这条腿就是"机制在、接线断"：
 *
 *  ① **取值口径没有第二份**：Kotlin 侧一行都不读夜间位，且 `data-theme` 真的是生产渲染
 *     路径上的那个属性（CSS 拿它选色、两个生产写入点都写它）。
 *  ② **探针真的会上报**：不是"文件里出现过 `data-theme` 这个字样"，而是把 Kotlin 里那段
 *     JS 原样抠出来**跑一遍**（`node:vm` + 最小宿主桩），看它到底 post 了什么。
 *  ③ **三种时机一个不少**：冷启动 / 运行期改主题 / 配置变化。
 *  ④ **中枢那三跳没断**：收信口收到的东西真的进了 `onThemeReport`；`onThemeReport` 真的既记住了
 *     `polarisDark` 又调了 `applySystemBarAppearance`；`reapplySystemBarAppearance` 真的重新施加。
 *     ①②③ 钉的全在**两端**，两端之间的缝上一条断言都没有 —— 三跳里任一跳被换成"只取个值"，
 *     上面全绿而缺陷原样复发（2026-09-05 实测四刀，逐刀记在 ⑤ 的段头）。
 *
 * ── 为什么②要执行而不是匹配字符串 ──────────────────────────────────────────
 * 这段 JS 住在 Kotlin 的字符串字面量里，**没有任何编译器看它**。`getAttribute('data-them')`
 * 少一个字母、`attributeFilter` 写成 `attributeFilters`、`postMessage` 拼错 —— 全都编译通过、
 * 运行期零报错，表现只是"状态栏再也不跟着主题变"。字符串匹配式判据挡不住其中任何一个。
 *
 * ── 宿主桩的射程如实登记 ────────────────────────────────────────────────────
 * 桩**不模拟 Chromium**，它只记录载荷对宿主 API 的调用，然后由本文件驱动回调。故本节能证的是
 * 「载荷与 MutationObserver / WebMessageListener 的**契约**对不对」（观察的是不是根元素、
 * 过滤的是不是 `data-theme`、post 出去的是不是属性值），不能证「Chromium 真会这么调它」——
 * 后者只有真机能证，本批未做。
 */

import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve as resolvePath } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createContext, runInContext } from 'node:vm';
import { maskRustComments, productionRsFilesUnder } from '@/contracts/rust-source.test-support';

/** …/ui/ */
const UI_ROOT = fileURLToPath(new URL('../../', import.meta.url));
/** 仓根（`src-tauri` 与 `ui` 的共同父目录）。 */
const REPO_ROOT = resolvePath(UI_ROOT, '..');

const readRepo = (relToRepo: string) => readFileSync(resolvePath(REPO_ROOT, relToRepo), 'utf8');

const MAIN_ACTIVITY = 'src-tauri/gen/android/app/src/main/java/com/polaris2/app/MainActivity.kt';
const KT_RAW = readRepo(MAIN_ACTIVITY);

/**
 * **必须净化**：本文件的类头注逐条讲了下面断言的每一个 API 名与每一条理由（`uiMode`、
 * `enableEdgeToEdge`、`data-theme` 全在注释里出现过）。不剥就是判据被被测文件自己的文档喂饱。
 *
 * 复用 `contracts/rust-source.test-support` 那份共享净化器（`crates/source-probe` 的移植），
 * 不在本文件再抄一份：Kotlin 与 Rust 的行/块注释与字符串词法同形，而两份剥法一旦分叉，
 * 「那道门挡得住、这道门挡不住」的缝是绿的。注释被换成空格，**字符串原样保留** ——
 * 下面要抠的 JS 载荷正住在字符串里。
 */
const KT = maskRustComments(KT_RAW);

/**
 * 取一个 Kotlin 成员的源码段：从 `fun <name>(` 起、到下一个 `fun` 声明止。
 * 找不到返回空串 —— 下面每条都会因为空串而红，而不是因为"整份文件里有这个字样"而绿。
 * （与 `mobile-entry.test.ts` ⑦⑧ 同一套切法，理由同：整份文件是弱判据。）
 */
const member = (name: string): string => {
  const at = KT.search(new RegExp(`\\bfun\\s+${name}\\s*\\(`));
  if (at < 0) return '';
  const rest = KT.slice(at + 1);
  const next = rest.search(/\n\s*(?:private\s+|override\s+|internal\s+|public\s+)*fun\s/);
  return next < 0 ? rest : rest.slice(0, next);
};

/**
 * 成员的**函数体**：把签名那一段（连同函数名本身）切掉。
 *
 * 必须切，否则判据会被被测符号自己的名字喂饱：`reapplySystemBarAppearance` 这个标识符里
 * **含有** `applySystemBarAppearance` —— 直接在 `member()` 的结果上断言"它调了
 * `applySystemBarAppearance(`"，光靠它自己那行声明就恒真。⓪ 里对这个陷阱有正反两条对照。
 * 名字用 `bodyOf` 不用 `body`：④ 的三个 `it` 里已各有一个同名局部变量，重名会把这个助手遮掉。
 */
const bodyOf = (name: string): string => {
  const s = member(name);
  const at = s.indexOf('{');
  return at < 0 ? '' : s.slice(at + 1);
};

/** `private const val NAME = "…"` 的值。取不到抛 —— 空串上的断言全是恒真。 */
function ktConst(name: string): string {
  const m = KT.match(new RegExp(`\\bconst\\s+val\\s+${name}\\s*=\\s*"((?:[^"\\\\]|\\\\.)*)"`));
  if (m === null) throw new Error(`Kotlin 常量 \`${name}\` 取不到 —— 改名了？判据面塌了，不是"它是空的"`);
  return m[1];
}

/**
 * 抠出探针载荷：`private const val THEME_PROBE_JS =` 之后那串用 `+` 拼起来的字面量，
 * 其中夹着的 `THEME_BRIDGE` 标识符按常量真值展开。
 *
 * **展开而不是照抄字面量**，正是为了让"两侧改名不同步"这件事在这里当场露馅：JS 里 post 的对象名
 * 与 Kotlin 注册给 `addWebMessageListener` 的名字若不是同一个，下面的执行判据拿不到 postMessage。
 */
function probePayload(): string {
  const at = KT.indexOf('const val THEME_PROBE_JS');
  if (at < 0) throw new Error('`THEME_PROBE_JS` 不见了 —— 探针载荷的取材面塌了');
  // 按行收拼接链：声明行之后，凡以 `+` 结尾的都还在链上，第一条不以 `+` 结尾的行是最后一段。
  // **不按"到下一个 class 为止"切**：常量放在类前还是类后是个随时会变的排版决定，
  // 而判据不该因为挪了一段代码就悄悄扩大取材面。
  const expr: string[] = [];
  for (const line of KT.slice(at).split('\n')) {
    expr.push(line);
    if (expr.length > 1 && !line.trimEnd().endsWith('+')) break;
  }
  const bridge = ktConst('THEME_BRIDGE');
  let out = '';
  const token = /"((?:[^"\\]|\\.)*)"|\bTHEME_BRIDGE\b/g;
  const joined = expr.join('\n');
  let m: RegExpExecArray | null = token.exec(joined);
  while (m !== null) {
    out += m[1] === undefined ? bridge : m[1].replace(/\\(.)/g, '$1');
    m = token.exec(joined);
  }
  return out;
}

const PROBE_JS = probePayload();

/** 一次 `postMessage` 的记录。 */
interface Posted {
  readonly name: string;
  readonly data: unknown;
}

/** 一次 `observe(target, options)` 的记录。 */
interface Observed {
  readonly target: unknown;
  readonly options: { attributes?: boolean; attributeFilter?: string[] };
  readonly fire: () => void;
}

/**
 * 最小宿主桩：只提供载荷用得到的那几个东西，并把它对宿主的每一次调用记下来。
 *
 * `bridgeName` 单独传：桩**按 Kotlin 注册的那个名字**挂 post 口。名字对不上时挂的就是别的属性，
 * 载荷惰性查到的是 `undefined` ⇒ `posted` 恒空 ⇒ 判据红。这就是两侧改名不同步的现形处。
 */
function runProbe(opts: { readonly bridgeName: string; readonly theme?: string; readonly reruns?: number }) {
  const posted: Posted[] = [];
  const observed: Observed[] = [];
  const domListeners: string[] = [];
  const attrs = new Map<string, string>();
  if (opts.theme !== undefined) attrs.set('data-theme', opts.theme);

  const documentElement = {
    getAttribute: (k: string) => attrs.get(k) ?? null,
    /** 桩自己的写入口：按已登记的 `attributeFilter` 决定要不要唤醒观察者。 */
    setAttribute(k: string, v: string) {
      attrs.set(k, v);
      for (const o of observed) {
        if (o.options.attributes === true && (o.options.attributeFilter ?? []).includes(k)) o.fire();
      }
    },
  };

  const sandbox: Record<string, unknown> = {};
  const win: Record<string, unknown> = {
    [opts.bridgeName]: { postMessage: (data: unknown) => posted.push({ name: opts.bridgeName, data }) },
  };
  sandbox.window = win;
  sandbox.document = {
    documentElement,
    addEventListener: (evt: string) => domListeners.push(evt),
  };
  sandbox.MutationObserver = class {
    constructor(private readonly cb: () => void) {}
    observe(target: unknown, options: Observed['options']) {
      observed.push({ target, options, fire: () => this.cb() });
    }
  };

  const ctx = createContext(sandbox);
  for (let i = 0; i <= (opts.reruns ?? 0); i += 1) runInContext(PROBE_JS, ctx);
  return { posted, observed, domListeners, documentElement };
}

describe('⓪ 自检：判据的取材面是活的（否则下面每条都恒绿）', () => {
  it('Kotlin 源读得到，且注释真的被剥掉了', () => {
    expect(KT_RAW.length, `${MAIN_ACTIVITY} 读到的是空文件 —— 判据面塌了`).toBeGreaterThan(400);
    expect(KT, '净化后什么都不剩 —— 剥法把代码一起吃了').toContain('class MainActivity');
    // 正面对照：`uiMode` 只出现在头注里（本条同时是①那条否定断言的前提）。
    expect(KT_RAW, '头注里连 `uiMode` 都没有了 —— 这条对照失效，剥法没被真正验过').toContain('uiMode');
    expect(KT, '`uiMode` 剥完还在 = 注释没剥干净，下面的断言可能是注释在充数').not.toContain('uiMode');
  });

  it('成员切段真的把成员分开了（切成整份文件 = 退回"文件里出现过"的弱判据）', () => {
    expect(member('applySystemBarAppearance').length, '`applySystemBarAppearance` 切不出来').toBeGreaterThan(60);
    expect(member('onThemeReport').length, '`onThemeReport` 切不出来').toBeGreaterThan(40);
    expect(member('applySystemBarAppearance').length).toBeLessThan(KT.length / 2);
    // 反向对照：不存在的成员切出空串（切段不是"永远返回整份文件"）。
    expect(member('applySomethingThatNeverExisted')).toBe('');
    // 反向对照之二：三条量真的被切开了 —— 主题这几段里没有另两条量的痕迹，反之亦然。
    expect(member('applySystemBarAppearance') + member('onThemeReport')).not.toContain('--safe-');
    expect(member('applySystemBarAppearance') + member('onThemeReport')).not.toContain('fontScale');
    expect(member('publishSafeArea'), '安全区那段里出现了系统栏外观 —— 切段串味了').not.toContain(
      'isAppearanceLight',
    );
  });

  it('切签名真的把函数名切掉了（不切的话 ⑤ 的"重申"那条恒真）', () => {
    // 正面：陷阱确实存在 —— 不切签名时，`reapplySystemBarAppearance` 靠自己的声明行就"含有"
    // `applySystemBarAppearance(`。这条一旦变红说明标识符改了名，⑤ 那条的自污染前提要重推。
    expect(
      member('reapplySystemBarAppearance'),
      '`reapplySystemBarAppearance` 不再包含 `applySystemBarAppearance(` —— 名字变了，' +
        '⑤ 用 `bodyOf()` 躲的那个陷阱要重新论证',
    ).toContain('applySystemBarAppearance(');
    // 反面：切完之后函数名自己不在里面了，剩下的只能是真的调用。
    expect(bodyOf('reapplySystemBarAppearance'), '签名没切掉 —— ⑤ 的"重申"那条会恒真').not.toContain(
      'reapplySystemBarAppearance',
    );
    // 三段函数体都非空：空串上 `toContain` 也会红，但那是取材面塌了，与"接线断了"是两回事。
    for (const n of ['installThemeBridge', 'onThemeReport', 'reapplySystemBarAppearance'])
      expect(bodyOf(n).length, `\`${n}\` 的函数体切不出来 —— ⑤ 整节的取材面塌了`).toBeGreaterThan(20);
  });

  it('探针载荷抠得出、拼得对，且是**可执行**的 JS（抠错了下面全是恒绿）', () => {
    expect(PROBE_JS.length, '抠出来的载荷太短 —— 拼接式常量没被完整拼回来').toBeGreaterThan(200);
    expect(PROBE_JS, '载荷里还留着 Kotlin 的拼接符号 —— 抠法把源码当字符串了').not.toContain('" +');
    // 边界自检：多抠一行或少抠一行，这两条立刻红（"跑得通"本身不证明抠全了）。
    expect(PROBE_JS.startsWith('(function(){'), `载荷不是从 IIFE 开头起的：${PROBE_JS.slice(0, 40)}`).toBe(true);
    expect(PROBE_JS.endsWith('})();'), `载荷不是在 IIFE 收尾处止的：${PROBE_JS.slice(-40)}`).toBe(true);
    expect(PROBE_JS, '桥接对象名没被展开进载荷 —— 拼接链里的标识符没解析').toContain(
      `window.${ktConst('THEME_BRIDGE')};`,
    );
    // 真的能跑：语法错在这里当场抛（字符串匹配式判据看不见语法错）。
    expect(() => runProbe({ bridgeName: ktConst('THEME_BRIDGE') })).not.toThrow();
  });
});

describe('① 取值口径只有一份：`data-theme`，不是系统夜间位', () => {
  it('Kotlin 侧一行都不读夜间模式（读了就是第二份判断，且分叉时没有门看得见）', () => {
    for (const banned of ['uiMode', 'UI_MODE_NIGHT', 'isNightModeActive', 'MODE_NIGHT_YES']) {
      expect(
        KT,
        `代码里出现了 \`${banned}\` —— 生效主题的折算口径必须只有 \`resolveTheme\` 那一份，` +
          '在原生侧照着再写一遍会立刻多出一条要同步的规则，而两份分叉时表现只是"某些组合下状态栏不对"',
      ).not.toContain(banned);
    }
  });

  it('探针读的就是根元素的 `data-theme`（执行验证，不是字样匹配）', () => {
    const run = runProbe({ bridgeName: ktConst('THEME_BRIDGE'), theme: 'dark' });
    expect(run.posted.map((p) => p.data), '属性已在时没有立刻上报 —— 冷启动那一格是空的').toEqual(['dark']);
  });

  it('`data-theme` 真的在生产渲染路径上：CSS 拿它选色，两个生产写入点都写它', () => {
    // a. 它决定页头颜色 —— 这才是"系统栏要跟它"的理由。
    expect(
      readRepo('ui/src/styles/tokens.resolved.css'),
      '`tokens.resolved.css` 不再按 `[data-theme]` 选色 —— 那这个属性就不是生效主题了，整条腿的前提没了',
    ).toMatch(/:root\[data-theme='dark'\]/);
    // b. 运行期写入点：设置→显示 改主题，写的是 `resolveTheme` 的返回值。
    const display = readRepo('ui/src/mobile/settings/DisplayPage.tsx');
    expect(display, 'DisplayPage 不再写 `data-theme`').toContain("'data-theme'");
    expect(display, 'DisplayPage 写的不再是 `resolveTheme` 的返回值 —— 口径分叉了').toContain('resolveTheme(');
    // c. 冷启动写入点：主进程首帧前的种子（移动端加载的同样是主窗）。
    expect(
      readRepo('src-tauri/src/tray/model.rs'),
      '`theme_boot_script` 不再播种 `data-theme` —— 冷启动那一格会拿不到值',
    ).toContain("'data-theme'");
  });
});

describe('② 生效动作：真的改系统栏外观，且方向没反', () => {
  const APPLY = member('applySystemBarAppearance');

  it('状态栏与导航栏两位都设（只设一位会在深浅色各留一半不可读）', () => {
    expect(APPLY, '没设状态栏外观位').toContain('isAppearanceLightStatusBars');
    expect(APPLY, '没设导航栏外观位 —— 底部导航铺的是 surface，手势条压在它上面').toContain(
      'isAppearanceLightNavigationBars',
    );
  });

  it('取的是 `!dark`（写成 `= dark` 会从"跟错了源"变成"永远反着"，一样读不出来）', () => {
    expect(APPLY.replace(/\s+/g, ' ')).toMatch(/isAppearanceLightStatusBars\s*=\s*!\s*dark\b/);
    expect(APPLY.replace(/\s+/g, ' ')).toMatch(/isAppearanceLightNavigationBars\s*=\s*!\s*dark\b/);
  });

  it('作用在真实窗口上（拿一个游离 View 去要 controller 等于什么都没改）', () => {
    expect(APPLY, '不是从 `window` / `window.decorView` 取的 controller').toMatch(
      /getInsetsController\(\s*window\s*,\s*window\.decorView\s*\)/,
    );
  });
});

describe('③ 探针的上报契约（把 Kotlin 里那段 JS 跑一遍）', () => {
  const BRIDGE = ktConst('THEME_BRIDGE');

  it('观察的是根元素、过滤的是 `data-theme`（观察错目标 = 永远收不到主题变化）', () => {
    const run = runProbe({ bridgeName: BRIDGE, theme: 'light' });
    expect(run.observed.length, '一个 MutationObserver 都没装 —— 运行期改主题这条腿是断的').toBe(1);
    expect(run.observed[0].target, '观察的不是 `document.documentElement`').toBe(run.documentElement);
    expect(run.observed[0].options.attributes).toBe(true);
    expect(run.observed[0].options.attributeFilter, '过滤器里没有 `data-theme`').toContain('data-theme');
  });

  it('运行期改主题会上报新值（`DisplayPage` 的 `pickTheme` 走的就是这条）', () => {
    const run = runProbe({ bridgeName: BRIDGE, theme: 'light' });
    run.documentElement.setAttribute('data-theme', 'dark');
    expect(run.posted.map((p) => p.data), '改属性后没有补报 —— 用户切主题后状态栏会停在旧值').toEqual([
      'light',
      'dark',
    ]);
  });

  it('post 的对象名 = Kotlin 注册给 `addWebMessageListener` 的那个（两侧改名不同步在这里现形）', () => {
    const install = member('installThemeBridge');
    expect(install, '没有调 `addWebMessageListener` —— 收信口就没开').toContain('addWebMessageListener');
    expect(
      install,
      '注册用的不是 `THEME_BRIDGE` 常量 —— 一旦写成第二份字面量，改名不同步时 postMessage 打在 undefined 上，' +
        '状态栏从此停在最后一个已知值且运行期零报错',
    ).toMatch(/addWebMessageListener\(\s*webView\s*,\s*THEME_BRIDGE\s*,/);
    // 反向对照：换一个名字挂桩，载荷就找不到 post 口（证明上面那条不是恒真）。
    expect(runProbe({ bridgeName: `${BRIDGE}Wrong`, theme: 'dark' }).posted).toEqual([]);
  });

  it('属性还没写上时不乱报（冷启动时种子可能比本脚本晚一步）', () => {
    const run = runProbe({ bridgeName: BRIDGE });
    expect(run.posted, '属性缺席时报了值 —— 那是编出来的').toEqual([]);
    // 种子随后写上，观察者接住 —— 这正是"两种注入顺序都覆盖"里的另一序。
    run.documentElement.setAttribute('data-theme', 'dark');
    expect(run.posted.map((p) => p.data)).toEqual(['dark']);
  });

  it('重复注入只装一个观察者（`onResume` 会重跑本载荷）', () => {
    const run = runProbe({ bridgeName: BRIDGE, theme: 'dark', reruns: 2 });
    expect(run.observed.length, '每跑一次多挂一个观察者 —— 改一次主题会上报 N 次').toBe(1);
    // 三次注入各补报一次当前值：那正是重跑的目的（`onResume` 的兜底）。
    expect(run.posted.map((p) => p.data)).toEqual(['dark', 'dark', 'dark']);
  });

  it('还有一张 `DOMContentLoaded` 的补报网（注入对象与种子到那时必然都已就位）', () => {
    expect(runProbe({ bridgeName: BRIDGE }).domListeners).toContain('DOMContentLoaded');
  });
});

describe('④ 三种时机一个不少', () => {
  /*
   * 本节只钉**时机在不在**：三个回调都在、都把重申调下去了。**"调下去真的会改到外观"不在本节
   * 射程内** —— `polarisDark` 不再被赋值、或 `polarisDark?.let { … }` 的块体被清空时，
   * 本节四条照样全绿（2026-09-05 实测）。接住那半的是 ⑤，两节成对才等于"这三条腿被钉住了"。
   */

  it('冷启动：`onWebViewCreate` 先开收信口、再放探针', () => {
    const body = member('onWebViewCreate');
    expect(body, '冷启动没开收信口 —— 探针 post 出去没人收').toContain('installThemeBridge(');
    expect(body, '冷启动没放探针 —— 要等用户切一次主题状态栏才对').toContain('publishThemeProbe(');
    expect(
      body.indexOf('installThemeBridge('),
      '探针放在了收信口之前 —— 注入对象晚于页面脚本时首次上报会落空',
    ).toBeLessThan(body.indexOf('publishThemeProbe('));
  });

  it('探针两条投递通道都在（少了文档起始那条，一次自愈重载后主题跟随永久消失）', () => {
    const body = member('publishThemeProbe');
    expect(body, '缺 `evaluateJavascript`').toContain('evaluateJavascript');
    expect(
      body,
      '缺 `addDocumentStartJavaScript`：冷启动时 URL 还没开始加载，且 `renderer-recovery` 的 ' +
        '`location.reload()` 会把整个文档连同观察者一起丢掉',
    ).toContain('addDocumentStartJavaScript');
  });

  it('配置变化与回前台都重申一次（`enableEdgeToEdge` 是 onCreate 里的一次性调用）', () => {
    for (const [hook, why] of [
      ['onConfigurationChanged', '`configChanges` 含 `uiMode` ⇒ 系统切深浅色时 Activity 不重建，没有任何东西会重新施加外观'],
      ['onResume', '后台期间外观位可能被系统重置'],
    ] as const) {
      const body = member(hook);
      expect(body.length, `${hook} 不见了 —— ${why}`).toBeGreaterThan(20);
      expect(body, `${hook} 里没有重申系统栏外观 —— ${why}`).toContain('reapplySystemBarAppearance()');
    }
  });

  it('一次都没收到过上报时**不动**（编一个值去覆盖 `enableEdgeToEdge` 那一格只会把对的改错）', () => {
    expect(member('reapplySystemBarAppearance'), '重申时没有判"收到过没有"').toMatch(
      /polarisDark\s*\?\.\s*let/,
    );
    expect(member('onThemeReport'), '归一化没有 `else -> return` —— 读不懂的值也会翻转状态栏').toMatch(
      /else\s*->\s*return/,
    );
  });
});

describe('⑤ 中枢接线：收信口 → 归一化 → 生效动作，三跳一跳都不能断', () => {
  /*
   * 为什么单开一节：①②③④ 把**两端**各自钉得很死（探针 post 得对、`applySystemBarAppearance`
   * 改得对），却没有任何一条把两端连起来 —— 正是本仓已命名的"函数被测、生产没在用它"。
   *
   * 2026-09-05 在本树实测四刀，每刀都不碰两端、都编译得过，而当时 21 条判据全绿（rc=0）：
   *  · 把 `installThemeBridge` 回调里的 `onThemeReport(…)` 换成只把值取到一个局部变量 ——
   *    整条腿离开生产路径，K2-01 的原缺陷（深色图标压深色页头）逐字复发；
   *  · 删掉 `onThemeReport` 里的 `applySystemBarAppearance(dark)` —— 改主题后要等一次切后台才纠正；
   *  · 删掉 `polarisDark = dark` —— `reapplySystemBarAppearance()` 永远空转，onResume 与
   *    onConfigurationChanged 两条兜底腿双双失效；
   *  · 把 `polarisDark?.let { … }` 的块体清空 —— 同上，而 ④ 只 match `polarisDark?.let`，照样绿。
   * 下面四条就是这四刀的对拍。
   *
   * ── 射程 ──────────────────────────────────────────────────────────────────
   * 本节证的是"这三跳在源码上确实接着"，不是"运行期真的走通了" —— 后者要 Kotlin 跑起来，
   * 本机没有 JVM（`gradlew` 要联网取发行版），本批未做。故这里挡的是**重构时手滑摘掉一环**，
   * 挡不住"接着但运行期抛异常"。
   */

  it('收信口收到的东西真的进了消费点（回调只取值不消费 = 整条腿离开生产路径）', () => {
    const install = bodyOf('installThemeBridge');
    const at = install.indexOf('addWebMessageListener');
    expect(at, '`installThemeBridge` 里没有 `addWebMessageListener` —— 收信口就没开').toBeGreaterThanOrEqual(0);
    // 只看**注册调用之后**那一段：`onThemeReport(` 落在 `addWebMessageListener` 之前
    // （例如挪进了"特性不支持"的早退分支里）不算接上。
    const callback = install.slice(at);
    expect(
      callback,
      '`addWebMessageListener` 的回调里没有调 `onThemeReport` —— 收信口开着、消息也收得到，' +
        '但没有任何东西消费它：状态栏从此不跟 Polaris 主题走，编译通过、运行期零报错',
    ).toContain('onThemeReport(');
    expect(
      callback,
      '回调里一次都没读 `message.data` —— 送进消费点的是编出来的值，不是 web 层报上来的那个',
    ).toContain('message.data');
  });

  it('归一化真的施加外观（少了这一句，改主题后要等一次切后台才纠正）', () => {
    expect(
      bodyOf('onThemeReport'),
      '`onThemeReport` 收下了值却没调 `applySystemBarAppearance` —— 上报链路完好、生效链路断开，' +
        '②"方向没反"那两条仍然全绿，因为它们只看那个函数本身怎么写',
    ).toContain('applySystemBarAppearance(');
  });

  it('归一化真的记住这次的值（少了它，onResume / onConfigurationChanged 两条兜底腿永久空转）', () => {
    expect(
      bodyOf('onThemeReport').replace(/\s+/g, ' '),
      '`onThemeReport` 没有把结果写回 `polarisDark` —— 它恒为 null ⇒ `reapplySystemBarAppearance()` ' +
        '永远是空操作，而 ④ 只 match `polarisDark?.let`，两条兜底腿死掉时它一声不吭',
    ).toMatch(/polarisDark\s*=\s*dark\b/);
  });

  it('重申真的重新施加（块体被清空时 ④ 那条 `polarisDark?.let` 照样绿）', () => {
    expect(
      bodyOf('reapplySystemBarAppearance'),
      '重申里没有调 `applySystemBarAppearance` —— onResume 与 onConfigurationChanged 都还在调它，' +
        '调下去却什么都不做；④ 那两条只查"调了重申没有"，看不见这一层',
    ).toContain('applySystemBarAppearance(');
  });
});

describe('⑥ 桌面零影响：这条腿一行 Rust 都没有', () => {
  const RS = productionRsFilesUnder('src-tauri/src');

  /** 同一条谓词：某个字样命中的文件（仓库相对路径）。下面的正反两条都用它，不各写一份。 */
  const hitting = (needle: string): string[] =>
    RS.filter((f) => readFileSync(f, 'utf8').includes(needle)).map((f) => f.slice(REPO_ROOT.length + 1));

  it('自检：Rust 取材面非空，且这条扫法**真的扫得出东西**（否则下面那条恒真）', () => {
    expect(RS.length, '`src-tauri/src` 下一个生产 .rs 都没扫到 —— 取材面塌了').toBeGreaterThan(20);
    // 正向对照：拿一个**确定存在**的字样喂同一条谓词。少了它，「没扫到」既可能是真没有，
    // 也可能是读文件/匹配那一步整个坏了 —— 两种情况在报告上长得一模一样。
    expect(
      hitting('theme_boot_script'),
      '连 `theme_boot_script` 都扫不到 —— 扫法坏了，下面那条 `toEqual([])` 是恒真，不是证据',
    ).not.toEqual([]);
  });

  it('Rust 生产代码里没有本条腿的任何痕迹（有的话就得逐个查 cfg 才敢说桌面没被动）', () => {
    const hits = [...hitting(ktConst('THEME_BRIDGE')), ...hitting('addWebMessageListener')];
    expect(
      hits,
      `系统栏这条腿出现在了 Rust 侧：\n${hits.join('\n')}\n` +
        '它今天整条住在 Android 的 `MainActivity.kt` 里，故桌面平台结构上不可能受影响；' +
        '一旦有 Rust 参与，这句话就得改成"逐个 cfg 查过"，而那是要重新论证的。',
    ).toEqual([]);
  });
});
