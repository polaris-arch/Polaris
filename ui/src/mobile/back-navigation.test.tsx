/**
 * 移动端**系统返回键**这条链路的门（Android）。
 *
 * # 被守的缺陷
 *
 * 接线之前：`AppPlugin.kt:28-46` 那个唯一的返回回调在「没人监听 `back-button`」时走
 * `canGoBack() ? goBack() : onBackPressed()`，而本仓没有路由库、WebView 没有历史 ⇒ `finish()`。
 * **任何一屏、任何一层弹层、任何一个二级页上按返回都直接退出 App。**
 *
 * 接线之后多出一个**新**缺陷面：注册了 JS 监听，那两条原生腿就一条都不走了 ⇒ 根页那一跳
 * 必须由 JS 自己兑现，否则表现是「按返回完全没反应」，比退出更糟。③ 组守的正是这一条。
 *
 * # 判据面（三块，逐块都有自检）
 *
 *  · **纯函数**：`back-stack.ts` 的 LIFO 语义 —— 不碰 DOM、不碰 React，直接驱动（① 组）。
 *  · **真渲染**：`renderToStaticMarkup` 喂真组件，用一颗渲染期读深度的探针看「这一层登记了没有」。
 *    能这么判的**前提**是登记发生在渲染期（`back-stack.ts` 头注写了为什么必须如此）——
 *    换成 `useEffect` 上报，② ④ 两组会**全部恒绿**，接线断了也不红。
 *  · **源码扫描**：只用在渲染判不了的两处 —— ② 组的「哪些组件画了背幕」与 ③ 组的跨语言桥名对拍。
 *
 * # ② 组的取材面（全称否定的前提，写在这里）
 *
 * 取材面①（**并集**，两条腿）= `ui/src/mobile/` 下全部 CSS 里的：
 *  · **A 名字腿**：类名含 `scrim` 的选择器；
 *  · **B 形态腿**：同一条规则里既有 `position: fixed` 又有 `inset: 0` 的类（= 全视口遮罩）。
 *
 * 只留 A，第五套弹层换个前缀叫 `-veil` / `-overlay` / `-backdrop` 就整套绕开（本仓四套各用各的
 * 前缀，另起一套命名是最自然的走向）；只留 B，`.mn-sheet-scrim` 那种 `position: absolute` 的背幕
 * 会漏掉（它铺的是外层那个 fixed 层）。两条腿都要 —— B 腿今天真的多捞到一个类
 * （`.mn-sheet-layer`，名字里没有 `scrim`），⓪ 组正面钉住这件事，否则「并集」可能只是写着好看。
 *
 * 消费面 = `ui/src/mobile/` 下全部 `.ts`/`.tsx` 去掉测试文件（剥注释、**保留字符串**，
 * class 名就住在字符串里），单引号与双引号两种字面量都吃。
 *
 * 选遮罩当结构标记的依据：本仓四套弹层**各自都画了一层背幕**，且那不是装饰 —— 它同时兑现
 * 「点外面关掉」与「不许把点击穿透到底下那颗连接开关」（`home/HomeScreenView.tsx` 的 `Scrim` 头注）。
 * 故「一套弹层」与「一层遮罩」在本仓是一一对应的。
 *
 * # ② 组的主判据是**恰等快照**，不是「每处遮罩所在的函数都登记了」
 *
 * 归属按「位置落在哪个顶层声明里」切出来，而那是一个**过近似**：把一层遮罩内联进一个已经为
 * **别的**层登记过的大屏组件（`MobileNodesScreen` 为批选态登记了一层），它会继承那处登记 ⇒
 * `registers` 假真。这不是漏报，是**错报成已登记**，而这道门唯一的使命就是让「忘了登记」自曝。
 * 故真值面是 `OVERLAY_RENDERED` 那份 `文件::函数::类` 的恰等快照：新增或挪动任何一处遮罩都会
 * 让集合不等而当场红，与切片器准不准无关。`registers` 那条腿保留，作为更早一步的诊断。
 *
 * ⚠️ **抓不到什么**（如实记）：
 *  · 一套既**不画全视口遮罩**、类名里又**不含 `scrim`** 的新弹层不在取材面里。
 *  · class 名用模板串 / `clsx` 拼出来时字面量匹配不到 —— 但若那是个**新**类，
 *    ⓪ 组「每个遮罩类都得有人渲染」会先红（诊断词写的是「死规则」，方向对但话说得不准）。
 *  · 「登记了但传了个恒为 false 的 `active`」由 ② 组的**行为腿**接住，不是靠扫描。
 *
 * # 判据不能被自己污染
 *
 * 本文件逐字写着 `useDismissableLayer`、`-scrim` 这些要断言其存在/不存在的字面量。
 * 取材面显式排除 `.test.`，并由 ⓪ 组正面断言「本文件不在取材面里」。
 */

import { beforeEach, describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, relative, resolve } from 'node:path';
import type { ReactElement } from 'react';
import { maskRustComments } from '@/contracts/rust-source.test-support';

import {
  clearDismissables,
  dismissTop,
  pushDismissable,
  useDismissableDepth,
} from './back-stack';
import {
  BACK_NAV_BRIDGE,
  BACK_NAV_EXIT,
  exitToBackground,
  handleBackPress,
  type BackOutcome,
} from './MobileApp';
import { DEFAULT_DESTINATION, type DestinationId } from './destinations';
import { MobileShell, setPushedPage } from './MobileShell';
import { Scrim } from './home/HomeScreenView';
import { Sheet as NodesSheet } from './nodes/NodesScreenView';
import { ActionSheet as ConnectionsSheet } from './connections/ConnectionsView';
import { MobileConnectionsScreen } from './connections/MobileConnectionsScreen';
import { MobileNodesScreen } from './nodes/MobileNodesScreen';
import { ActionSheet as RulesSheet, SelectSheetPanel } from './screens/rules/Primitives';
import { FormSheet } from './forms/FormSheet';
import { MobileRulesScreen } from './screens/rules/RulesScreen';
import { MobileSettingsScreen } from './settings/MobileSettingsScreen';
import i18next from 'i18next';
import { useAppStore } from '@/store/app-store';
import type { UserConfig } from '@/contracts/types';

/** …/ui/src/mobile/ */
const MOBILE = fileURLToPath(new URL('.', import.meta.url));
/** 仓根（`src-tauri` 与 `ui` 的共同父目录）。 */
const REPO_ROOT = resolve(MOBILE, '../../..');
const rel = (abs: string): string => relative(MOBILE, abs).split('\\').join('/');

/**
 * 文案**去问 i18next 自己**，不在判据里写死键名或译文。
 *
 * 理由是踩过的：本文件的 import 面（`MobileApp` → 五个屏）会把 `@/i18n` 一并拉起来，
 * 于是屏渲染出的是**译文**而不是 key；而相邻的 `screens/rules/rules-screen.test.tsx`
 * 因为 import 面更窄，同一棵树渲染出的是 key。断言写死哪一种都会随 import 面漂。
 * 走同一个默认实例问一次，两种初始化状态下都成立。
 */
const tr = (key: string): string => String(i18next.t(key));

// ── 取材器 ───────────────────────────────────────────────────────────────────

/**
 * 剥 TS/TSX 的注释，**字符串原样保留**（class 名与桥名都住在字符串里）。
 * 必须识别字符串，否则 `'**​/*.css'` 这类 glob 里的 `/*` 会被当成注释起点，一路吃到文件末尾。
 */
