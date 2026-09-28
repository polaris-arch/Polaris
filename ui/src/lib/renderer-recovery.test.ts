/**
 * `renderer-recovery` —— 守住「**每个完整应用入口**都真的装了白屏自愈那三层」的门。
 *
 * 被守的缺陷是实证过的：移动端入口 `mobile/MobileMain.tsx` 落地时三层一层都没有，因为那两层实现是
 * 桌面 `main.tsx` 的**模块私有函数**。后果不是"少个功能"，而是 Android 上抛错 = 白屏且零日志。
 *
 * ── 为什么不用「import 了某模块」当判据 ─────────────────────────────────────
 * 「import 了 X」会被「import 了但没调用」骗过 —— 而那正是重构时最容易留下的形态（改调用点、忘了
 * 改 import；或反过来）。故本门全部判据都落在 **AST 上的调用/包裹关系**：
 *   L1 = 入口**顶层语句**里有一次 `installErrorForwarding()` 调用，且**位置早于** createRoot；
 *   L2 = 传给 `.render()` 的 JSX 里恰有一个 `<ErrorBoundary>`，且除外壳（StrictMode）外
 *        **所有**元素都是它的后代（挂在 App 返回值内部 = 等于没挂，见 ErrorBoundary 头注）；
 *   L3 = `.render()` 调用**词法上落在**某个 `try` 块内，该 try 的 `catch` 里调用了
 *        `injectStaticFailureDom`；且 `i18nReady` 链上的 `.catch()` 也落到同一个兜底。
 * import 只作为"名字来源正确"的补充断言，不是主判据。
 *
 * ── 入口花名册不手写 ────────────────────────────────────────────────────────
 * 从 `vite.config.ts` 的 `rollupOptions.input` → 各 html 的 `<script src>` 推导出全部 webview 入口，
 * 再筛出**完整应用入口**（消费主 i18n 的 `i18nReady`；托盘与更新弹窗走 `createAuxI18n`，是零框架/
 * 小窗，本就不该扛这三层）。筛出来的集合再与快照逐值相等 ⇒ 新增第三个完整应用入口时本门立刻转红，
 * 逼人来这里登记，而不是静默地少一个入口没防线。
 *
 * ── 反向对照 ────────────────────────────────────────────────────────────────
 * 「N 项全通过」本身没有信息量 —— 判据坏掉时它也全通过。故 `⓪` 一节把**合成的坏入口**喂进同一批
 * 判据，证明它们报得出错，且报的是对应的那一条（含「import 了但没调用」这个专门的形态）。
 */

import { describe, expect, it, beforeEach, afterEach, vi } from 'vitest';
import * as ts from '@/test/ts-compiler';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

/** `ui/`（本文件在 `ui/src/lib/`）。 */
const UI_ROOT = fileURLToPath(new URL('../../', import.meta.url));
const readUi = (rel: string) => readFileSync(join(UI_ROOT, rel), 'utf8');

/** 共享模块（三层里可复用的那两层的唯一实现）。 */
const SHARED_MODULE = 'src/lib/renderer-recovery.ts';
/** 完整应用入口的快照 —— 真值由下方从 `vite.config.ts` 推导，本表是"必须有人登记"的那一半。 */
const REGISTERED_ENTRIES = ['src/main.tsx', 'src/mobile/MobileMain.tsx'];

// ════════════════════════════════════════════════════════════════════════════
// AST 小工具
// ════════════════════════════════════════════════════════════════════════════

function collect(root: ts.Node, pred: (n: ts.Node) => boolean): ts.Node[] {
  const out: ts.Node[] = [];
  const walk = (n: ts.Node): void => {
    if (pred(n)) out.push(n);
    ts.forEachChild(n, walk);
  };
  walk(root);
  return out;
}

/** `Foo` / `React.StrictMode` 形态的 JSX 标签名（不用 getText，避免依赖 sf 绑定）。 */
function tagOf(n: ts.Node): string {
  if (ts.isIdentifier(n)) return n.text;
  if (ts.isPropertyAccessExpression(n)) return `${tagOf(n.expression)}.${n.name.text}`;
  return '<unknown>';
}

