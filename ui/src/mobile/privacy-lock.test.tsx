/**
 * 移动端**隐私锁**的门（W-16）—— 本批唯一带安全语义的一批，故判据也最密。
 *
 * # 被守的三件事，以及每件事**不守**会怎样
 *
 *  A. **闲置计时真的在跑**。不守：把 `armIdlePrivacyLock` 的 `setTimeout` 那一句删掉、或让
 *     `useMobileAppWiring` 不再调它 —— 开关照常拨得动、配置照常写进去，而**什么都不会锁**。
 *     那正是本批之前的状态（当时页面上还诚实写着一句缺席说明；接线之后再退回去就是无声的谎）。
 *  B. **解锁走 `privacy_unlock`**（IA §4.11 / 裁定 #10）。不守：把那两跳换成一句
 *     `setPrivacyMode(false)` —— 遮罩照常消失，界面看不出任何差别，而**密码从此不再被校验**
 *     （常量时间比对、300ms 弱限速、legacy 哈希升级全在后端那条命令里）。
 *     这是本文件最要紧的一组，故它有三条腿：函数体行为、生产那一跳、以及变异对照。
 *  C. **遮罩绕不过去**。不守：把它登记进 `back-stack`（一按返回就 dismiss = 返回键成了解锁键），
 *     或让它不再盖住底下的内容。
 *
 * # 判据面与它的**边界**（如实标注，不许把这道门说得比实际宽）
 *
 * 本仓 vitest 跑 `environment:'node'`、刻意不装 jsdom ⇒ `renderToStaticMarkup` **不跑 effect、
 * 也点不动按钮**。故：
 *  · 「函数体对不对」→ 喂假 IPC / 假计时器 / 假事件靶子**直接驱动**（最强的一档）；
 *  · 「生产真的在用它」→ AST：那个组件/hook 的体内真的有那次调用（`app-wiring.test.tsx` ⑪ 的同款
 *    手法）。两条**必须成对**：只测前者会被「函数是好的，只是没人调」骗过 —— 那正是本批之前
 *    隐私锁的实际状态（`useIdlePrivacyLock` 一直是好的，只是移动入口没人调）。
 *  · **抓不到**：真机上的手势/触点是不是真的重置了计时、遮罩在真机上是不是真的挡住了点击
 *    （静态 markup 判不了指针事件）、以及 Android 上系统截图/最近任务里的预览。
 *    那三格归真机验收，本门不假装覆盖。
 *
 * # 判据不能被自己污染
 *
 * 下面逐字写着 `setPrivacyMode` / `useDismissableLayer` 这些要断言其**不存在**的字面量。
 * 源码取材面显式排除测试文件，并由 ⓪ 组正面断言「本文件不在取材面里」。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createElement, type ReactElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import i18next, { type i18n as I18n } from 'i18next';
import { I18nextProvider, initReactI18next } from 'react-i18next';

import * as ts from '@/test/ts-compiler';
import zhCN from '@/i18n/locales/zh-CN.json';

/* ── 假后端 ──────────────────────────────────────────────────────────────────
 *
 * 🔴 只对**后端边界**打桩，被测的组件 / hook / 纯函数全部是真身。上两批复审抓到的三条 major
 * 都是「把要验的东西 mock 成桩」，这里不重犯：`submitPrivacyUnlock` 与 `MobileLockOverlay`
 * 都是真的那一个。
 */
const unlockMock = vi.fn(async (_password: string) => ({ ok: true }));
const setPrivacyModeMock = vi.fn(async (_value: boolean) => undefined);
const hasPasswordMock = vi.fn(async () => false);
const getPrivacyModeMock = vi.fn(async () => false);

vi.mock('@/ipc', () => ({
  api: {
    privacy: {
      unlock: (password: string) => unlockMock(password),
      hasPassword: () => hasPasswordMock(),
    },
    config: {
      setPrivacyMode: (value: boolean) => setPrivacyModeMock(value),
      getPrivacyMode: () => getPrivacyModeMock(),
    },
  },
}));

import {
  armIdlePrivacyLock,
  MOBILE_IDLE_ACTIVITY_EVENTS,
  readPrivacyHasPassword,
  resetColdStartPrivacyLock,
  shouldLockAfterColdStart,
  submitPrivacyUnlock,
  unlockOutcomeMessageKey,
  type IdleActivityTarget,
  type IdleTimers,
  type UnlockOutcome,
} from './privacy-lock';
import {
  applySecureScreen,
  hasSecureScreenBridge,
  SECURE_SCREEN_BRIDGE,
  SECURE_SCREEN_OFF,
  SECURE_SCREEN_ON,
} from './secure-screen-bridge';
import { maskRustComments } from '@/contracts/rust-source.test-support';
import { MobileLockOverlay } from './MobileLockOverlay';
import { dismissTop, pushDismissable, useDismissableDepth, useDismissableLayer } from './back-stack';
import { useAppStore } from '@/store/app-store';
import { IDLE_PRIVACY_LOCK_MS } from '@/domain/privacy';

const HERE = dirname(fileURLToPath(import.meta.url));
const UI_SRC = join(HERE, '..');

/* ── 取材面 ───────────────────────────────────────────────────────────────── */

/**
 * 剥注释。**这一步是判据的正确性，不是洁癖**：本批每个文件的头注里都在解释「为什么不碰
 * `setPrivacyMode`」「为什么不调 `useDismissableLayer`」—— 不剥就会把这些**反面例子**当成真实调用
 * 抓进来，于是判据对着自己的文档说明报红（`MobileSettings.test.tsx` 头注记过同一件事）。
 * **必须识别字符串**：本仓踩过一次「同时含 `*​/` 与 `/​*` 的字面量把正则版剥法带到文件末尾」。
 */