function stripTs(src: string): string {
  let out = '';
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    const next = src[i + 1];
    if (c === '/' && next === '*') {
      const end = src.indexOf('*/', i + 2);
      i = end === -1 ? src.length : end + 2;
      out += ' ';
    } else if (c === '/' && next === '/') {
      const end = src.indexOf('\n', i);
      i = end === -1 ? src.length : end;
    } else if (c === "'" || c === '"' || c === '`') {
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

/** 剥 CSS 的注释与字符串（一遍扫完：分两遍会互相误吞）。 */
function stripCss(src: string): string {
  let out = '';
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === '/' && src[i + 1] === '*') {
      const end = src.indexOf('*/', i + 2);
      i = end === -1 ? src.length : end + 2;
      out += ' ';
    } else if (c === "'" || c === '"') {
      let j = i + 1;
      while (j < src.length && src[j] !== c) j += src[j] === '\\' ? 2 : 1;
      i = j + 1;
      out += ' ';
    } else {
      out += c;
      i += 1;
    }
  }
  return out;
}

function walk(dir: string, keep: (p: string) => boolean): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const abs = join(dir, name);
    if (statSync(abs).isDirectory()) out.push(...walk(abs, keep));
    else if (keep(abs)) out.push(abs);
  }
  return out;
}

/** 取材面①：移动端全部 CSS。 */
const CSS_FILES = walk(MOBILE, (p) => p.endsWith('.css'));
/** 取材面②：移动端全部源码，**去掉测试文件**（自污染防线，⓪ 组正面钉住）。 */
const SRC_FILES = walk(
  MOBILE,
  (p) => (p.endsWith('.ts') || p.endsWith('.tsx')) && !p.includes('.test.'),
);
const SRC = new Map(SRC_FILES.map((p) => [p, stripTs(readFileSync(p, 'utf8'))]));