/** 被调函数的名字：`f()` → `f`；`a.b()` → `b`。 */
function calleeName(call: ts.CallExpression): string | null {
  const e = call.expression;
  if (ts.isIdentifier(e)) return e.text;
  if (ts.isPropertyAccessExpression(e) && ts.isIdentifier(e.name)) return e.name.text;
  return null;
}

const callsTo = (root: ts.Node, name: string): ts.CallExpression[] =>
  collect(root, (n) => ts.isCallExpression(n) && calleeName(n) === name) as ts.CallExpression[];

/** `X.createRoot(...).render(arg)` —— 应用挂载点。 */
function renderCalls(sf: ts.SourceFile): ts.CallExpression[] {
  return (collect(sf, (n) => ts.isCallExpression(n)) as ts.CallExpression[]).filter((c) => {
    if (calleeName(c) !== 'render') return false;
    const recv = ts.isPropertyAccessExpression(c.expression) ? c.expression.expression : null;
    return recv !== null && ts.isCallExpression(recv) && calleeName(recv) === 'createRoot';
  });
}

/** JSX 元素（含自闭合）的标签名 + 祖先标签链。 */
interface JsxNode {
  tag: string;
  ancestors: string[];
}
function jsxTree(root: ts.Node): JsxNode[] {
  const out: JsxNode[] = [];
  const walk = (n: ts.Node, ancestors: string[]): void => {
    let next = ancestors;
    if (ts.isJsxElement(n)) {
      const tag = tagOf(n.openingElement.tagName);
      out.push({ tag, ancestors });
      next = [...ancestors, tag];
    } else if (ts.isJsxSelfClosingElement(n)) {
      out.push({ tag: tagOf(n.tagName), ancestors });
    }
    ts.forEachChild(n, (c) => walk(c, next));
  };
  walk(root, []);
  return out;
}

/** 只作为外壳、允许出现在 ErrorBoundary **上方**的标签。 */
const SHELL_TAGS = new Set(['StrictMode', 'React.StrictMode']);

// ════════════════════════════════════════════════════════════════════════════
// 三层判据（纯函数：吃一个 SourceFile，吐违规码 —— 便于喂合成坏输入做反向对照）
// ════════════════════════════════════════════════════════════════════════════

type Violation = 'L1-no-call' | 'L1-not-first' | 'L1-no-import' | 'L2' | 'L3-try' | 'L3-promise';