function stripComments(src: string): string {
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

/** `ui/src/mobile/settings/` 下的产品代码（**排除测试文件**，见头注「判据不能被自己污染」）。 */
function settingsSources(): { file: string; text: string }[] {
  const dir = join(HERE, 'settings');
  return readdirSync(dir)
    .filter((n) => /\.tsx?$/.test(n) && !/\.test\.tsx?$/.test(n))
    .sort()
    .map((n) => ({ file: n, text: stripComments(readFileSync(join(dir, n), 'utf8')) }));
}

const SETTINGS_SOURCES = settingsSources();

const parse = (relative: string): ts.SourceFile => {
  const abs = join(UI_SRC, relative);
  return ts.parseSourceFile(abs, readFileSync(abs, 'utf8'));
};

const synthetic = (name: string, text: string): ts.SourceFile => ts.parseSourceFile(name, text);

/* ── AST 小工具（与 `app-wiring.test.tsx` ⑪ 同一套，理由见那份头注）───────────── */

function collectNodes(root: ts.Node, pred: (n: ts.Node) => boolean): ts.Node[] {
  const out: ts.Node[] = [];
  const walk = (n: ts.Node): void => {
    if (pred(n)) out.push(n);
    ts.forEachChild(n, walk);
  };
  walk(root);
  return out;
}

function calleeNameOf(call: ts.CallExpression): string | null {
  const e = call.expression;
  if (ts.isIdentifier(e)) return e.text;
  if (ts.isPropertyAccessExpression(e) && ts.isIdentifier(e.name)) return e.name.text;
  return null;
}

const callsTo = (root: ts.Node, name: string): ts.CallExpression[] =>
  (collectNodes(root, ts.isCallExpression) as ts.CallExpression[]).filter(
    (c) => calleeNameOf(c) === name,
  );

/** 顶层函数体（`function f(){}` 与 `const f = (…) => …` 两形态）。 */
function bodyOf(sf: ts.SourceFile, name: string): ts.Node | null {
  for (const n of collectNodes(sf, ts.isFunctionDeclaration) as ts.FunctionDeclaration[]) {
    if (n.name?.text === name) return n;
  }
  for (const d of collectNodes(sf, ts.isVariableDeclaration) as ts.VariableDeclaration[]) {
    if (!ts.isIdentifier(d.name) || d.name.text !== name || !d.initializer) continue;
    if ((collectNodes(d.initializer, ts.isArrowFunction) as ts.Node[]).length > 0) return d.initializer;
  }
  return null;
}

/** `fn` 体内某个 `useEffect(…)` 的回调里调了 `callee`，且那个回调把拆卸闭包交回去了。 */
function effectInstalls(sf: ts.SourceFile, fn: string, callee: string): boolean {
  const body = bodyOf(sf, fn);
  if (body === null) return false;
  return callsTo(body, 'useEffect').some((effect) => {
    const callback = effect.arguments[0];
    if (callback === undefined) return false;
    if (callsTo(callback, callee).length === 0) return false;
    if (ts.isArrowFunction(callback) && !ts.isBlock(callback.body)) return true;
    return collectNodes(callback, ts.isReturnStatement).some(
      (r) => (r as ts.ReturnStatement).expression !== undefined,
    );
  });
}

/**
 * 取 JSX 元素（按某个 `data-*` 标记定位）上某个属性的**表达式文本**。
 *
 * 「组件体里调过那个函数」挡不住「按钮的 `onClick` 被换成 `() => undefined`」——
 * 那个函数还在文件里、还被定义着，只是**没人点得到它**（协调者实测：换掉 `onClick` 后本文件
 * 26/26 全绿）。故这里判的是那一跳属性本身。
 */
function jsxAttrText(sf: ts.SourceFile, marker: string, attribute: string): string | null {
  const elements = [
    ...(collectNodes(sf, ts.isJsxSelfClosingElement) as ts.Node[]),
    ...(collectNodes(sf, ts.isJsxOpeningElement) as ts.Node[]),
  ] as (ts.JsxSelfClosingElement | ts.JsxOpeningElement)[];
  for (const el of elements) {
    const props = el.attributes.properties.filter(ts.isJsxAttribute);
    if (!props.some((a) => a.name.getText(sf) === marker)) continue;
    const hit = props.find((a) => a.name.getText(sf) === attribute);
    if (hit === undefined) return null;
    const init = hit.initializer;
    if (init === undefined) return '';
    return ts.isJsxExpression(init) ? (init.expression?.getText(sf) ?? '') : init.getText(sf);
  }
  return null;
}

/** `fn` 体内（不限 effect）调了 `callee`。 */
function bodyCalls(sf: ts.SourceFile, fn: string, callee: string): boolean {
  const body = bodyOf(sf, fn);
  return body !== null && callsTo(body, callee).length > 0;
}

/** `fn` 体内某个 `useEffect(…)` 的回调里调了 `callee`（不要求交回拆卸闭包）。 */
function effectCallsIn(sf: ts.SourceFile, fn: string, callee: string): boolean {
  const body = bodyOf(sf, fn);
  if (body === null) return false;
  return callsTo(body, 'useEffect').some((effect) => {
    const callback = effect.arguments[0];
    return callback !== undefined && callsTo(callback, callee).length > 0;
  });
}

/**
 * `fn` 体内那条调了 `callee` 的 `useEffect` 的**依赖数组文本**（`null` = 没有那条 effect，
 * `''` = 那条 effect 压根没写依赖数组 —— 那是「每次渲染都重跑」，与写空数组是两码事）。
 *
 * 🔴 **为什么单独有这把尺子**（2026-09-06 复审 major）：上一版的判据只看 effect 的**回调体**
 * （正则与 `effectInstalls` 都是），依赖数组那一格没有任何门读它。实测把
 * `}, [autoPrivacyMode, privacyMode]);` 改成 `}, []);` ⇒ 全量 4300 条全绿、`tsc` 也绿，
 * 而生产上 `useEffectiveConfig` 在挂载那一帧恒返 false ⇒ effect 早退且永不重跑 ⇒
 * 用户拨开开关之后**闲置计时一次都不武装**，正是 W-16 要修的缺口原样回来。
 */
function effectDepsOf(sf: ts.SourceFile, fn: string, callee: string): string | null {
  const body = bodyOf(sf, fn);
  if (body === null) return null;
  for (const effect of callsTo(body, 'useEffect')) {
    const callback = effect.arguments[0];
    if (callback === undefined || callsTo(callback, callee).length === 0) continue;
    const deps = effect.arguments[1];
    return deps === undefined ? '' : deps.getText(sf).replace(/\s+/g, ' ');
  }
  return null;
}

/**
 * 从 `sf` 里找到标签名为 `tag` 的 JSX 节点，返回它**祖先链**上的全部 JSX 标签名。
 *
 * 🔴 用它替掉上一版那条「自闭合表里只有遮罩 ⇒ 它不在外壳里」的推论（2026-09-06 复审 major）：
 * 那条推论不成立 —— `MobileShell` 是带 children 的开标签，无论遮罩在它内还是外都不会出现在
 * 自闭合表里，故那条断言对「把遮罩塞进外壳」这件事**零分辨率**（实测：塞进去之后全量全绿）。
 * 而那个改动在真机上是真缺陷：外壳的滚动区 `.m-scroll` 带 `container-type: inline-size`
 * （`mobile.css`），按 CSS Containment 规范该元素成为 fixed 后代的**包含块** ⇒ 遮罩的
 * `position:fixed; inset:0` 从此只铺满滚动区，停靠区（待应用条 + 底部导航）露在遮罩之外且可点。
 */
function jsxAncestorTags(sf: ts.SourceFile, tag: string): string[] | null {
  /* `JsxElement`（带 children 的那种）的标签名在它的 `openingElement` 上，而 children 是
     `JsxElement` 的兄弟节点、不是 `openingElement` 的 —— 故标签名必须记在 `JsxElement` 这一层，
     否则「谁是谁的祖先」整个算错。 */
  const nameOf = (n: ts.Node): string | null => {
    if (ts.isJsxElement(n)) return (n as ts.JsxElement).openingElement.tagName.getText(sf);
    if (ts.isJsxSelfClosingElement(n)) return (n as ts.JsxSelfClosingElement).tagName.getText(sf);
    return null;
  };
  // 显式栈（不用闭包累加）：`let found` 被闭包改写时 TS 的窄化会把它认成恒 `null`。
  const stack: { node: ts.Node; trail: string[] }[] = [{ node: sf, trail: [] }];
  while (stack.length > 0) {
    const { node, trail } = stack.pop() as { node: ts.Node; trail: string[] };
    const name = nameOf(node);
    if (name === tag) return trail;
    const next = name === null ? trail : [...trail, name];
    ts.forEachChild(node, (child) => {
      stack.push({ node: child, trail: next });
    });
  }
  return null;
}

/* ── 假事件靶子 / 假计时器 ───────────────────────────────────────────────────── */

interface FakeTarget extends IdleActivityTarget {
  fire: (type: string) => void;
  listening: () => string[];
  /** 注册时带了 `capture:true` 的事件名（`scroll` 那一条的成败全在这里）。 */
  captured: () => string[];
  /** 拆卸时**没有**按 capture 对上号的事件名（DOM 按 `(type, cb, capture)` 三元组匹配）。 */
  leaked: () => string[];
}

function fakeTarget(): FakeTarget {
  const listeners = new Map<string, Set<() => void>>();
  const captureOf = new Map<string, boolean>();
  const leaked = new Set<string>();
  return {
    addEventListener(type, listener, options) {
      const set = listeners.get(type) ?? new Set();
      listeners.set(type, set);
      set.add(listener);
      captureOf.set(type, options?.capture === true);
    },
    removeEventListener(type, listener, options) {
      /* 逐字模拟 DOM 的匹配规则：capture 不同 = 另一条监听，删不掉。
         少了这一步，「注册带 capture、拆卸不带」这个真实缺陷在假靶子上会被当成删成功。 */
      if ((options?.capture === true) !== (captureOf.get(type) === true)) {
        leaked.add(type);
        return;
      }
      listeners.get(type)?.delete(listener);
    },
    fire(type) {
      for (const l of [...(listeners.get(type) ?? [])]) l();
    },
    listening: () => [...listeners].filter(([, s]) => s.size > 0).map(([t]) => t).sort(),
    captured: () => [...captureOf].filter(([, on]) => on).map(([t]) => t).sort(),
    leaked: () => [...leaked].sort(),
  };
}

interface FakeTimers extends IdleTimers {
  /** 让最后一次武装的计时立刻到点。没有在途计时返 `false`。 */
  fire: () => boolean;
  /** 每次武装记一条（用于断言「重新武装了」而不只是「还有计时”）。 */
  arms: number[];
}

function fakeTimers(): FakeTimers {
  let pending: (() => void) | null = null;
  const arms: number[] = [];
  return {
    arms,
    setTimeout(handler, ms) {
      pending = handler;
      arms.push(ms);
      return arms.length;
    },
    clearTimeout() {
      pending = null;
    },
    fire() {
      const run = pending;
      pending = null;
      if (run === null) return false;
      run();
      return true;
    },
  };
}

/* ── 渲染 ────────────────────────────────────────────────────────────────── */

let i18n: I18n;

beforeEach(async () => {
  unlockMock.mockClear();
  setPrivacyModeMock.mockClear();
  hasPasswordMock.mockClear();
  getPrivacyModeMock.mockClear();
  unlockMock.mockImplementation(async () => ({ ok: true }));
  hasPasswordMock.mockImplementation(async () => false);
  getPrivacyModeMock.mockImplementation(async () => false);
  resetColdStartPrivacyLock();
  useAppStore.getState().setPrivacyMode(false);
  // 排空可关闭层登记表：同一个 `it` 里前面那趟渲染留下的层会一起算进深度（同 `back-navigation` 的 `drain`）。
  let guard = 0;
  while (dismissTop()) {
    guard += 1;
    if (guard > 64) throw new Error('`dismissTop` 排不空登记表 —— 它没有在出栈');
  }
  if (i18n === undefined) {
    i18n = i18next.createInstance();
    await i18n.use(initReactI18next).init({
      lng: 'zh-CN',
      fallbackLng: 'zh-CN',
      resources: { 'zh-CN': { translation: zhCN } },
      interpolation: { escapeValue: false },
    });
  }
});

function render(node: ReactElement): string {
  return renderToStaticMarkup(createElement(I18nextProvider, { i18n }, node));
}

/** 渲染期读栈深 —— 它自己就是「登记发生在渲染期」的证明（同 `back-navigation.test.tsx`）。 */
function DepthProbe(): ReactElement {
  return createElement('i', { 'data-depth': useDismissableDepth() });
}

/** 一层**会**登记的假层，用作正向对照：证明探针分得清「登记了」与「没登记」。 */
function RegisteringLayer(): ReactElement {
  useDismissableLayer(true, () => undefined);
  return createElement('i', { 'data-fake-layer': 'yes' });
}

function depthAfter(node: ReactElement): number {
  const html = renderToStaticMarkup(
    createElement(
      I18nextProvider,
      { i18n },
      createElement('div', null, node, createElement(DepthProbe, null)),
    ),
  );
  const m = /data-depth="(\d+)"/.exec(html);
  if (m === null) throw new Error('深度探针没渲染出来 —— 判据面塌了');
  return Number(m[1]);
}

// ═══════════════════════════════════════════════════════════════════════════
describe('⓪ 自检：三块判据面都是活的（否则下面每条都恒绿）', () => {
  it('源码取材面非空，且**不含本判据文件自己**', () => {
    expect(SETTINGS_SOURCES.length, '设置目录一个源码文件都没扫到').toBeGreaterThanOrEqual(10);
    expect(SETTINGS_SOURCES.map((s) => s.file)).not.toContain('privacy-lock.test.tsx');
    expect(SETTINGS_SOURCES.map((s) => s.file)).toContain('GeneralPage.tsx');
  });

  it('AST 谓词正反都判得对（否则下面那些「真的调了」是空话）', () => {
    const wired = 'function useX() {\n  useEffect(() => armIdlePrivacyLock(window, lock), []);\n}\n';
    const gutted = 'function useX() {\n  useEffect(() => {\n    void 0;\n  }, []);\n}\n';
    expect(effectInstalls(synthetic('syn-wired.ts', wired), 'useX', 'armIdlePrivacyLock')).toBe(true);
    expect(effectInstalls(synthetic('syn-gutted.ts', gutted), 'useX', 'armIdlePrivacyLock')).toBe(false);
    // 函数整个改名/删掉 ⇒ 判否，不是「找不到就跳过」。
    expect(effectInstalls(synthetic('syn-wired2.ts', wired), 'useGone', 'armIdlePrivacyLock')).toBe(false);
    expect(bodyCalls(synthetic('syn-body.ts', 'function f() { g(1); }\n'), 'f', 'g')).toBe(true);
    expect(bodyCalls(synthetic('syn-body2.ts', 'function f() { h(1); }\n'), 'f', 'g')).toBe(false);
  });

  it('依赖数组尺子分得清「依赖齐」「空数组」「压根没写」（否则那三条依赖断言是空话）', () => {
    const withDeps = 'function useX() {\n  useEffect(() => {\n    arm(window);\n  }, [a, b]);\n}\n';
    const emptyDeps = 'function useX() {\n  useEffect(() => {\n    arm(window);\n  }, []);\n}\n';
    const noDeps = 'function useX() {\n  useEffect(() => {\n    arm(window);\n  });\n}\n';
    expect(effectDepsOf(synthetic('syn-deps1.ts', withDeps), 'useX', 'arm')).toBe('[a, b]');
    expect(effectDepsOf(synthetic('syn-deps2.ts', emptyDeps), 'useX', 'arm')).toBe('[]');
    expect(
      effectDepsOf(synthetic('syn-deps3.ts', noDeps), 'useX', 'arm'),
      '「压根没写依赖数组」被读成了空数组 —— 两者的行为相反',
    ).toBe('');
    expect(effectDepsOf(synthetic('syn-deps4.ts', withDeps), 'useX', 'other')).toBeNull();
    // `effectCallsIn`：不要求交回拆卸闭包的那一版（冷启动补锁那条 effect 就没有拆卸）。
    expect(effectCallsIn(synthetic('syn-eff1.ts', emptyDeps), 'useX', 'arm')).toBe(true);
    expect(effectCallsIn(synthetic('syn-eff2.ts', emptyDeps), 'useX', 'nope')).toBe(false);
  });

  it('祖先链尺子分得清「兄弟」与「塞进外壳里」（C 组那条包含关系断言的正反对照）', () => {
    const sibling =
      'const A = () => (\n  <>\n    <MobileShell><Screen /></MobileShell>\n    <MobileLockOverlay locked={l} />\n  </>\n);\n';
    const nested =
      'const B = () => (\n  <>\n    <MobileShell><Screen /><MobileLockOverlay locked={l} /></MobileShell>\n  </>\n);\n';
    expect(jsxAncestorTags(synthetic('syn-sib.tsx', sibling), 'MobileLockOverlay')).not.toContain(
      'MobileShell',
    );
    expect(
      jsxAncestorTags(synthetic('syn-nest.tsx', nested), 'MobileLockOverlay'),
      '遮罩被塞进外壳 children 里却没被认出来 —— C 组那条包含关系断言恒绿',
    ).toContain('MobileShell');
    expect(jsxAncestorTags(synthetic('syn-none.tsx', sibling), 'NotThere')).toBeNull();
  });

  it('剥注释器把注释里的反面例子挡在外面，且不吃掉字符串', () => {
    expect(stripComments('/* 不碰 setPrivacyMode */\nconst a = 1;\n')).not.toContain('setPrivacyMode');
    expect(stripComments('// 不调 useDismissableLayer\nconst b = 2;\n')).not.toContain(
      'useDismissableLayer',
    );
    // 反向对照：真的写在代码里就必须留下来（否则这个剥法会让下面的否定断言恒绿）。
    expect(stripComments("const c = 'setPrivacyMode';\n")).toContain('setPrivacyMode');
  });

  it('假计时器 / 假靶子自检（假的坏了会让 A 组全绿）', () => {
    const timers = fakeTimers();
    expect(timers.fire(), '没武装过就说「到点了」').toBe(false);
    let hit = 0;
    timers.setTimeout(() => {
      hit += 1;
    }, 5);
    expect(timers.fire()).toBe(true);
    expect(hit).toBe(1);
    const target = fakeTarget();
    let fired = 0;
    target.addEventListener('touchstart', () => {
      fired += 1;
    });
    target.fire('touchstart');
    target.fire('keydown'); // 没人听 ⇒ 不该触发
    expect(fired).toBe(1);
  });

  it('JSX 属性谓词正反都判得对（否则 B 组那条「点得到」是空话）', () => {
    const sf = synthetic(
      'syn-jsx.tsx',
      'const C = () => <button data-x="1" onClick={go} />;\nconst D = () => <i data-y="1" />;\n',
    );
    expect(jsxAttrText(sf, 'data-x', 'onClick')).toBe('go');
    expect(jsxAttrText(sf, 'data-x', 'onKeyDown'), '属性不在时该返 null，不是空串').toBeNull();
    expect(jsxAttrText(sf, 'data-nope', 'onClick'), '找不到那个元素时该返 null').toBeNull();
  });

  it('深度探针分得清「登记了」与「没登记」（C 组那条否定断言的正向对照）', () => {
    expect(depthAfter(createElement('i', null)), '一个什么都不登记的节点却读出了层').toBe(0);
    expect(
      depthAfter(createElement(RegisteringLayer, null)),
      '一层**真的**登记了的层却读出 0 —— 探针是瞎的，C 组那条否定断言恒绿',
    ).toBe(1);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('A 闲置计时：真的在计时，且移动入口真的挂了它', () => {
  it('挂载即武装；到点调 lock（阈值取共享常量，不是这里另写一个数）', () => {
    const target = fakeTarget();
    const timers = fakeTimers();
    let locks = 0;
    armIdlePrivacyLock(target, () => { locks += 1; }, IDLE_PRIVACY_LOCK_MS, timers);
    expect(timers.arms, '挂载没有立刻起计时 —— 用户一进来就把手机放下的那一档不会锁').toEqual([
      IDLE_PRIVACY_LOCK_MS,
    ]);
    expect(timers.fire()).toBe(true);
    expect(locks, '计时到点却没有去锁').toBe(1);
  });

  it('每一种用户活动都重置计时（逐个事件对拍，不是「有监听就算」）', () => {
    for (const event of MOBILE_IDLE_ACTIVITY_EVENTS) {
      const target = fakeTarget();
      const timers = fakeTimers();
      armIdlePrivacyLock(target, () => undefined, 1000, timers);
      target.fire(event);
      expect(timers.arms.length, `\`${event}\` 没有重置闲置计时`).toBe(2);
    }
  });

  /*
   * 🔴 **`scroll` 必须挂在捕获相**（2026-09-06 复审 minor，已修）。
   *
   * 本仓滚动的是内层 `div.m-scroll{overflow-y:auto}`，不是 document；而 `scroll` 在 Element 上
   * 派发时**不冒泡**（只有 Document 那一发会投到 `window`）。此前这个监听挂在 `window` 的冒泡相，
   * 于是「用户在应用里滚了半天」这条活动信号**一次都收不到** —— 五个事件里实际只有四个是活的。
   * 上面那条「每一种用户活动都重置计时」对着假靶子直接 `fire('scroll')`，故它照单全收一个
   * 恒不触发的事件名：那正是「测了函数本体、没测生产那一跳」的同族形态，这里补上相位这一格。
   */
  it('🔴 五条监听全部注册在**捕获相**（`scroll` 在元素上不冒泡，冒泡相恒收不到）', () => {
    const target = fakeTarget();
    armIdlePrivacyLock(target, () => undefined, 1000, fakeTimers());
    expect(
      target.captured(),
      '有监听没走捕获相 —— `scroll` 那一条会恒不触发（滚动容器是 `.m-scroll`，事件不冒泡到 window）',
    ).toEqual([...MOBILE_IDLE_ACTIVITY_EVENTS].sort());
  });

  it('拆卸后：不再计时、也不再监听（否则切开关会叠出好几条计时）', () => {
    const target = fakeTarget();
    const timers = fakeTimers();
    let locks = 0;
    const off = armIdlePrivacyLock(target, () => { locks += 1; }, 1000, timers);
    expect(target.listening()).toEqual([...MOBILE_IDLE_ACTIVITY_EVENTS].sort());
    off();
    // 拆卸的 capture 必须与注册相等，否则 DOM 认不出那条监听、静默删不掉。
    expect(
      target.leaked(),
      '拆卸时的 capture 与注册不一致 —— DOM 按 `(type, callback, capture)` 三元组匹配，删不掉',
    ).toEqual([]);
    expect(target.listening(), '拆卸之后监听还在').toEqual([]);
    expect(timers.fire(), '拆卸之后在途计时还在，它到点会锁一次').toBe(false);
    expect(locks).toBe(0);
  });

  it('🔴 `useMobileAppWiring` 的 effect 里真的调了 `armIdlePrivacyLock`，且交回了拆卸闭包', () => {
    expect(
      effectInstalls(parse('mobile/app-wiring.ts'), 'useMobileAppWiring', 'armIdlePrivacyLock'),
      '移动入口不再挂闲置计时 —— 设置页那个开关拨得动、配置写得进去，而**什么都不会锁**。' +
        '上面 A 组前三条测的是「计时器本身是好的」，它们一条都不会红（本批之前 `useIdlePrivacyLock` ' +
        '就一直是好的，只是移动端没人调）',
    ).toBe(true);
  });

  /*
   * 🔴 **整条闲置腿按「整次调用」对拍**，不是拆成三条互不相干的「出现过某个词」。
   *
   * 三个变异各自都能让锁**从此不再发生**，而上面那些行为断言与 `bodyCalls` 一条都不红
   * （协调者逐条实测）：
   *  · 武装条件的 `!` 被去掉 ⇒ 只在**已锁**时武装 ⇒ 从来不锁；
   *  · `armIdlePrivacyLock` 的 `lock` 回调被掏空（`() => Promise.resolve()`）⇒ 计时到点什么都不做；
   *  · 靶子从 `window` 换成别的 ⇒ 用户活动重置不了计时（或反过来永远重置）。
   * 三条都住在同一段 effect 里，故按那一段的整体形态对拍。同 `back-navigation.test.tsx`
   * 那条 `handleBackPress({…})` 的做法。
   */
  it('🔴 武装条件走共享的 `shouldArmIdleLock`，且**取反**（已锁不再武装）', () => {
    const src = stripComments(readFileSync(join(UI_SRC, 'mobile/app-wiring.ts'), 'utf8'));
    expect(bodyCalls(parse('mobile/app-wiring.ts'), 'useMobileAppWiring', 'shouldArmIdleLock')).toBe(true);
    expect(
      /if\s*\(\s*!shouldArmIdleLock\(\s*autoPrivacyMode\s*,\s*privacyMode\s*\)\s*\)\s*return undefined;/.test(
        src,
      ),
      '武装条件不再是「开了自动隐私锁 且 当前未锁」—— 去掉那个 `!` 就变成「只在已锁时武装」，' +
        '于是从来不锁，而 A 组其余每一条照样全绿',
    ).toBe(true);
  });

  it('🔴 计时靶子是 `window`，到点真的去调 `config_set_privacy_mode(true)`', () => {
    const src = stripComments(readFileSync(join(UI_SRC, 'mobile/app-wiring.ts'), 'utf8'));
    expect(
      /return armIdlePrivacyLock\(\s*window\s*,/.test(src),
      '闲置计时挂错了靶子 —— 用户的触摸/滚动重置不了它（或者它压根收不到任何活动）',
    ).toBe(true);
    expect(
      /reportIfFails\(\s*\(\)\s*=>\s*api\.config\.setPrivacyMode\(true\)\s*,/.test(src),
      '计时到点了却不去进隐私态 —— 锁腿被掏空，而「effect 里调了 armIdlePrivacyLock」照样成立',
    ).toBe(true);
    /*
     * 🔴 **依赖数组也在射程内**（2026-09-06 复审 major）。上面三条与 `effectInstalls` 全都只看
     * 回调体。实测把 `}, [autoPrivacyMode, privacyMode]);` 改成 `}, []);` ⇒ 全量 4300 条全绿：
     * effect 只在挂载那一帧跑一次，而那时 `useEffectiveConfig` 恒返 false ⇒ 早退且永不重跑，
     * 用户拨开开关之后闲置计时**一次都不武装**。
     */
    expect(
      effectDepsOf(parse('mobile/app-wiring.ts'), 'useMobileAppWiring', 'armIdlePrivacyLock'),
      '闲置计时那条 effect 的依赖不再跟着开关与锁定态走 —— 依赖空了它就只在挂载那一帧跑一次，' +
        '而那一帧 config 还没载入、武装条件必假 ⇒ 从此不再武装',
    ).toBe('[autoPrivacyMode, privacyMode]');
    // 失败要看得见（裁定 #14）：这一层没有可归因的控件，故落全局 toast。
    expect(
      /mobileSettings\.general\.privacyLockFailed/.test(src),
      '锁失败静默了 —— 用户以为把手机递出去界面会锁',
    ).toBe(true);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * B 组 —— 本文件最要紧的一组。三条腿缺一条都留着一条绕行路：
 *  ① 函数体：解锁真的先打 `privacy_unlock`，`ok:false` 绝不落到翻转那一步；
 *  ② 生产那一跳：遮罩组件真的调的是这个函数（不是自己另写一段）；
 *  ③ 变异对照：把 ① 换成就地翻标志位，① 必须红（见那条 `it` 里的说明）。
 */
describe('B 解锁链：必须经过 `privacy_unlock`，不许就地翻标志位（裁定 #10）', () => {
  const outcomes: UnlockOutcome[] = [];
  const record = (o: UnlockOutcome): void => {
    outcomes.push(o);
  };

  beforeEach(() => {
    outcomes.length = 0;
  });

  it('🔴 成功路径：先 `privacy_unlock(密码)`，**校验通过之后**才 `setPrivacyMode(false)`', async () => {
    await submitPrivacyUnlock(true, 'hunter2', record);
    expect(unlockMock, '解锁没有经过 `privacy_unlock` —— 密码从此不再被校验').toHaveBeenCalledWith(
      'hunter2',
    );
    expect(setPrivacyModeMock).toHaveBeenCalledWith(false);
    expect(outcomes).toEqual(['unlocked']);
    // 顺序：校验在前。两次调用的 invocation 序号对拍，不是「两个都调过」。
    expect(
      unlockMock.mock.invocationCallOrder[0] < setPrivacyModeMock.mock.invocationCallOrder[0],
      '先翻转、后校验 —— 那等于不校验',
    ).toBe(true);
  });

  it('🔴 密码错：**一次都不许**碰隐私态（这是「变异必须红」的那条判据本体）', async () => {
    unlockMock.mockImplementation(async () => ({ ok: false }));
    await submitPrivacyUnlock(true, 'wrong', record);
    expect(unlockMock).toHaveBeenCalledWith('wrong');
    expect(
      setPrivacyModeMock,
      '密码错却退出了隐私态 —— 这正是「把校验换成就地 `setPrivacyMode(false)`」那个变异的表现',
    ).not.toHaveBeenCalled();
    expect(outcomes).toEqual(['rejected']);
  });

  it('设了密码却空输入 ⇒ 本地早退，**不打后端**（同桌面 `resolveUnlockAttempt`）', async () => {
    await submitPrivacyUnlock(true, '', record);
    expect(unlockMock, '空输入也去打了一次后端 —— 白白吃一次 300ms 弱限速').not.toHaveBeenCalled();
    expect(setPrivacyModeMock).not.toHaveBeenCalled();
    expect(outcomes).toEqual(['require-input']);
  });

  it('未设密码 + 空输入 ⇒ 照样走后端（自由解锁由 `unlock_core` 判，不在前端预判）', async () => {
    await submitPrivacyUnlock(false, '', record);
    expect(unlockMock).toHaveBeenCalledWith('');
    expect(outcomes).toEqual(['unlocked']);
  });

  it('调用本身失败 ⇒ `failed`，且**不**折成「密码错」（否则用户会一遍遍重打正确的密码）', async () => {
    unlockMock.mockImplementation(async () => {
      throw new Error('ipc down');
    });
    await submitPrivacyUnlock(true, 'hunter2', record);
    expect(outcomes).toEqual(['failed']);
    expect(setPrivacyModeMock).not.toHaveBeenCalled();
    expect(unlockOutcomeMessageKey('failed')).not.toBe(unlockOutcomeMessageKey('rejected'));
  });

  it('四支各有各的提示键，且 `unlocked` 不说话（枚举全覆盖，少一支会编不过）', () => {
    const keys = (['require-input', 'rejected', 'failed', 'unlocked'] as const).map(
      unlockOutcomeMessageKey,
    );
    expect(new Set(keys).size, '有两支共用了同一句话').toBe(4);
    expect(unlockOutcomeMessageKey('unlocked')).toBeNull();
  });

  it('读密码状态失败 ⇒ 按「已设密码」处理（反过来会放行一次空密码解锁）', async () => {
    hasPasswordMock.mockImplementation(async () => {
      throw new Error('nope');
    });
    expect(await readPrivacyHasPassword()).toBe(true);
    hasPasswordMock.mockImplementation(async () => false);
    expect(await readPrivacyHasPassword(), '正向对照：读得到时如实返回').toBe(false);
  });

  /*
   * 🔴 **点得到**（复审同族形态的第二跳）。
   *
   * 下一条只证明「组件体里调过那条解锁链」。把提交按钮的 `onClick` 换成 `() => undefined`
   * （解锁腿整条点不到、组件与函数都原样在场）时，本文件其余每一条**全绿**（协调者实测）。
   * 故这里逐一钉住两个入口：按钮的 `onClick` 与输入框的 `onKeyDown`（回车提交）。
   */
  it('🔴 遮罩上那两个提交入口真的接在同一个提交闭包上（不是惰性的 `() => undefined`）', () => {
    const sf = parse('mobile/MobileLockOverlay.tsx');
    expect(
      jsxAttrText(sf, 'data-lock-submit', 'onClick'),
      '「解锁」按钮点了什么都不做 —— 密码框成了摆设',
    ).toBe('submit');
    const onKeyDown = jsxAttrText(sf, 'data-lock-input', 'onKeyDown') ?? '';
    expect(onKeyDown, '密码框里按回车提交不了（触屏软键盘上「完成」是主路径）').toContain('submit(');
    expect(onKeyDown, '回车没判键名 —— 每敲一个字都会提交一次').toContain("'Enter'");
  });

  it('🔴 生产那一跳：遮罩组件真的调 `submitPrivacyUnlock`，且不自己碰隐私态', () => {
    const sf = parse('mobile/MobileLockOverlay.tsx');
    expect(
      bodyCalls(sf, 'MobileLockOverlay', 'submitPrivacyUnlock'),
      '遮罩不再走那条解锁链 —— 上面每一条行为断言照样全绿（它们测的是那个函数，不是生产在用它）',
    ).toBe(true);
    const text = stripComments(readFileSync(join(UI_SRC, 'mobile/MobileLockOverlay.tsx'), 'utf8'));
    expect(
      /setPrivacyMode/.test(text),
      '遮罩自己翻了隐私态 —— 那会绕过 `privacy_unlock` 整条校验链',
    ).toBe(false);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('C 遮罩：绕不过去，也盖得住', () => {
  const lockedMarkup = (): string => render(createElement(MobileLockOverlay, { locked: true }));

  it('自检：锁定时它真的渲染出了东西，未锁时它什么都不渲染', () => {
    expect(lockedMarkup(), '锁定态下遮罩没渲染 —— 下面每条都在对空气断言').toContain(
      'data-overlay="privacy-lock"',
    );
    expect(
      render(createElement(MobileLockOverlay, { locked: false })),
      '没锁也画遮罩 —— DOM 里留一个能拿焦点的密码框是另一回事',
    ).toBe('');
  });

  it('🔴 **不登记**进 `back-stack`（登记了按一下返回就解锁 —— 返回键成了解锁键）', () => {
    expect(
      depthAfter(createElement(MobileLockOverlay, { locked: true })),
      '遮罩把自己登记成了可关闭层 —— 系统返回键会直接把它 dismiss 掉，密码形同虚设',
    ).toBe(0);
    // 正向对照在 ⓪ 组（一层真的登记了的层读出 1）：证明这条 0 不是探针瞎了。
    expect(
      stripComments(readFileSync(join(UI_SRC, 'mobile/MobileLockOverlay.tsx'), 'utf8')),
    ).not.toContain('useDismissableLayer');
  });

  it('🔴 底下那层弹层不受影响：遮罩渲染前后，栈深与栈顶闭包都不动', () => {
    const hit: string[] = [];
    pushDismissable('sheet', () => void hit.push('closed'));
    render(createElement(MobileLockOverlay, { locked: true }));
    expect(dismissTop(), '遮罩把底下那层弹掉了').toBe(true);
    expect(hit, '弹出来的不是底下那层自己的关闭闭包').toEqual(['closed']);
  });

  it('🔴 盖在所有内容之上：fixed + 四边贴边 + 不透明实底 + 有层号', () => {
    const html = lockedMarkup();
    const style = /data-overlay="privacy-lock"[^>]*style="([^"]*)"/.exec(html);
    expect(style, '遮罩没有内联样式 —— 它铺不满任何东西').not.toBeNull();
    const css = style?.[1] ?? '';
    expect(css, 'position 不是 fixed：会跟着内容一起滚走').toContain('position:fixed');
    for (const side of ['top:0', 'right:0', 'bottom:0', 'left:0']) {
      expect(css, `没有贴住 ${side} —— 那一边会露出底下的内容`).toContain(side);
    }
    expect(css, '没有层号：与停靠区同层时谁在上取决于文档顺序').toMatch(/z-index:\d+/);
    expect(
      css,
      '底色不是实底 —— 半透明/blur 在这里是安全问题不是观感问题：底下那一屏正是要挡住的东西',
    ).toContain('background:hsl(var(--bg))');
    expect(css, '背景写成了 transparent/none').not.toMatch(/background:(transparent|none)/);
  });

  it('🔴 遮罩是**外壳的兄弟**而不是子节点（按祖先链判包含关系，不按标签表推）', () => {
    const sf = parse('mobile/MobileApp.tsx');
    const ancestors = jsxAncestorTags(sf, 'MobileLockOverlay');
    expect(ancestors, '`MobileApp` 里没有挂遮罩 —— 锁上之后手机上什么都不会发生').not.toBeNull();
    /*
     * 🔴 判的是**祖先链**（2026-09-06 复审 major：上一版判的是「自闭合标签表恰等于
     * `['MobileLockOverlay']`」，而 `MobileShell` 是带 children 的开标签、无论遮罩在它内还是外
     * 都不在那张表里 ⇒ 那条断言对「塞进外壳」零分辨率，实测塞进去之后全量全绿）。
     * 塞进去在真机上是真缺陷：外壳滚动区 `.m-scroll` 的 `container-type: inline-size`
     * （`mobile.css`）施加 layout containment ⇒ 该元素成为 fixed 后代的**包含块**，
     * 遮罩的 `inset:0` 从此只铺满滚动区，停靠区（待应用条 + 底部导航）露在外面且可点：
     * 锁屏期间用户仍能在五个目的地之间切换。
     */
    expect(
      ancestors as string[],
      '遮罩被塞进了外壳内部 —— `.m-scroll` 的 containment 会把它的 fixed 定位关进滚动区，' +
        '停靠区露在遮罩之外且可点',
    ).not.toContain('MobileShell');
  });

  /*
   * 🔴 **把参数喂进去的那一跳**（上两批复审点名的同族形态）。
   *
   * C 组前几条都是拿 `locked: true` 直接驱动组件的行为断言 —— 它们证明「遮罩这个组件是好的」，
   * 一条都拦不住「容器恒传 `false`」：那样锁上之后手机上什么都不会发生，而本文件其余判据全绿。
   * 故这里判的是那个 JSX 属性的**表达式本身**：必须是从 store 读来的那个标识符，
   * 不是 `false` / `true` / 任何字面量。
   */
  it('🔴 `MobileApp` 把**真的**锁定态喂了进去（不是恒传一个字面量）', () => {
    const sf = parse('mobile/MobileApp.tsx');
    const el = (
      collectNodes(sf, (n) => ts.isJsxSelfClosingElement(n)) as ts.JsxSelfClosingElement[]
    ).find((n) => n.tagName.getText(sf) === 'MobileLockOverlay');
    expect(el, '`MobileApp` 里找不到那个遮罩节点 —— 判据面塌了').toBeDefined();
    const attr = el?.attributes.properties.find(
      (a) => ts.isJsxAttribute(a) && a.name.getText(sf) === 'locked',
    ) as ts.JsxAttribute | undefined;
    expect(attr, '遮罩没有拿到 `locked` —— 它永远画不出来（TS 会拦，但拦不住下面那两种）').toBeDefined();
    const expr = attr?.initializer;
    expect(expr && ts.isJsxExpression(expr), '`locked` 被写成了字符串属性').toBe(true);
    const inner = (expr as ts.JsxExpression).expression;
    expect(
      inner !== undefined && ts.isIdentifier(inner) && inner.text === 'locked',
      '喂进去的不是那个从 store 读来的标识符 —— 恒传字面量时锁上之后什么都不会发生，' +
        '而 C 组其余每一条照样全绿',
    ).toBe(true);
    // 正面：那个标识符确实是从 store 读来的（否则它可能只是一个恒 false 的局部常量）。
    expect(
      /const\s+locked\s*=\s*useAppStore\(/.test(stripComments(readFileSync(join(UI_SRC, 'mobile/MobileApp.tsx'), 'utf8'))),
      '`locked` 不是从 app-store 的 `privacyMode` 读来的',
    ).toBe(true);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('D 设置页：密码可设，但隐私态的翻转**不在**这一屏', () => {
  const text = SETTINGS_SOURCES.map((s) => s.text).join('\n');

  it('🔴 设置目录里一处 `setPrivacyMode` 都没有（翻转的唯一收敛点在遮罩与闲置计时那侧）', () => {
    expect(
      text.includes('setPrivacyMode'),
      '设置页碰了隐私态的翻转 —— 那是一条绕过 `privacy_unlock` 的新路',
    ).toBe(false);
    // 正向对照：这份取材面确实装着本屏的写腿（否则上面那条否定是对空气说的）。
    expect(text).toContain('privacyApi.setPassword');
  });

  it('🔴 密码设置真的接在 `privacy_set_password` 上，且经本屏那条 `commit`（失败要看得见）', () => {
    const sf = parse('mobile/settings/GeneralPage.tsx');
    expect(bodyCalls(sf, 'PrivacyPasswordRow', 'setPassword')).toBe(true);
    expect(
      bodyCalls(sf, 'PrivacyPasswordRow', 'commit'),
      '保存密码不走 `commit` —— 后端在锁屏中会以 `PRIVACY_LOCKED` 拒绝，那句拒绝没人显示',
    ).toBe(true);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * E 组 —— **冷启动补锁**（复审 blocker）。
 *
 * 被守的事：锁定态的唯一真值是 Rust 进程内的一个 `AtomicBool`（`commands/config.rs:783`，
 * 源码写明「随重启复位」），Android 上系统回收整个进程是日常且非自愿的，闲置计时也随进程一起死。
 * 不补这一拍，「锁着 → 放进口袋 → 系统回收进程 → 再打开」得到的是**没有遮罩的解锁界面**，
 * 而 `settings.general.autoPrivacyModeDesc` 对用户承诺的是「用密码锁定界面防偷窥」。
 */
describe('E 冷启动补锁：后端说没锁但配置说该锁 ⇒ 先锁上', () => {
  it('🔴 后端说没锁 + 已设密码 ⇒ 判「该补一次锁」', async () => {
    getPrivacyModeMock.mockImplementation(async () => false);
    hasPasswordMock.mockImplementation(async () => true);
    expect(
      await shouldLockAfterColdStart(),
      '进程被系统回收后重开，界面不会锁上 —— 不输密码直接进全部界面',
    ).toBe(true);
  });

  it('🔴 没设密码 ⇒ 不锁（那样得到的是一个随便敲一个字就过的遮罩，只有摩擦没有保护）', async () => {
    getPrivacyModeMock.mockImplementation(async () => false);
    hasPasswordMock.mockImplementation(async () => false);
    expect(await shouldLockAfterColdStart()).toBe(false);
  });

  it('后端说锁着 ⇒ 不补（水合那一拍已经把遮罩挂上了，再翻一次是纯浪费）', async () => {
    getPrivacyModeMock.mockImplementation(async () => true);
    hasPasswordMock.mockImplementation(async () => true);
    expect(await shouldLockAfterColdStart()).toBe(false);
    expect(hasPasswordMock, '后端已经说锁着了却还去问密码状态').not.toHaveBeenCalled();
  });

  it('读不到后端状态 ⇒ 按「没锁」继续判（保守方向：这一档正是要补锁的那一档）', async () => {
    getPrivacyModeMock.mockImplementation(async () => {
      throw new Error('ipc down');
    });
    hasPasswordMock.mockImplementation(async () => true);
    expect(await shouldLockAfterColdStart()).toBe(true);
  });

  it('🔴 本进程只做一次（第二次连一条 IPC 都不发；复位之后又能做）', async () => {
    getPrivacyModeMock.mockImplementation(async () => false);
    hasPasswordMock.mockImplementation(async () => true);
    expect(await shouldLockAfterColdStart()).toBe(true);
    const calls = getPrivacyModeMock.mock.calls.length;
    expect(await shouldLockAfterColdStart(), '第二次又判了一遍').toBe(false);
    expect(getPrivacyModeMock.mock.calls.length, '第二次仍然打了后端').toBe(calls);
    // 正向对照：复位之后它又做得成（证明上面那个 false 不是「这条腿整个坏了」）。
    resetColdStartPrivacyLock();
    expect(await shouldLockAfterColdStart()).toBe(true);
  });

  it('🔴 生产那一跳：挂在 `useMobileAppWiring` 上、依赖跟着开关走、判 true 时真的去锁', () => {
    const sf = parse('mobile/app-wiring.ts');
    expect(
      effectCallsIn(sf, 'useMobileAppWiring', 'shouldLockAfterColdStart'),
      '移动入口没有挂冷启动补锁 —— 上面五条行为断言测的是那个判定，一条都不会红',
    ).toBe(true);
    expect(
      effectDepsOf(sf, 'useMobileAppWiring', 'shouldLockAfterColdStart'),
      '依赖不是 `[autoPrivacyMode]` —— 空依赖时它只在挂载那一帧跑一次，而那一帧 config 还没载入、' +
        '`autoPrivacyMode` 恒 false ⇒ 早退且永不重跑，一次都不会补锁',
    ).toBe('[autoPrivacyMode]');
    const src = stripComments(readFileSync(join(UI_SRC, 'mobile/app-wiring.ts'), 'utf8'));
    // 开关关着时不补锁（决定了「没开这把锁的人不会被莫名锁上」）。
    expect(
      /if \(!autoPrivacyMode\) return;/.test(src),
      '没开自动隐私锁的用户也会在冷启动被锁上',
    ).toBe(true);
    /*
     * 🔴 判 `true` 之后**真的去翻**，且那一跳罩在 `reportIfFails(` 里。
     * 只断言「调了那个判定」拦不住「判完什么都不做」—— 那一档判定的五条行为断言照样全绿，
     * 而手机上冷启动依然不锁。失败要落全局 toast 的理由同闲置计时那条（没锁上是安全承诺）。
     */
    expect(
      /reportIfFails\(async \(\) => \{\s*if \(await shouldLockAfterColdStart\(\)\) await api\.config\.setPrivacyMode\(true\);\s*\}, i18nRef\.current\.t\('mobileSettings\.general\.privacyLockFailed'\)\)/.test(
        src,
      ),
      '冷启动判「该锁」之后没有真的翻隐私态（或翻了但失败静默）—— 用户以为把手机放下就锁上了',
    ).toBe(true);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * F 组 —— **`FLAG_SECURE`**（复审 major：遮罩结构上挡不住的那一格）。
 *
 * 闲置锁的典型发生时刻是应用**已经在后台**，而 Android 的任务快照在 Activity 停止那一刻就拍完了、
 * 之后不再刷新 ⇒ 锁上之后最近任务里那张缩略图仍是锁定前的一屏（节点名/订阅名/流量数字可读），
 * 全程不需要密码。遮罩在这一格帮不了忙：快照根本早于遮罩存在。
 */
describe('F FLAG_SECURE：跟着开关走，两个方向都要有', () => {
  const withBridge = <T,>(run: (posted: string[]) => T): T => {
    const g = globalThis as unknown as Record<string, unknown>;
    const before = g[SECURE_SCREEN_BRIDGE];
    const posted: string[] = [];
    g[SECURE_SCREEN_BRIDGE] = { postMessage: (m: string) => posted.push(m) };
    try {
      return run(posted);
    } finally {
      if (before === undefined) delete g[SECURE_SCREEN_BRIDGE];
      else g[SECURE_SCREEN_BRIDGE] = before;
    }
  };

  it('🔴 开 ⇒ 投 `on`；关 ⇒ 投 `off`（少了 `off` 那一半 = 关掉隐私锁后截图永久不能用）', () => {
    withBridge((posted) => {
      expect(applySecureScreen(true)).toBe(true);
      expect(applySecureScreen(false)).toBe(true);
      expect(posted, '两个方向没有各投一条').toEqual([SECURE_SCREEN_ON, SECURE_SCREEN_OFF]);
    });
  });

  it('桥不在 ⇒ 如实返 false（「桥不在也算办到了」会让这条安全腿失效时无声无息）', () => {
    const g = globalThis as unknown as Record<string, unknown>;
    expect(g[SECURE_SCREEN_BRIDGE], '前一条用例没把假桥收干净').toBeUndefined();
    expect(hasSecureScreenBridge()).toBe(false);
    expect(applySecureScreen(true)).toBe(false);
    // 正向对照：桥在时它认得出来（证明上面那两个 false 不是恒 false）。
    withBridge(() => {
      expect(hasSecureScreenBridge()).toBe(true);
    });
  });

  it('🔴 生产那一跳：`useMobileAppWiring` 真的挂了它，且依赖跟着**开关**走（不是锁定态）', () => {
    const sf = parse('mobile/app-wiring.ts');
    expect(
      effectCallsIn(sf, 'useMobileAppWiring', 'applySecureScreen'),
      '移动入口没有挂 FLAG_SECURE —— 最近任务里那张缩略图仍是锁定前的一屏',
    ).toBe(true);
    expect(
      effectDepsOf(sf, 'useMobileAppWiring', 'applySecureScreen'),
      '依赖不是 `[autoPrivacyMode]` —— 跟着锁定态走等于「锁上那一刻才置 flag」，' +
        '而那一刻快照早已拍完，置了也不会重拍',
    ).toBe('[autoPrivacyMode]');
    const src = stripComments(readFileSync(join(UI_SRC, 'mobile/app-wiring.ts'), 'utf8'));
    expect(
      /applySecureScreen\(autoPrivacyMode\)/.test(src),
      '喂进去的不是那个开关 —— 恒传字面量时这条腿要么永不生效、要么永远禁着用户的截图',
    ).toBe(true);
  });

  it('🔴 跨语言：Kotlin 侧桥名与两条载荷逐字相同，且 `addFlags`/`clearFlags` 两条腿都在', () => {
    const kt = maskRustComments(
      readFileSync(
        join(UI_SRC, '../../src-tauri/gen/android/app/src/main/java/com/polaris2/app/MainActivity.kt'),
        'utf8',
      ),
    );
    expect(kt.length, 'MainActivity.kt 读到的是空文件 —— 判据面塌了').toBeGreaterThan(400);
    const pick = (name: string): string | undefined =>
      new RegExp(`private const val ${name}\\s*=\\s*"([^"]+)"`).exec(kt)?.[1];
    expect(
      pick('SECURE_SCREEN_BRIDGE'),
      '两侧桥名漂了 —— `postMessage` 打在 undefined 上，隐私锁开着而快照照样可读、运行期零报错',
    ).toBe(SECURE_SCREEN_BRIDGE);
    expect(pick('SECURE_SCREEN_ON')).toBe(SECURE_SCREEN_ON);
    expect(pick('SECURE_SCREEN_OFF')).toBe(SECURE_SCREEN_OFF);
    // 收信口装在 WebView 建好那一刻（不是「整份文件里出现过」）。
    const created = /fun onWebViewCreate\([\s\S]*?\n  \}/.exec(kt)?.[0] ?? '';
    expect(created, '收信口没装 —— `window.polarisSecureScreen` 永远不存在').toContain(
      'installSecureScreenBridge(webView)',
    );
    const fn = /fun installSecureScreenBridge\([\s\S]*?\n  \}/.exec(kt)?.[0] ?? '';
    expect(fn.length, '`installSecureScreenBridge` 的函数体切不出来').toBeGreaterThan(80);
    /*
     * 🔴 两条腿逐个断言。只留 `addFlags` 的表现是「关掉隐私锁之后截图永久不能用，重装才恢复」——
     * 那不是一句更严格的安全策略，是一个用户改不回来的副作用。
     */
    expect(fn, '缺 `addFlags` —— FLAG_SECURE 根本没置上，最近任务里那一屏照样可读').toContain(
      'window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)',
    );
    expect(fn, '缺 `clearFlags` —— 关掉隐私锁之后用户的截图能力再也回不来').toContain(
      'window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)',
    );
  });
});