/** CSS 的最内层规则块。`@media` 的外壳自然被跳过：它的块里还有块，`[^{}]*` 吃不下。 */
function cssRules(css: string): Array<{ selector: string; decls: string }> {
  return [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((m) => ({ selector: m[1], decls: m[2] }));
}

/**
 * 取材面①：移动端 CSS 里的**遮罩类**（两条腿的并集，理由见文件头注）。
 *
 * A 名字腿 = 类名含 `scrim`；B 形态腿 = 同一条规则里 `position: fixed` + `inset: 0`。
 */
const OVERLAY_CLASSES = [
  ...new Set(
    CSS_FILES.flatMap((p) => {
      const out: string[] = [];
      for (const { selector, decls } of cssRules(stripCss(readFileSync(p, 'utf8')))) {
        const fullscreen =
          /position:\s*fixed/.test(decls) && /(?:^|[;{\s])inset:\s*0\s*(?:;|$)/.test(decls);
        for (const [, name] of selector.matchAll(/\.([A-Za-z][\w-]*)/g)) {
          if (fullscreen || name.includes('scrim')) out.push(name);
        }
      }
      return out;
    }),
  ),
].sort();

/**
 * 顶层声明的起点表。**两种写法都要**：`function X(` 与 `const X = (…)` / `ident =>` /
 * `memo(` / `forwardRef(`。
 *
 * 少了 const 那一支，一个箭头函数组件不是「一个声明」而是上一个 `function` 的一部分，
 * 它里面的遮罩会**继承**上一个函数的登记 —— 不是漏报，是错报成已登记。本仓移动树今天
 * 恰好零处箭头组件，但那是巧合不是纪律（仓内无 eslint 配置，桌面侧箭头组件遍地）。
 */
function topLevelFunctions(src: string): Array<{ name: string; at: number }> {
  return [
    ...src.matchAll(/^(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*[(<]/gm),
    ...src.matchAll(
      /^(?:export\s+)?const\s+([A-Za-z_$][\w$]*)\s*(?::[^=\n]*)?=\s*(?:async\s+)?(?:\(|[A-Za-z_$][\w$]*\s*=>|memo\(|forwardRef\()/gm,
    ),
  ]
    .map((m) => ({ name: m[1], at: m.index ?? 0 }))
    .sort((x, y) => x.at - y.at);
}

/**
 * 顶层声明的收尾：**列 0 的 `}`**（本仓 2 空格缩进，函数体内不会出现）。
 *
 * 只按「下一个顶层声明起点」切不够：两个函数之间还可能有模块级代码，而它会被并进上一个
 * 函数的切片里。反过来也不能只按 `\n}` 切 —— `function useRulesStack(): {…} {` 的返回类型
 * 就在列 0 收口，故 `}` 后面必须是行尾（可带一个 `;`/`,`），`} {` 与 `}): T {` 都不算。
 */
const TOP_LEVEL_CLOSER = /\n\}[;,]?[^\S\n]*(?=\r?\n|$)/;

/** 位置 `at` 落在哪个顶层声明的函数体里；落在两者之间（模块级）返回 `null`。 */
function enclosingFunction(
  src: string,
  at: number,
): { name: string; body: string; span: number } | null {
  const fns = topLevelFunctions(src);
  let cur: { name: string; at: number } | null = null;
  let nextAt = src.length;
  for (const fn of fns) {
    if (fn.at <= at) cur = fn;
    else {
      nextAt = fn.at;
      break;
    }
  }
  if (cur === null) return null;
  const closer = TOP_LEVEL_CLOSER.exec(src.slice(cur.at));
  const closedAt = closer === null ? src.length : cur.at + closer.index + closer[0].length;
  const end = Math.min(nextAt, closedAt);
  if (at >= end) return null;
  return { name: cur.name, body: src.slice(cur.at, end), span: end - cur.at };
}

/** 每个遮罩类 → 渲染它的那些顶层函数（跨全部源码文件）。单引号与双引号两种字面量都吃。 */
type OverlaySite = { cls: string; file: string; fn: string; registers: boolean; span: number };
function overlaySites(): OverlaySite[] {
  const out: OverlaySite[] = [];
  for (const cls of OVERLAY_CLASSES) {
    const needle = new RegExp(`["']${cls}["']`, 'g');
    for (const [abs, src] of SRC) {
      for (const m of src.matchAll(needle)) {
        const fn = enclosingFunction(src, m.index ?? 0);
        if (fn === null) {
          throw new Error(
            `${rel(abs)} 里的 \`${cls}\` 不在任何顶层声明的函数体内 —— 切片器失效，或它住在模块级`,
          );
        }
        out.push({
          cls,
          file: rel(abs),
          fn: fn.name,
          registers: fn.body.includes('useDismissableLayer('),
          span: fn.span,
        });
      }
    }
  }
  return out;
}
const OVERLAY_SITES = overlaySites();

/** `文件::函数::类`，去重后排序。② 组的主判据就是它与快照恰等。 */
const OVERLAY_SITE_KEYS = [
  ...new Set(OVERLAY_SITES.map((s) => `${s.file}::${s.fn}::${s.cls}`)),
].sort();

// ── Kotlin 侧取材（跨语言桥对拍）──────────────────────────────────────────────

const MAIN_ACTIVITY = 'src-tauri/gen/android/app/src/main/java/com/polaris2/app/MainActivity.kt';
const KT_RAW = readFileSync(join(REPO_ROOT, MAIN_ACTIVITY), 'utf8');

/**
 * **必须剥注释**：`MainActivity.kt` 的头注逐字写着下面要断言的每一个 API 名。不剥的话，
 * 把安装那一行改成 `// installBackNavBridge(webView)` 就能让「收信口装上了」那条恒绿，
 * 而真机上 `window.polarisBackNav` 从未注入 ⇒ 根页按返回静默无反应、运行期零报错（实测过）。
 *
 * 复用 `contracts/rust-source.test-support` 那份共享净化器（Kotlin 与 Rust 的注释/字符串词法同形），
 * 与 `status-bar-theme.test.ts` 同一口径 —— 两份剥法一旦分叉，「那道门挡得住、这道门挡不住」的缝是绿的。
 * 字符串原样保留：桥名就住在字符串里。
 */
const KT = maskRustComments(KT_RAW);

/**
 * 取一个 Kotlin 成员的**函数体**（`fun <name>(` 起、到下一个 `fun` 声明止，再把签名切掉）。
 *
 * 整份文件是弱判据：`installBackNavBridge(webView)` 光靠自己那行定义就「文件里出现过」，
 * 那条断言等于没判。切法与 `status-bar-theme.test.ts` ⑦⑧ 同一套。
 */
function ktBody(src: string, name: string): string {
  const at = src.search(new RegExp(`\\bfun\\s+${name}\\s*\\(`));
  if (at < 0) return '';
  const rest = src.slice(at + 1);
  const next = rest.search(/\n\s*(?:private\s+|override\s+|internal\s+|public\s+)*fun\s/);
  const member = next < 0 ? rest : rest.slice(0, next);
  const brace = member.indexOf('{');
  return brace < 0 ? '' : member.slice(brace + 1);
}

/**
 * `MobileApp.tsx` 的返回键接线形态。三条腿各自独立，缺任意一条都是**某一档静默失灵**：
 *
 *  · `listener` —— 没人监听 `back-button` ⇒ `AppPlugin` 走 `onBackPressed()`，按返回直接退出 App；
 *  · `call` —— 喂给 `handleBackPress` 的**四**样东西按**整次调用**对拍。拆成互不相干的子串就会
 *    漏掉 `goTo: () => undefined` 这种：第二档静默 no-op，而此时原生腿已废 ⇒ 按返回完全没反应，
 *    比接线之前更糟（实测：改掉它，ui 全量仍然全绿）。
 *    🔴 `locked: lockedRef.current` 是第四样（W-16）：喂 `false` 字面量、喂 `locked`（渲染期捕获的
 *    那一帧，监听只注册一次 ⇒ 永远是冷启动那次的 false）、或整条不喂，都会让锁定态的返回键退回
 *    「关一层 / 回落点」两档 —— 遮罩底下的界面被换掉，而遮罩不登记进 back-stack 这件事只挡住了
 *    第一档。故这里按 `lockedRef.current` 逐字对拍；
 *  · `guard` —— 原生桥不在就不接管（见 `MobileApp` 头注：两侧的开关必须是同一个真值）；
 *  · `refs` —— **给那两个 ref 续帧的两行**。🔴 这一条是 2026-09-06 复审补的（major）：
 *    上一版只对拍了调用点 `handleBackPress({… locked: lockedRef.current })`，没有对拍
 *    `lockedRef.current = locked;`。实测把那一行删掉（保留 `useRef(locked)`）⇒ 全量 4300 条全绿，
 *    而 `useRef` 在后续渲染只返回同一个 ref 对象、**不刷新 `.current`**，冷启动时 store 的
 *    `privacyMode` 为 false ⇒ `lockedRef.current` 永远是 false：遮罩挂着的时候按返回仍走原来三档
 *    （先弹掉底下某个真开着的弹层，再在遮罩底下换屏）。`activeRef` 那一行同一条缝，一并钉上。
 */
function backNavWiring(src: string): {
  listener: boolean;
  call: boolean;
  guard: boolean;
  refs: boolean;
} {
  return {
    listener: src.includes("addPluginListener('app', 'back-button'"),
    call: /handleBackPress\(\{\s*active:\s*activeRef\.current,\s*goTo:\s*setActive,\s*exit:\s*exitToBackground,\s*locked:\s*lockedRef\.current,?\s*\}\)/.test(
      src,
    ),
    guard: /if\s*\(!hasBackNavBridge\(\)\)\s*\{/.test(src),
    refs:
      /const\s+activeRef\s*=\s*useRef\(active\);\s*activeRef\.current\s*=\s*active;/.test(src) &&
      /const\s+lockedRef\s*=\s*useRef\(locked\);\s*lockedRef\.current\s*=\s*locked;/.test(src),
  };
}

/**
 * 连接屏那处登记有没有挂在**在场的行**上。
 *
 * `expandedId` 是屏级 state，与「这一行现在还画不画得出来」完全解耦：连接结束（下一帧被移出
 * `activeRows`）或用户翻页，都会让它指向一个不在场的目标。条件只写 `expandedId !== null` 的话，
 * 那一层留在表上 ⇒ 按返回「关」掉一个看不见的东西：屏幕零变化，又因为已经算「关掉了一层」而
 * 不再落到下一档 —— 用户看到的是**返回键没反应**，比接线之前的「按一下退出」更糟。
 *
 * 判据的形状：**交给视图渲染的那批行**（`rows={…}` 的实参）必须出现在登记条件里。
 * 九处登记里只有这一处需要这一步，理由写在生产侧那段注释里。
 */
function rowBoundRegistration(src: string): boolean {
  const rows = /\brows=\{([A-Za-z_$][\w$]*)\}/.exec(src);
  if (rows === null) return false;
  const call = /useDismissableLayer\(([\s\S]*?),\s*collapseRow/.exec(src);
  if (call === null) return false;
  return call[1].includes('expandedId') && call[1].includes(`${rows[1]}.some(`);
}

/** 全部登记点 `文件::函数`（`back-stack.ts` 是定义处，不算调用点）。 */
const REGISTRATION_SITES = [
  ...new Set(
    [...SRC].flatMap(([abs, src]) => {
      if (rel(abs) === 'back-stack.ts') return [];
      const hits: string[] = [];
      let from = src.indexOf('useDismissableLayer(');
      while (from !== -1) {
        const fn = enclosingFunction(src, from);
        if (fn === null) throw new Error(`${rel(abs)} 的登记点不在任何顶层函数内 —— 切片器失效`);
        hits.push(`${rel(abs)}::${fn.name}`);
        from = src.indexOf('useDismissableLayer(', from + 1);
      }
      return hits;
    }),
  ),
].sort();

/**
 * 登记点快照。**不是**「支不支持看登记表」那种登记表：可关闭层的真值面由遮罩扫描（产物侧，
 * 见 [`OVERLAY_RENDERED`]）给出，这一份只多守一件遮罩扫描守不住的事 —— **有人把某处登记删了**
 * （删了之后那一屏静默退化成「这一层不吃返回」，遮罩还在、扫描仍绿），
 * 以及两处不画遮罩的模式态 / 二级页登记（它们根本不在遮罩取材面里）。
 * 加一处登记要在这里补一行，那正是「新弹层落地」该有的动静。
 */
const REGISTERED = [
  'MobileInfo.tsx::MobileInfoPanel',
  'MobileSelect.tsx::MobileSelect',
  'connections/ConnectionsView.tsx::ActionSheet',
  'connections/MobileConnectionsScreen.tsx::MobileConnectionsScreen',
  /* 表单宿主那一层（批 2）。登记放在**弹层外壳**身上而不是六个面板各写一处：
     以后新表单复用 `FormSheet` 就自动带上，「忘了登记」的表现降级成「这一层不吃返回」。 */
  'forms/FormSheet.tsx::FormSheet',
  'home/HomeScreenView.tsx::Scrim',
  'nodes/MobileNodesScreen.tsx::MobileNodesScreen',
  'nodes/NodesScreenView.tsx::Sheet',
  'screens/rules/Primitives.tsx::ActionSheet',
  'screens/rules/Primitives.tsx::SelectSheetPanel',
  'screens/rules/RulesScreen.tsx::useRulesStack',
  'settings/MobileSettingsScreen.tsx::useSettingsStack',
];

/**
 * 遮罩渲染点快照 —— ② 组的**主判据**（理由见文件头注「为什么以恰等快照为主判据」）。
 *
 * 归属靠切片器，而切片器是过近似：一层遮罩内联进已经为别的层登记过的大屏组件时，`registers`
 * 会假真。恰等快照绕开这件事 —— 新增/挪动/改名任何一处遮罩都让集合不等而当场红，
 * 逼一次显式登记。加一套弹层要在这里补一行，那正是「新弹层落地」该有的动静。
 */
const OVERLAY_RENDERED = [
  // Read-only information portal shares form geometry, but owns its independent top back layer.
  'MobileInfo.tsx::MobileInfoPanel::m-form-scrim',
  'MobileSelect.tsx::MobileSelect::m-select-layer',
  'connections/ConnectionsView.tsx::ActionSheet::mc-scrim',
  /* 表单宿主（批 2）：`m-form-layer` 是全视口 fixed 容器（B 形态腿捞到的），
     `m-form-scrim` 是它里面那颗真的拦点击的背幕按钮（A 名字腿）。两条腿各捞到一个，
     且两个都住在同一个会登记的 `FormSheet` 里。 */
  'forms/FormSheet.tsx::FormSheet::m-form-layer',
  'forms/FormSheet.tsx::FormSheet::m-form-scrim',
  'home/HomeScreenView.tsx::Scrim::h-scrim',
  'nodes/NodesScreenView.tsx::Sheet::mn-sheet-layer',
  'nodes/NodesScreenView.tsx::Sheet::mn-sheet-scrim',
  'screens/rules/Primitives.tsx::ActionSheet::mr-sheet-scrim',
  'screens/rules/Primitives.tsx::SelectSheetPanel::mr-sheet-scrim',
];

// ── 渲染期深度探针 ───────────────────────────────────────────────────────────

/**
 * 渲染期读栈深。**它自己就是「登记发生在渲染期」的证明**：探针排在被测层之后渲染，
 * 若登记挪进了 `useEffect`，这里读到的永远是 0。
 */
function DepthProbe(): ReactElement {
  return <i data-depth={useDismissableDepth()} />;
}

/**
 * 量**这一棵树自己**登记了几层，故先排空。
 *
 * 不排空的话，同一个 `it` 里前面那次渲染留下的层会一起算进来；而 `useId` 是**按渲染趟**分配的，
 * 两趟独立的 `renderToStaticMarkup` 可能发出同一个 id ⇒ 深度到底是 1 还是 2 取决于 id 撞没撞上。
 * 那种判据看着绿，其实测的是 React 的 id 分配策略。
 */
function depthAfter(node: ReactElement): number {
  drain();
  const html = renderToStaticMarkup(
    <>
      {node}
      <DepthProbe />
    </>,
  );
  const m = /data-depth="(\d+)"/.exec(html);
  if (m === null) throw new Error('深度探针没渲染出来 —— 判据面塌了');
  return Number(m[1]);
}

/**
 * **当前**栈深（不排空）—— 用于「同一棵树上连按两次返回」这种跨调用的观察。
 *
 * 与 [`depthAfter`] 的分工：那个量「这一趟渲染登记了几层」（先排空，故读的是净增量），
 * 这个量「此刻表上还剩几层」。锁定层那条判据要的正是后者：第一下返回之后层还在不在。
 */
function depthNow(): number {
  const html = renderToStaticMarkup(<DepthProbe />);
  const m = /data-depth="(\d+)"/.exec(html);
  if (m === null) throw new Error('深度探针没渲染出来 —— 判据面塌了');
  return Number(m[1]);
}

/**
 * 排空登记表。顺带自检 `dismissTop` 真的在出栈（否则这里会死循环）。
 *
 * 末尾那一下 `clearDismissables()` 是给**锁定层**收尾的：`closeLocked` 的表单拒绝关闭、
 * `dismissTop` 会把它压回原位（那正是本文件下面那条判据要的行为）⇒ 这张表靠 `dismissTop`
 * 排不空。先按正常路径排到底（保住上面那条自检的信息量），再把剩下的拒关层清掉。
 */
// eslint-disable-next-line @typescript-eslint/no-use-before-define -- 提升函数声明，供上面的探针复用
function drain(): void {
  let n = 0;
  let before = depthNow();
  while (before > 0 && dismissTop()) {
    const after = depthNow();
    if (after >= before) break; // 顶层拒绝关闭：再调也是同一个结果
    before = after;
    n += 1;
    if (n > 64) throw new Error('`dismissTop` 排不空登记表 —— 它没有在出栈');
  }
  clearDismissables();
}

beforeEach(() => {
  drain();
  setPushedPage('rules', null);
  setPushedPage('settings', null);
});

// ═══════════════════════════════════════════════════════════════════════════

describe('⓪ 自检：三块判据面都是活的（否则下面每条都恒绿）', () => {
  it('取材面非空、有量级，且**本文件不在里面**（判据自污染防线）', () => {
    expect(CSS_FILES.length, '一份移动端 CSS 都没扫到').toBeGreaterThan(3);
    expect(SRC_FILES.length, '一份移动端源码都没扫到').toBeGreaterThan(20);
    expect(SRC_FILES.map(rel)).not.toContain('back-navigation.test.tsx');
    expect(
      [...SRC].some(([, s]) => s.includes('useDismissableLayer(')),
      '消费面里一处登记都没有 —— 剥注释把源码吃掉了？',
    ).toBe(true);
  });

  it('剥注释保留字符串、且真的把注释里的反例挡在外面', () => {
    const synthetic = '/* 反例：className="x-scrim" 不算 */\nconst a = "y-scrim";\n';
    expect(stripTs(synthetic)).not.toContain('x-scrim');
    expect(stripTs(synthetic)).toContain('"y-scrim"');
    // 字符串里的 `/*` 不是注释起点（mobile-entry 那道门在这个真实输入上塌过）。
    const glob = "const ignored = ['**/src-tauri/**'];\nconst b = 1;\n";
    expect(stripTs(glob)).toContain('src-tauri');
    expect(stripTs(glob)).toContain('const b = 1');
  });

  it('切片器：位置解析得出函数名、切片含该位置、且**短于整份文件**（不是退化成整文件）', () => {
    const src = 'function A() {\n  return "a-scrim";\n}\nexport function B() {\n  return 2;\n}\n';
    const at = src.indexOf('"a-scrim"');
    const fn = enclosingFunction(src, at);
    expect(fn?.name).toBe('A');
    expect(fn?.body).toContain('"a-scrim"');
    expect(fn?.body).not.toContain('return 2');
    expect(fn?.span).toBeLessThan(src.length);
    // 反向对照：落在第二个函数里的位置解析成 B，而不是恒返回第一个。
    expect(enclosingFunction(src, src.indexOf('return 2'))?.name).toBe('B');
  });

  it('🔴 切片器认得**箭头函数组件**（否则它里面的遮罩会继承上一个 `function` 的登记）', () => {
    const src =
      'function A() {\n  useDismissableLayer(true, x);\n  return 1;\n}\n' +
      'export const B = ({ onClose }) => <div className="b-scrim" onClick={onClose} />;\n';
    const fn = enclosingFunction(src, src.indexOf('"b-scrim"'));
    expect(fn?.name, '箭头组件不在声明表里 —— 遮罩被归给了上一个 `function`').toBe('B');
    expect(
      fn?.body.includes('useDismissableLayer('),
      '箭头组件继承了别人的登记 —— 这不是漏报，是错报成已登记',
    ).toBe(false);
  });

  it('切片器：返回类型在列 0 收口的 hook 不被截断（`function f(): {…} {` 的形态）', () => {
    const src =
      'function useStack(): {\n  page: string | null;\n} {\n  useDismissableLayer(true, x);\n  return "s-scrim";\n}\n';
    const fn = enclosingFunction(src, src.indexOf('"s-scrim"'));
    expect(fn?.name, '切片在返回类型那个 `}` 上就断了').toBe('useStack');
    expect(fn?.body).toContain('useDismissableLayer(');
  });

  it('切片器：落在两个顶层声明**之间**的位置返回 null（不悄悄归给上一个函数）', () => {
    const src = 'function A() {\n  return 1;\n}\nconst X = "c-scrim";\nfunction B() {\n  return 2;\n}\n';
    expect(enclosingFunction(src, src.indexOf('"c-scrim"')), '模块级的位置被归给了函数 A').toBeNull();
  });

  it('深度探针在栈空时读 0（否则「登记了」这件事恒真）', () => {
    expect(depthAfter(<span />)).toBe(0);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ① `dismissTop` 的纯函数语义。返回键的全部顺序都压在这一条上：
 * 弹了错的那一层 = 用户按返回，最上面那层还在、底下那层没了。
 */
describe('① 登记表：LIFO / 空栈 / 退订 / 幂等', () => {
  it('空栈返 `false`（正面对照：非空时返 `true`）', () => {
    expect(dismissTop()).toBe(false);
    pushDismissable('a', () => undefined);
    expect(dismissTop()).toBe(true);
    expect(dismissTop()).toBe(false);
  });

  it('🔴 后登记的先关（LIFO），且**只关一层**', () => {
    const order: string[] = [];
    pushDismissable('outer', () => {
      order.push('outer');
    });
    pushDismissable('inner', () => {
      order.push('inner');
    });
    expect(dismissTop()).toBe(true);
    expect(order, '关掉的不是最上面那一层').toEqual(['inner']);
    expect(dismissTop()).toBe(true);
    expect(order, '一次返回关掉了不止一层').toEqual(['inner', 'outer']);
  });

  it('退订之后不再被弹（关掉的层不许再吃一次返回）', () => {
    const hit: string[] = [];
    const off = pushDismissable('a', () => {
      hit.push('a');
    });
    pushDismissable('b', () => {
      hit.push('b');
    });
    off();
    expect(dismissTop()).toBe(true);
    expect(hit).toEqual(['b']);
    expect(dismissTop(), '退订过的层还留在表上').toBe(false);
  });

  it('重复退订幂等，且不会误摘别人那一层', () => {
    const hit: string[] = [];
    const off = pushDismissable('a', () => {
      hit.push('a');
    });
    off();
    off();
    pushDismissable('b', () => {
      hit.push('b');
    });
    off();
    expect(dismissTop()).toBe(true);
    expect(hit, '幂等退订把别人那一层摘掉了').toEqual(['b']);
  });

  it('同 key 重登记：层序不变，但调用的是**最后一帧**的闭包（重渲染的形态）', () => {
    const hit: string[] = [];
    pushDismissable('a', () => {
      hit.push('a1');
    });
    pushDismissable('b', () => {
      hit.push('b');
    });
    pushDismissable('a', () => {
      hit.push('a2');
    }); // a 重渲染
    dismissTop();
    expect(hit, '重登记把 a 顶到了最上面').toEqual(['b']);
    dismissTop();
    expect(hit, '调到的是过期闭包').toEqual(['b', 'a2']);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ② 结构性门：让「忘了登记」自曝。
 *
 * A 腿（扫描）说的是「画了背幕的那个函数得登记」，B 腿（真渲染）说的是「登记确实生效」。
 * 两条都要：只有 A，`useDismissableLayer(false, …)` 也算过；只有 B，新加的第五套弹层没人去渲染它。
 */
describe('② 每一层背幕都由一个会登记的组件渲染', () => {
  it('自检：遮罩类扫出来了、每一个都有人渲染，且**形态腿真的多捞到了东西**', () => {
    expect(OVERLAY_CLASSES.length, 'CSS 里一个遮罩类都没扫到').toBeGreaterThanOrEqual(5);
    for (const cls of OVERLAY_CLASSES) {
      expect(
        OVERLAY_SITES.filter((s) => s.cls === cls).length,
        `CSS 里的 \`.${cls}\` 没有任何 .tsx 渲染它 —— 要么是死规则，要么 class 名是拼出来的（本门看不见）`,
      ).toBeGreaterThan(0);
    }
    /* 并集不是写着好看：至少有一个类**只**靠形态腿进来（名字里没有 `scrim`）。
       这条塌了就说明形态腿退化成了名字腿的子集，「换个名字就绕开」那条路重新打开。 */
    expect(
      OVERLAY_CLASSES.filter((c) => !c.includes('scrim')),
      '没有任何一个遮罩类是靠 `position:fixed + inset:0` 捞进来的 —— 形态腿是死的',
    ).not.toEqual([]);
  });

  it('🔴 遮罩渲染点恰等快照（新加一套弹层 ⇒ 当场红，与切片器准不准无关）', () => {
    expect(
      OVERLAY_SITE_KEYS,
      '多出来的那处遮罩没有登记可关闭层就不吃返回；少掉的那处要么被删了、要么 class 名改成了拼装写法',
    ).toEqual(OVERLAY_RENDERED);
  });

  it('🔴 每一处遮罩所在的组件都调了登记（更早一步的诊断腿）', () => {
    const missing = OVERLAY_SITES.filter((s) => !s.registers).map(
      (s) => `${s.file}::${s.fn} 画了 .${s.cls} 却没有登记可关闭层`,
    );
    expect(
      missing,
      '这些弹层不吃系统返回键 —— 在它们上面按返回会穿到下一档（回落点或退出 App）',
    ).toEqual([]);
  });

  it('反向对照：检查器对着"画了遮罩但没登记"的三种写法都报得出来', () => {
    const check = (src: string, cls: string): boolean => {
      const stripped = stripTs(src);
      const at = stripped.indexOf(`"${cls}"`) >= 0 ? stripped.indexOf(`"${cls}"`) : stripped.indexOf(`'${cls}'`);
      expect(at, `合成源码里找不到 .${cls} —— 这条对照自己塌了`).toBeGreaterThanOrEqual(0);
      return enclosingFunction(stripped, at)?.body.includes('useDismissableLayer(') === true;
    };
    // ① `function` 写法：最朴素的一条。
    const bad = 'export function NewSheet({ onClose }) {\n  return <div className="z-scrim" onClick={onClose} />;\n}\n';
    const good = bad.replace('return <div', 'useDismissableLayer(true, onClose);\n  return <div');
    expect(check(bad, 'z-scrim'), '检查器对着真违规是绿的 —— 它认不出"什么都没做"').toBe(false);
    expect(check(good, 'z-scrim'), '检查器对着合规写法也报红 —— 它在乱报').toBe(true);
    // ② 箭头组件排在一个**会登记**的函数之后：切片器若不认它，这里会假绿。
    const arrow =
      'export function Old({ onClose }) {\n  useDismissableLayer(true, onClose);\n  return <b />;\n}\n' +
      'export const NewScrim = ({ onClose }) => <button className="z-scrim" onClick={onClose} />;\n';
    expect(check(arrow, 'z-scrim'), '箭头组件继承了上一个函数的登记').toBe(false);
    // ③ 单引号字面量：needle 只认双引号的话，这一处整条不进取材面。
    const single = "export function Q({ onClose }) {\n  return <div className='z-scrim' onClick={onClose} />;\n}\n";
    expect(check(single, 'z-scrim'), '单引号写的 class 名没进取材面').toBe(false);
    expect(
      [...stripTs(single).matchAll(new RegExp(`["']z-scrim["']`, 'g'))].length,
      'needle 认不出单引号字面量',
    ).toBe(1);
  });

  it('登记点集合恰等于快照（有人删掉一处登记时，背幕还在、只有这条会红）', () => {
    expect(REGISTRATION_SITES).toEqual(REGISTERED);
  });

  /**
   * 两处**模式态**（节点屏批选、连接屏展开行）没有行为腿：它们的开合是屏内 `useState`，
   * 容器外驱动不动（本仓无 jsdom，点不了）。这条是它们能拿到的最强的源码级断言 ——
   * 「登记了但第一个实参恒为 `false`」正是那种「看着接了、其实这一层永远不吃返回」的形态。
   */
  it('没有一处登记把开关写死成 `false`（登记了却恒关 = 门在但没牙）', () => {
    const dead: string[] = [];
    for (const [abs, src] of SRC) {
      if (rel(abs) === 'back-stack.ts') continue;
      for (const m of src.matchAll(/useDismissableLayer\(\s*(false|true\s*&&)/g)) {
        dead.push(`${rel(abs)}: useDismissableLayer(${m[1]}…)`);
      }
    }
    expect(dead, '这些层永远不会被登记').toEqual([]);
    // 反向对照：同一条正则对着写死 false 的合成源码确实报得出来。
    expect(
      [...'useDismissableLayer(false, onClose);'.matchAll(/useDismissableLayer\(\s*(false|true\s*&&)/g)].length,
    ).toBe(1);
  });

  it('🔴 连接屏展开行：登记条件挂在**交给视图渲染的那批行**上，不是「`expandedId` 非空」', () => {
    const src = SRC.get(join(MOBILE, 'connections/MobileConnectionsScreen.tsx'));
    expect(src, '连接屏不在取材面里 —— 判据面塌了').toBeDefined();
    expect(
      rowBoundRegistration(src as string),
      '展开的那一行结束或翻页滑出窗口后，这一层还留在表上 —— 按返回屏幕零变化、也不落到下一档',
    ).toBe(true);
    // 反向对照：同一个检查器对着旧写法（只判非空）确实报得出来。
    const loose = (src as string).replace(
      /useDismissableLayer\([\s\S]*?,\s*collapseRow/,
      'useDismissableLayer(expandedId !== null, collapseRow',
    );
    expect(loose, '变异没落上 —— 这条对照自己塌了').not.toBe(src);
    expect(rowBoundRegistration(loose), '检查器对着旧写法是绿的 —— 它认不出"什么都没做"').toBe(false);
  });

  it('两屏的模式态在冷启动首帧不占层（否则"进屏就有一层"，返回键第一下会空按）', () => {
    expect(depthAfter(<MobileNodesScreen />), '节点屏首帧就登记了一层').toBe(0);
    expect(depthAfter(<MobileConnectionsScreen />), '连接屏首帧就登记了一层').toBe(0);
  });

  it('🔴 行为腿：五套弹层各渲染一次，每一套都让栈深 +1', () => {
    const noop = (): void => undefined;
    const cases: Array<[string, ReactElement]> = [
      ['home/Scrim', <Scrim onClose={noop} label="close" />],
      /* 表单弹层（批 2）。此前只补了**扫描腿**（`REGISTERED` / `OVERLAY_RENDERED` 两张源码
         快照），而那张死开关扫描器只认字面量 `false` / `true &&`：实测把登记条件改成
         `useDismissableLayer(props.closeLocked === true, …)`（常态恒 false ⇒ 恒不登记）
         ⇒ tsc rc=0、全量 vitest 全绿，而表单开着按返回不关表单、直接落到「回首页」，
         用户填了一半的节点表单当场没了。`FormSheet` 不 `useTranslation()`、文案全走 props，
         `renderToStaticMarkup` 渲染得动 ⇒ 行为腿在这里补得上。 */
      [
        'forms/FormSheet',
        <FormSheet title="t" onRequestClose={noop} closeLabel="c" cancelLabel="c">
          {null}
        </FormSheet>,
      ],
      [
        'nodes/Sheet',
        <NodesSheet title="t" closeLabel="c" onClose={noop}>
          {null}
        </NodesSheet>,
      ],
      [
        'connections/ActionSheet',
        <ConnectionsSheet sheet={{ title: 't', items: [] }} onClose={noop} closeLabel="c" />,
      ],
      ['rules/ActionSheet', <RulesSheet title="t" actions={[]} onClose={noop} closeLabel="c" />],
      [
        'rules/SelectSheetPanel',
        <SelectSheetPanel
          label="l"
          options={[]}
          value=""
          open
          onClose={noop}
          onSelect={noop}
          closeLabel="c"
        />,
      ],
    ];
    for (const [name, node] of cases) {
      drain();
      expect(depthAfter(node), `${name} 渲染出来了却没登记 —— 它不吃返回`).toBe(1);
    }
    drain();
  });

  it('反向对照：面板关着时**不**登记（否则"登记了"这件事恒真，上一条没有信息量）', () => {
    const noop = (): void => undefined;
    expect(
      depthAfter(
        <SelectSheetPanel
          label="l"
          options={[]}
          value=""
          open={false}
          onClose={noop}
          onSelect={noop}
          closeLabel="c"
        />,
      ),
      '面板关着也占着一层 —— 按返回会先"关"一个根本没开的东西',
    ).toBe(0);
  });

  /*
   * 🔴 **锁住的表单：连按两次返回，第二次仍须捞到这一层**（2026-09-06 三条复审同时点名）。
   *
   * `dismissTop` 是**先出栈再调用**（`back-stack.ts` 里那条顺序有注释说明是刻意的），而
   * `FormSheet` 在 `closeLocked` 时此前直接 `return` ⇒ 层已经从表上摘了、表单还开着，
   * 而 `handleBackPress` 见 `dismissTop()` 返 true 即判 'dismissed' 不再往下走。
   * 落到生产：`SubFormPanel:244 closeLocked={busy||operationBusy}`（订阅创建那几秒里没有任何
   * 渲染触发，登记不会自愈）、`ImportFormPanel:189 closeLocked={importing}`、
   * `NodeFormPanel:335 closeLocked={submitting}`、`TsExitPanel` 同形 ⇒
   * 第一下什么都不发生但层没了、第二下回首页、第三下退到后台，而那次写操作仍在后端跑。
   *
   * 修法是让「我没关」可表达（`DismissFn` 返回 `false` ⇒ `dismissTop` 压回原位）。
   * 本组两条**成对**：锁住时层还在且闭包没被调；不锁时正常关掉。
   */
  it('🔴 锁住的表单：返回被吃掉但层不许丢（连按两次，第二次仍捞得到）', () => {
    let closed = 0;
    drain();
    renderToStaticMarkup(
      <FormSheet
        title="t"
        closeLocked
        onRequestClose={() => {
          closed += 1;
        }}
        closeLabel="c"
        cancelLabel="c"
      >
        {null}
      </FormSheet>,
    );
    expect(depthNow(), '锁住的表单压根没登记 —— 那一下返回会直接落到应用级分支').toBe(1);
    expect(dismissTop(), '这一下返回没有被表单吃掉（会继续落到「回首页」）').toBe(true);
    expect(closed, '锁住了却还是把表单关了 —— 写操作会留下孤儿 continuation').toBe(0);
    expect(
      depthNow(),
      '第一下返回把这一层从登记表上静默摘掉了 —— 第二下就落到「回首页」、第三下退出 App，' +
        '而那次创建/导入仍在后端跑且移动端没有恢复面',
    ).toBe(1);
    expect(dismissTop(), '第二下返回捞不到这一层了').toBe(true);
    expect(closed).toBe(0);
    drain();
  });

  it('正面对照：**没**锁住的同一个组件，一下返回就真的关掉了（否则上一条只是「永远不关」）', () => {
    let closed = 0;
    drain();
    renderToStaticMarkup(
      <FormSheet
        title="t"
        onRequestClose={() => {
          closed += 1;
        }}
        closeLabel="c"
        cancelLabel="c"
      >
        {null}
      </FormSheet>,
    );
    expect(dismissTop()).toBe(true);
    expect(closed, '未锁定的表单按返回没关').toBe(1);
    expect(depthNow(), '关掉之后这一层还留在表上').toBe(0);
    drain();
  });

  it('弹出来的就是那一层自己的关闭闭包（不是随便一个能跑的函数）', () => {
    const hit: string[] = [];
    renderToStaticMarkup(<Scrim onClose={() => hit.push('closed')} label="close" />);
    expect(dismissTop()).toBe(true);
    expect(hit).toEqual(['closed']);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ③ 根页不许变成哑巴。
 *
 * 这一组守的是本方案**自己制造**的那个新缺陷面：注册了 `back-button` 监听之后，
 * `AppPlugin` 的原生兜底整条不走了 —— 根页那一跳若 return 掉，用户按返回**毫无反应**。
 */
describe('③ 返回键的三档：关一层 / 回落点 / 交还系统', () => {
  const spy = (): { calls: DestinationId[]; fn: (id: DestinationId) => void } => {
    const calls: DestinationId[] = [];
    return { calls, fn: (id) => calls.push(id) };
  };

  const run = (
    active: DestinationId,
    locked = false,
  ): { outcome: BackOutcome; goTo: DestinationId[]; exits: number } => {
    const go = spy();
    let exits = 0;
    const outcome = handleBackPress({
      active,
      goTo: go.fn,
      exit: () => {
        exits += 1;
      },
      locked,
    });
    return { outcome, goTo: go.calls, exits };
  };

  it('🔴 栈空 + 停在冷启动落点 ⇒ **调用了交还系统那条腿**（不是 return 掉）', () => {
    const r = run(DEFAULT_DESTINATION);
    expect(r.outcome).toBe('exit');
    expect(r.exits, '根页按返回什么都没发生 —— 比接线之前的"直接退出"更糟').toBe(1);
    expect(r.goTo, '根页按返回却去导航了').toEqual([]);
  });

  it('不在落点 ⇒ 回落点，且**不**交还系统', () => {
    const other: DestinationId = 'settings';
    expect(other).not.toBe(DEFAULT_DESTINATION); // 自检：夹具真的不是落点
    const r = run(other);
    expect(r.outcome).toBe('to-root');
    expect(r.goTo).toEqual([DEFAULT_DESTINATION]);
    expect(r.exits, '还没回落点就把应用交还系统了').toBe(0);
  });

  it('有可关闭层 ⇒ 只关那一层，既不导航也不交还系统（三档互斥）', () => {
    const hit: string[] = [];
    pushDismissable('sheet', () => {
      hit.push('closed');
    });
    const r = run(DEFAULT_DESTINATION);
    expect(r.outcome).toBe('dismissed');
    expect(hit).toEqual(['closed']);
    expect(r.exits, '弹层还开着就把应用交还系统了').toBe(0);
    expect(r.goTo).toEqual([]);
  });

  /*
   * 🔴 隐私锁定态（W-16）：**第四档，排在三档之前**。
   *
   * 锁屏遮罩刻意不登记进 `back-stack`（登记了按返回就把它 dismiss 掉 = 返回键成了解锁键）。
   * 但「不登记」只解决第一档：第二档会在遮罩底下换屏，而那是锁定期间界面状态不该动的东西。
   * 故 `locked` 为真时 `handleBackPress` 早退到「交还系统」，且**连 `dismissTop()` 都不调**
   * —— 调了会顺手弹掉底下某个真的还开着的弹层，用户解锁回来发现它没了。
   */
  it('🔴 锁定 + 停在二级目的地 ⇒ 交还系统，**不导航**（遮罩底下的界面一格都不许动）', () => {
    const other: DestinationId = 'settings';
    expect(other).not.toBe(DEFAULT_DESTINATION); // 自检：夹具真的不是落点
    const r = run(other, true);
    expect(r.outcome).toBe('exit');
    expect(r.exits, '锁定时按返回什么都没发生').toBe(1);
    expect(r.goTo, '锁定时按返回把遮罩底下的界面换掉了').toEqual([]);
    // 反向对照：同一个目的地、没锁 ⇒ 走第二档。证明上面那条不是「run 恒返 exit」。
    expect(run(other, false).outcome).toBe('to-root');
  });

  it('🔴 锁定时**不动**可关闭层栈（返回键不许顺手关掉底下那层弹层）', () => {
    const hit: string[] = [];
    pushDismissable('sheet', () => void hit.push('closed'));
    const r = run(DEFAULT_DESTINATION, true);
    expect(r.outcome).toBe('exit');
    expect(hit, '锁定时按返回把底下那层弹层关掉了 —— 用户解锁回来发现它没了').toEqual([]);
    // 正向对照：同一条栈、没锁 ⇒ 第一档照常关掉它（证明这条栈真的是可关的）。
    expect(run(DEFAULT_DESTINATION, false).outcome).toBe('dismissed');
    expect(hit).toEqual(['closed']);
  });

  it('🔴 交还系统这条腿真的往桥上投递了（正向对照：桥不在时不抛）', () => {
    const g = globalThis as unknown as Record<string, unknown>;
    const before = g[BACK_NAV_BRIDGE];
    const posted: string[] = [];
    g[BACK_NAV_BRIDGE] = { postMessage: (m: string) => posted.push(m) };
    try {
      exitToBackground();
      expect(posted, '桥在场却没收到任何东西 —— 根页那一跳是个空壳').toEqual([BACK_NAV_EXIT]);
    } finally {
      delete g[BACK_NAV_BRIDGE];
    }
    // 反向对照：桥不在（桌面 / 浏览器调试档）时静默返回，不抛。
    expect(() => exitToBackground()).not.toThrow();
    if (before !== undefined) g[BACK_NAV_BRIDGE] = before;
  });

  it('自检：Kotlin 取材面是活的，注释真的被剥掉了（不剥的话下面每条都能被一行 `//` 骗过）', () => {
    expect(KT_RAW.length, `${MAIN_ACTIVITY} 读到的是空文件 —— 判据面塌了`).toBeGreaterThan(400);
    expect(KT, '净化后什么都不剩 —— 剥法把代码一起吃了').toContain('class MainActivity');
    // 正面对照：`addJavascriptInterface` 只出现在头注里（本条是下面那条否定断言的前提）。
    expect(KT_RAW, '头注里连 `addJavascriptInterface` 都没有了 —— 这条对照失效').toContain(
      'addJavascriptInterface',
    );
    expect(KT, '注释没剥干净 —— 下面的断言可能是注释在充数').not.toContain('addJavascriptInterface');
    expect(ktBody(KT, 'onWebViewCreate').length, '`onWebViewCreate` 的函数体切不出来').toBeGreaterThan(60);
    expect(
      ktBody(KT, 'installBackNavBridge').length,
      '`installBackNavBridge` 的函数体切不出来',
    ).toBeGreaterThan(60);
  });

  it('🔴 跨语言：Kotlin 侧那条桥的**名字**与本侧逐字相同，且收到即 `moveTaskToBack`', () => {
    const name = /private const val BACK_NAV_BRIDGE\s*=\s*"([^"]+)"/.exec(KT);
    expect(name, 'Kotlin 侧找不到 `BACK_NAV_BRIDGE` —— 判据面塌了（改名了？）').not.toBeNull();
    expect(
      name?.[1],
      '两侧桥名漂了 —— `postMessage` 会打在 undefined 上，根页按返回静默无反应、运行期零报错',
    ).toBe(BACK_NAV_BRIDGE);
    // 收信口装在 **WebView 建好那一刻**：切进 `onWebViewCreate` 的函数体，不是"整份文件里出现过"
    //（后者光靠 `installBackNavBridge` 自己那行定义就恒真）。
    expect(
      ktBody(KT, 'onWebViewCreate'),
      '收信口没装 —— `window.polarisBackNav` 永远不存在，根页按返回静默无反应',
    ).toContain('installBackNavBridge(webView)');
    // 兑现的是「收起界面」，不是 `finish()`（VPN 客户端要留住任务栈与前台服务）。
    const fn = ktBody(KT, 'installBackNavBridge');
    expect(fn).toContain('WebViewCompat.addWebMessageListener(webView, BACK_NAV_BRIDGE');
    expect(fn).toContain('moveTaskToBack(true)');
    expect(fn, '根页那一跳改成了 finish()：任务栈与 VPN 前台服务会一起没').not.toContain('finish()');
  });

  it('反向对照：把安装那一行**注释掉**，同一套检查器当场报得出来（不剥注释时它恒绿）', () => {
    const commentedRaw = KT_RAW.replace(
      '    installBackNavBridge(webView)',
      '    // installBackNavBridge(webView)',
    );
    expect(commentedRaw, '变异没落上 —— 这条对照自己塌了').not.toBe(KT_RAW);
    expect(
      ktBody(maskRustComments(commentedRaw), 'onWebViewCreate'),
      '注释掉安装那一行之后判据还是绿的 —— 剥注释这条腿是死的',
    ).not.toContain('installBackNavBridge(webView)');
    // 正面对照：同一套检查器对着**没改**的源码仍然认得出它装上了（不是恒红）。
    expect(ktBody(KT, 'onWebViewCreate')).toContain('installBackNavBridge(webView)');
  });

  it('🔴 生产接线：监听注册了，且喂给处理器的三样东西按**整次调用**对得上', () => {
    const app = SRC.get(join(MOBILE, 'MobileApp.tsx'));
    expect(app, 'MobileApp.tsx 不在取材面里 —— 判据面塌了').toBeDefined();
    expect(
      backNavWiring(app as string),
      'listener 假 = 按返回直接退出 App；call 假 = 某一档静默 no-op；guard 假 = 桥缺席时返回键彻底失灵；' +
        'refs 假 = 监听读到的永远是冷启动那一帧（锁定态/当前目的地都停在初值）',
    ).toEqual({ listener: true, call: true, guard: true, refs: true });

    // 反向对照：三条腿逐条断掉，检查器必须**只**报被断的那一条（笼统地红等于没有分辨率）。
    const cut = (from: string, to: string): ReturnType<typeof backNavWiring> => {
      const mutated = (app as string).replace(from, to);
      expect(mutated, `变异 \`${from}\` 没落上 —— 这条对照自己塌了`).not.toBe(app);
      return backNavWiring(mutated);
    };
    expect(cut("addPluginListener('app', 'back-button'", 'noop(')).toEqual({
      listener: false,
      call: true,
      guard: true,
      refs: true,
    });
    expect(cut('goTo: setActive', 'goTo: () => undefined')).toEqual({
      listener: true,
      call: false,
      guard: true,
      refs: true,
    });
    expect(cut('active: activeRef.current', 'active: DEFAULT_DESTINATION')).toEqual({
      listener: true,
      call: false,
      guard: true,
      refs: true,
    });
    expect(cut('if (!hasBackNavBridge()) {', 'if (false) {')).toEqual({
      listener: true,
      call: true,
      guard: false,
      refs: true,
    });
    // 🔴 续帧那两行各断一次：删掉之后监听读到的永远是冷启动那一帧（实测这个变异此前全绿）。
    expect(cut('  lockedRef.current = locked;\n', '')).toEqual({
      listener: true,
      call: true,
      guard: true,
      refs: false,
    });
    expect(cut('  activeRef.current = active;\n', '')).toEqual({
      listener: true,
      call: true,
      guard: true,
      refs: false,
    });
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ④ 规则屏二级页：不重复画底部导航 + 吃返回。
 *
 * 这是一个**独立于返回键**的既有缺陷：规则屏的 `page` 此前是屏内 `useState`，外壳读不到 ⇒
 * `MobileShell` 的 `pushed` 恒 false ⇒ 两个 DNS 二级页上照常画着五格导航
 * （`mobile-screen-shell.md#Variants` / IA §2.4 明文禁止）。设置屏是做对的那条腿，
 * 故这不是刻意取舍，是漏接 —— 判据渲染的是**整棵生产树**，判的是接线不是机制。
 */
describe('④ 二级页：无底部导航，且系统返回键退得回来', () => {
  const shell = (active: DestinationId, child: ReactElement): string =>
    renderToStaticMarkup(
      <MobileShell active={active} onSelect={() => undefined}>
        {child}
      </MobileShell>,
    );
  const rulesTree = (): string => shell('rules', <MobileRulesScreen />);

  it('自检 + 正面对照：规则屏根页上导航真的画得出来（否则下面每条都恒绿）', () => {
    const html = rulesTree();
    expect(html).toContain('class="m-nav"');
    // 分段条只在根页画（二级页是另一条渲染分支）⇒ 它就是"这棵树停在根页"的结构标记。
    expect(html, '渲染出来的不是规则屏根页').toContain('role="tablist"');
  });

  it('🔴 压栈之后：同一棵生产树上导航消失、二级页出现、停靠区留着', () => {
    setPushedPage('rules', 'dns-servers');
    const html = rulesTree();
    expect(html, '规则屏二级页上仍然画着底部导航').not.toContain('class="m-nav"');
    expect(html, '屏没读外壳那一格 —— 二级页没换出来').toContain(
      tr('rules.dnsWorkspace.serversTab'),
    );
    expect(html, '二级页上还画着分段条').not.toContain('role="tablist"');
    expect(html, '停靠区本体没了：二级页最后一行会压在手势条底下').toContain('class="m-dock"');
  });

  it('🔴 二级页登记了可关闭层，`dismissTop` 退得回根页（返回键在这一屏的全链路）', () => {
    setPushedPage('rules', 'dns-servers');
    expect(depthAfter(<MobileRulesScreen />), '二级页没登记 —— 在它上面按返回会直接回落点').toBe(1);
    expect(dismissTop()).toBe(true);
    const html = rulesTree();
    expect(html, '关掉之后没回到根页').toContain('class="m-nav"');
    expect(html).toContain('role="tablist"');
  });

  it('反向对照：根页上不登记任何层（否则上一条测的只是"栈里总有东西"）', () => {
    expect(depthAfter(<MobileRulesScreen />)).toBe(0);
  });

  /**
   * 收窄在读回处做：不在页表里的值一律当"停在根页"，不渲染一页空白、也不占返回栈的一层。
   *
   * ⚠️ 外壳那一侧**不收窄**（`MobileShell` 只判 `!== null`）⇒ 此时导航仍然不画。
   * 这是外壳既有的口径，设置屏那条腿（`MobileSettings.test.tsx` ⑮ 末条）同样如此，
   * 不是本批引入的，也不在本批射程内 —— 这里如实钉住现状，改了会红。
   */
  it('栈里塞一个不认识的页 id 时屏当作根页（不渲染一页空白，也不占一层）', () => {
    setPushedPage('rules', 'not-a-page');
    const html = rulesTree();
    expect(html, '不认识的页 id 渲染出了一页空白').toContain('role="tablist"');
    expect(depthAfter(<MobileRulesScreen />)).toBe(0);
  });

  it('正向对照：设置屏二级页同样无导航、同样吃返回（做对的那条腿没被改坏）', () => {
    useAppStore.setState({ config: {} as unknown as UserConfig });
    setPushedPage('settings', 'about');
    /* 先量深度：`depthAfter` 会先排空，而排空会**执行**已登记的关闭闭包 —— 那正好会把栈退回根页。
       顺序反过来的话，量到的是"退回根页之后"的 0，测的就不是这一条了。 */
    expect(depthAfter(<MobileSettingsScreen />), '设置屏二级页没登记可关闭层').toBe(1);
    const html = shell('settings', <MobileSettingsScreen />);
    expect(html, '设置屏二级页上画着底部导航').not.toContain('class="m-nav"');
    expect(html, '渲染出来的不是关于页').toContain('data-external=');
    expect(dismissTop()).toBe(true);
    expect(shell('settings', <MobileSettingsScreen />)).toContain('class="m-nav"');
  });
});