function auditEntry(sf: ts.SourceFile): Violation[] {
  const bad: Violation[] = [];

  // ── L1 可观测性 ────────────────────────────────────────────────────────────
  // 判据是**顶层语句里的一次调用**，不是"文件里出现过这个名字"。
  const topLevelInstall = sf.statements.filter(
    (s) =>
      ts.isExpressionStatement(s) &&
      ts.isCallExpression(s.expression) &&
      calleeName(s.expression) === 'installErrorForwarding',
  );
  const renders = renderCalls(sf);
  if (topLevelInstall.length === 0) bad.push('L1-no-call');
  // 「必须最先执行」：早于挂载点。挂载点缺失时这条不判（L2 会先报）。
  else if (
    renders.length > 0 &&
    topLevelInstall[0].getStart(sf) > renders[0].getStart(sf)
  ) {
    bad.push('L1-not-first');
  }
  const imported = new Set<string>();
  for (const s of sf.statements) {
    if (!ts.isImportDeclaration(s)) continue;
    const spec = s.moduleSpecifier;
    if (!ts.isStringLiteral(spec) || !/(^|\/)lib\/renderer-recovery$/.test(spec.text)) continue;
    const named = s.importClause?.namedBindings;
    if (named && ts.isNamedImports(named)) for (const e of named.elements) imported.add(e.name.text);
  }
  if (!imported.has('installErrorForwarding') || !imported.has('injectStaticFailureDom')) {
    bad.push('L1-no-import');
  }

  // ── L2 渲染期抛错：根级 ErrorBoundary ──────────────────────────────────────
  // 「根级」= 除 StrictMode 外，render 参数里**每一个**元素都在它下面。挂在应用返回值内部护不住
  // App 函数体的 hooks 与 provider 自身的 render 抛错（上游 A3 实证）。
  const mounted = renders[0]?.arguments[0];
  if (!mounted) {
    bad.push('L2');
  } else {
    const nodes = jsxTree(mounted);
    const boundaries = nodes.filter((n) => n.tag === 'ErrorBoundary');
    const misplaced = nodes.filter(
      (n) => n.tag !== 'ErrorBoundary' && !SHELL_TAGS.has(n.tag) && !n.ancestors.includes('ErrorBoundary'),
    );
    const wrapsSomething = nodes.some((n) => n.ancestors.includes('ErrorBoundary'));
    if (boundaries.length !== 1 || misplaced.length > 0 || !wrapsSomething) bad.push('L2');
  }

  // ── L3 同步 mount 抛错：render 外的 try/catch → 静态兜底 DOM ────────────────
  const guarded =
    renders.length > 0 &&
    (collect(sf, (n) => ts.isTryStatement(n)) as ts.TryStatement[]).some((t) => {
      const at = renders[0].getStart(sf);
      const inTry = at >= t.tryBlock.getStart(sf) && at <= t.tryBlock.end;
      const rescued =
        t.catchClause !== undefined && callsTo(t.catchClause, 'injectStaticFailureDom').length > 0;
      return inTry && rescued;
    });
  if (!guarded) bad.push('L3-try');

  // i18nReady 链上的 rejection 也落同一个兜底（i18n 自己起不来同样是白屏）。
  const promiseRescued = (collect(sf, (n) => ts.isCallExpression(n)) as ts.CallExpression[]).some(
    (c) => calleeName(c) === 'catch' && c.arguments.some((a) => callsTo(a, 'injectStaticFailureDom').length > 0),
  );
  if (!promiseRescued) bad.push('L3-promise');

  return bad;
}

// ════════════════════════════════════════════════════════════════════════════
// 入口花名册（从 vite 多入口推导，不手写）
// ════════════════════════════════════════════════════════════════════════════

/** `vite.config.ts` 的 `rollupOptions.input` 里登记的全部 html。 */
function viteHtmlEntries(): string[] {
  const cfg = readUi('vite.config.ts');
  const at = cfg.indexOf('input: {');
  if (at < 0) throw new Error('vite.config.ts 的 rollupOptions.input 块找不到了 —— 判据面塌了');
  // 值都是 `path.resolve(import.meta.dirname, 'x.html')`，无嵌套花括号 ⇒ 首个 `}` 即块尾。
  const block = cfg.slice(at, cfg.indexOf('}', at));
  return [...block.matchAll(/'([^']+\.html)'/g)].map((m) => m[1]);
}

/** html → 它加载的入口模块（相对 `ui/`）。 */
function entryModuleOf(html: string): string {
  const src = /<script[^>]+src="\/(src\/[^"]+)"/.exec(readUi(html))?.[1];
  if (!src) throw new Error(`${html} 未找到入口模块 —— 判据面塌了`);
  return src;
}

const ALL_ENTRIES = viteHtmlEntries().map(entryModuleOf);
/** 完整应用入口 = 消费主 i18n（`i18nReady`）的那些；辅助窗走 `createAuxI18n`，不在此列。 */
const APP_ENTRIES = ALL_ENTRIES.filter((m) => /\bi18nReady\b/.test(readUi(m)));

const parseUi = (rel: string) => ts.parseSourceFile(join(UI_ROOT, rel), readUi(rel));
/** 合成源码（文件名不在 tsconfig 工程里 ⇒ 走 ts-compiler 的虚拟解析路径）。 */
const parseSynthetic = (name: string, text: string) => ts.parseSourceFile(name, text);

// ════════════════════════════════════════════════════════════════════════════

describe('⓪ 自检 + 反向对照：判据报得出错，且报的是对应那条', () => {
  it('花名册推导是活的（vite 入口 ≥ 3，完整应用入口 ≥ 2）', () => {
    expect(ALL_ENTRIES.length, 'vite 多入口抠空了 —— 下面每条都会空转成绿').toBeGreaterThanOrEqual(3);
    expect(APP_ENTRIES.length).toBeGreaterThanOrEqual(2);
    // 辅助窗确实被筛掉了（否则筛子等于没筛，本门会去要求托盘也挂 ErrorBoundary）。
    expect(ALL_ENTRIES.length).toBeGreaterThan(APP_ENTRIES.length);
  });

  it('完整应用入口集合 = 登记表（新增一个没登记 ⇒ 红）', () => {
    expect([...APP_ENTRIES].sort()).toEqual([...REGISTERED_ENTRIES].sort());
  });

  it('AST 走查器解析得到东西（解析器塌了会退化成"什么都没扫到所以全绿"）', () => {
    for (const entry of APP_ENTRIES) {
      const sf = parseUi(entry);
      expect(renderCalls(sf).length, `${entry} 里找不到 createRoot(...).render(...)`).toBe(1);
      expect(sf.statements.length, `${entry} 语句数异常`).toBeGreaterThan(5);
    }
  });

  it('三层全被摘掉的合成入口 ⇒ 三条全报（阳性对照）', () => {
    const stripped = `
import ReactDOM from 'react-dom/client';
import App from './App';
import { i18nReady } from './i18n';
const rootEl = document.getElementById('root');
if (rootEl) {
  void i18nReady.then(() => {
    ReactDOM.createRoot(rootEl).render(<App />);
  });
}
`;
    const bad = auditEntry(parseSynthetic('synthetic-stripped.tsx', stripped));
    expect(bad.sort()).toEqual(['L1-no-call', 'L1-no-import', 'L2', 'L3-promise', 'L3-try']);
  });

  it('「import 了但没调用」⇒ 仍报 L1（本门不是钉 import 的假门）', () => {
    const importedNotCalled = `
import ReactDOM from 'react-dom/client';
import ErrorBoundary from './components/ErrorBoundary';
import { i18nReady } from './i18n';
import { injectStaticFailureDom, installErrorForwarding } from './lib/renderer-recovery';
const rootEl = document.getElementById('root');
if (rootEl) {
  void i18nReady
    .then(() => {
      try {
        ReactDOM.createRoot(rootEl).render(
          <ErrorBoundary>
            <App />
          </ErrorBoundary>
        );
      } catch (err) {
        injectStaticFailureDom(rootEl, err);
      }
    })
    .catch((err: unknown) => injectStaticFailureDom(rootEl, err));
}
`;
    expect(auditEntry(parseSynthetic('synthetic-import-only.tsx', importedNotCalled))).toEqual([
      'L1-no-call',
    ]);
  });

  it('ErrorBoundary 挂在应用内部（不在根）⇒ 报 L2', () => {
    const inner = `
import ReactDOM from 'react-dom/client';
import { i18nReady } from './i18n';
import { injectStaticFailureDom, installErrorForwarding } from './lib/renderer-recovery';
installErrorForwarding();
const rootEl = document.getElementById('root');
if (rootEl) {
  void i18nReady
    .then(() => {
      try {
        ReactDOM.createRoot(rootEl).render(
          <App>
            <ErrorBoundary>
              <Inner />
            </ErrorBoundary>
          </App>
        );
      } catch (err) {
        injectStaticFailureDom(rootEl, err);
      }
    })
    .catch((err: unknown) => injectStaticFailureDom(rootEl, err));
}
`;
    expect(auditEntry(parseSynthetic('synthetic-inner-boundary.tsx', inner))).toEqual(['L2']);
  });

  it('`installErrorForwarding()` 排在挂载之后 ⇒ 报 L1-not-first', () => {
    const late = `
import ReactDOM from 'react-dom/client';
import ErrorBoundary from './components/ErrorBoundary';
import { i18nReady } from './i18n';
import { injectStaticFailureDom, installErrorForwarding } from './lib/renderer-recovery';
const rootEl = document.getElementById('root');
try {
  ReactDOM.createRoot(rootEl).render(
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  );
} catch (err) {
  injectStaticFailureDom(rootEl, err);
}
void i18nReady.catch((err: unknown) => injectStaticFailureDom(rootEl, err));
installErrorForwarding();
`;
    expect(auditEntry(parseSynthetic('synthetic-late-install.tsx', late))).toEqual(['L1-not-first']);
  });
});

describe('① 两个完整应用入口都装齐了三层', () => {
  for (const entry of REGISTERED_ENTRIES) {
    it(`${entry}`, () => {
      expect(
        auditEntry(parseUi(entry)),
        `${entry} 的白屏防线有缺口：\n` +
          '  L1-* = 可观测性（console.error / onerror / unhandledrejection → Rust 日志）；' +
          '缺它 = 抛错白屏且零日志\n' +
          '  L2   = 根级 <ErrorBoundary>（必须包住应用根及其所有 provider）\n' +
          '  L3-* = createRoot().render() 外的 try/catch 与 i18nReady 的 .catch → injectStaticFailureDom',
      ).toEqual([]);
    });
  }
});

describe('② 共享模块不是空壳（三层的实现真的在里面）', () => {
  const sf = parseUi(SHARED_MODULE);
  const fnBody = (name: string): ts.Node => {
    const fn = collect(sf, (n) => ts.isFunctionDeclaration(n) && n.name?.text === name)[0];
    expect(fn, `${SHARED_MODULE} 里找不到 ${name} —— 两个入口调的可能是别的东西`).toBeDefined();
    return fn;
  };

  it('`installErrorForwarding` 三个信号源一个不少', () => {
    const body = fnBody('installErrorForwarding');
    // console.error 旁路（赋值，不是只读一下）。
    const patches = collect(
      body,
      (n) =>
        ts.isBinaryExpression(n) &&
        ts.isPropertyAccessExpression(n.left) &&
        ts.isIdentifier(n.left.expression) &&
        n.left.expression.text === 'console' &&
        n.left.name.text === 'error',
    );
    expect(patches.length, 'console.error 没被旁路 —— renderer 的错误日志一条都到不了 Rust').toBe(1);
    // window 上的两个事件。
    const listened = callsTo(body, 'addEventListener')
      .map((c) => (c.arguments[0] && ts.isStringLiteral(c.arguments[0]) ? c.arguments[0].text : ''))
      .sort();
    expect(listened).toEqual(['error', 'unhandledrejection']);
  });

  it('`injectStaticFailureDom` 上报 + 走 i18n 文案 + 兑现上屏', () => {
    const body = fnBody('injectStaticFailureDom');
    expect(callsTo(body, 'recoveryText').length, '兜底页文案没走 i18n（recovery-text）').toBe(3);
    expect(callsTo(body, 'replaceChildren').length, '兜底 DOM 没真的挂到 root 上').toBe(1);
    expect(callsTo(body, 'reportRendererReady').length, '兜底页没兑现上屏，用户还要多等一轮终局页').toBe(1);
  });

  it('逃生文案不依赖 i18next 运行期查询（i18n 自己可能就是没起来的那个）', () => {
    // 取材面是 **import 声明**而非原文：本文件头注里正写着「不依赖 React / i18next / theme」，
    // 扫原文会把自己的说明当成违规（判据被自己污染）。
    const specs = sf.statements
      .filter((st) => ts.isImportDeclaration(st))
      .map((st) => (ts.isStringLiteral(st.moduleSpecifier) ? st.moduleSpecifier.text : ''));
    expect(specs, '自检：import 一条都没解析出来').not.toHaveLength(0);
    expect(specs, '逃生文案必须走同步 auxiliary 词表').toContain('@/i18n/recovery-text');
    for (const bad of ['i18next', 'react-i18next', '@/i18n']) {
      expect(specs, `共享兜底模块引了 ${bad} —— 回到了会失败的那条路上`).not.toContain(bad);
    }
    // 运行期 `t(...)` / `useTranslation()` 同样是回到 i18next 上。
    expect(callsTo(sf, 't').length, '出现了运行期 i18next 查询').toBe(0);
    expect(callsTo(sf, 'useTranslation').length).toBe(0);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ③ 行为层：结构对不等于跑起来对（实测 > review）
// ════════════════════════════════════════════════════════════════════════════

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

interface FakeEl {
  style: Record<string, string>;
  type?: string;
  textContent?: string;
  children: FakeEl[];
  append: (...kids: FakeEl[]) => void;
  replaceChildren: (...kids: FakeEl[]) => void;
  addEventListener: () => void;
}

function fakeEl(): FakeEl {
  const el: FakeEl = {
    style: {},
    children: [],
    append: (...kids: FakeEl[]) => el.children.push(...kids),
    replaceChildren: (...kids: FakeEl[]) => el.children.splice(0, el.children.length, ...kids),
    addEventListener: () => {},
  };
  return el;
}

describe('③ 行为：三层跑起来真的产生信号', () => {
  const saved = {
    error: console.error,
    window: Reflect.get(globalThis, 'window') as unknown,
    document: Reflect.get(globalThis, 'document') as unknown,
  };
  let listeners: Map<string, (e: unknown) => void>;

  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
    vi.resetModules();
    listeners = new Map();
    Reflect.set(globalThis, 'window', {
      addEventListener: (t: string, h: (e: unknown) => void) => listeners.set(t, h),
      location: { reload: () => {} },
    });
  });

  const restore = (key: 'window' | 'document', value: unknown): void => {
    if (value === undefined) Reflect.deleteProperty(globalThis, key);
    else Reflect.set(globalThis, key, value);
  };

  afterEach(() => {
    console.error = saved.error;
    restore('window', saved.window);
    restore('document', saved.document);
  });

  /** 上报到 renderer_log 的全部消息。 */
  const logged = (): string[] =>
    invokeMock.mock.calls
      .filter((c) => c[0] === 'renderer_log')
      .map((c) => String((c[1] as { message?: unknown } | undefined)?.message ?? ''));

  it('console.error / onerror / unhandledrejection 三条都转发到 Rust 日志', async () => {
    const { installErrorForwarding } = await import('./renderer-recovery');
    const seen: unknown[][] = [];
    console.error = (...a: unknown[]) => seen.push(a);
    installErrorForwarding();

    console.error('boom', new Error('kaput'));
    listeners.get('error')?.({ error: new Error('onerror'), message: 'onerror' });
    listeners.get('unhandledrejection')?.({ reason: new Error('rejected') });

    // 原实现仍被调用（devtools / vite overlay 里还看得见）。
    expect(seen).toHaveLength(1);
    const messages = logged();
    expect(messages).toHaveLength(3);
    expect(messages[0]).toContain('boom');
    expect(messages[0]).toContain('kaput');
    expect(messages[1]).toContain('onerror');
    expect(messages[2]).toContain('rejected');
  });

  it('装载前抛的同类错误没有任何上报（阴性对照：上面三条不是恒真）', async () => {
    await import('./renderer-recovery');
    console.error = () => {};
    console.error('before install');
    expect(logged()).toEqual([]);
  });

  it('兜底 DOM：上报失败原因 + 文案来自 recovery-text + 立刻兑现上屏', async () => {
    const created: FakeEl[] = [];
    Reflect.set(globalThis, 'document', {
      // `recovery-text` 建 aux i18n 时会写 `<html lang/dir>`（RTL）—— 假 document 也得有这一处。
      documentElement: {} as Record<string, string>,
      createElement: () => {
        const el = fakeEl();
        created.push(el);
        return el;
      },
    });
    const { injectStaticFailureDom } = await import('./renderer-recovery');
    const { recoveryText } = await import('@/i18n/recovery-text');
    const root = fakeEl();

    injectStaticFailureDom(root as unknown as HTMLElement, new Error('mount exploded'));

    expect(logged()[0], '同步 mount 失败没有上报 —— 这条路径的唯一可观测信号').toContain(
      'mount exploded',
    );
    // 文案是 i18n 取出来的真句子，不是键名、不是硬编码。
    const texts = created.map((e) => e.textContent).filter((t): t is string => Boolean(t));
    expect(texts).toEqual([recoveryText('title'), recoveryText('body'), recoveryText('reload')]);
    for (const t of texts) expect(t).not.toContain('native.fatalPage');
    // 兜底页真的挂上了，且立刻回发 ready（否则用户还要多等一轮终局页）。
    expect(root.children).toHaveLength(1);
    expect(invokeMock.mock.calls.map((c) => c[0])).toContain('renderer_ready');
  });
});
