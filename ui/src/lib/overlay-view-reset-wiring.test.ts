/**
 * G14 —— 独立窗浮层的**视图态复位**守卫。
 *
 * # 守什么
 *
 * 原型的托盘每次打开都回主视图（`proto:4009 openTray(){ … trayView(false); … }`）—— 浮层是「弹出即用完就收」
 * 的表面，上次停在哪跟这次没关系。实现把托盘做成了**常驻不重建的独立 Tauri 窗**（`src-tauri/src/tray.rs`
 * 「预建一次（隐藏）」），于是 React 组件永不卸载、`useState` 永不重置：上次停在二级视图，下次点托盘图标
 * 大概率还停在二级。这不是渲染问题，是「窗生命周期变了、复位腿没跟着补」。
 *
 * 常驻窗把「组件卸载即复位」这条隐式保障拿掉了，而拿掉它是**静默**的 —— 没有任何测试会因此转红。
 * 本门就是把它变成显式的：浮层里每一个联合字面量 `useState` 都要登记，视图态必须有 show/focus 复位腿。
 *
 * # 判据面
 *
 *  - `ui/src/tray/**` 与 `ui/src/update-popup/**` 的非测试源码；
 *  - 其中形如 `const [x, setX] = useState<'a' | 'b'>('a')` 的**联合字面量 state**；
 *  - focus 处理器与成功 show 桥的有界 AST 调用链（内联、具名 const、唯一 ref 回调）。
 *
 * # 什么算「视图态」
 *
 * 联合成员**全是字符串字面量**（不含 `null` / `undefined`）**且默认值是其中之一** ⇒ 视图态，必须有复位腿。
 * 含 `null` 的（如 `pending: 'start' | 'stop' | null`）是「在飞方向」，天然以 `null` 为静息值、不是层级视图；
 * 它们照样进登记表（改成视图态时门会说话），只是不要求复位腿。
 *
 * # 为什么是源码结构守卫
 *
 * 本仓 vitest 是 `environment:'node'`（无 jsdom，有意为之）⇒ 组件渲染不了，「窗再弹出时停在哪一屏」
 * 在这一层不可观测。「focus 处理器里有没有那句复位」是纯文本可断言的，且正是缺陷所在的那一层。
 * **真机仍需看一眼**：窗显隐由 Rust 侧控制，`focus` 事件是否每次弹出都触发要设备上确认。
 */
import { describe, it, expect } from 'vitest';
import { parse } from '@babel/parser';
import { IS_TEST_ONLY_MODULE } from '@/contracts/test-only-modules';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const SRC = fileURLToPath(new URL('..', import.meta.url));

/** 浮层目录 —— 独立 Tauri 窗承载、组件永不卸载的那几处。 */
const OVERLAY_DIRS = ['tray', 'update-popup'] as const;

function walk(dir: string, acc: string[] = []): string[] {
  for (const e of readdirSync(dir)) {
    if (e === 'node_modules' || e === 'dist') continue;
    const full = join(dir, e);
    if (statSync(full).isDirectory()) walk(full, acc);
    // 共享谓词（`contracts/test-only-modules.ts` 头注：三道门需要同一个概念，不许各留一份拷贝）。
    // `.test-support.` 同样不进产物，且产品代码不许 import 它们（`i18n-coverage` G0-b 锁着）——
    // 把它们留在产品面上，判据会被别的判据的**锚文本**喂饱（2026-09-06 在 app-wiring ⑩/⑫ 实测过一次假绿）。
    else if (/\.tsx?$/.test(e) && !IS_TEST_ONLY_MODULE.test(e)) acc.push(full);
  }
  return acc;
}

/** 去注释（注释里逐字写着 `setView('main')` 这类说明，直接扫会把「解释」当成「实现」）。 */
function code(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/.*$/gm, '$1');
}

const FILES = OVERLAY_DIRS.flatMap((d) => walk(join(SRC, d))).map((f) => ({
  rel: relative(SRC, f).split(sep).join('/'),
  src: readFileSync(f, 'utf8'),
}));

// ── 解析器 ──────────────────────────────────────────────────────────────────

interface UnionState {
  file: string;
  name: string;
  setter: string;
  members: string[];
  /** 联合里是否含 `null` / `undefined`（含则不是层级视图，是「在飞/静息」二元态）。 */
  nullable: boolean;
  init: string;
}

function parseUnionStates(rel: string, src: string): UnionState[] {
  const out: UnionState[] = [];
  const re = /const\s*\[\s*(\w+)\s*,\s*(set\w+)\s*\]\s*=\s*useState<([^>]*'[^>]*)>\(([^)]*)\)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(src))) {
    const typeText = m[3];
    const members = [...typeText.matchAll(/'([^']*)'/g)].map((x) => x[1]);
    if (members.length < 2) continue;
    out.push({
      file: rel,
      name: m[1],
      setter: m[2],
      members,
      nullable: /\bnull\b|\bundefined\b/.test(typeText),
      init: m[4].trim(),
    });
  }
  return out;
}

/** Bounded AST call graph: only direct executable statements are followed. Nested
 * unused functions, comments, strings, conditional decoys and unresolved bindings
 * cannot stand in for a reset. Four edges cover focus -> ref -> reset today.
 */
type FlowNode = { type: string; [key: string]: unknown };
const flowNode = (value: unknown): value is FlowNode => typeof value === 'object' && value !== null
  && 'type' in value && typeof value.type === 'string';
const identifier = (value: unknown, name: string): boolean => flowNode(value) && value.type === 'Identifier' && value.name === name;
const stringValue = (value: unknown, text: string): boolean => flowNode(value) && value.type === 'StringLiteral' && value.value === text;
function reference(value: unknown): string | null {
  if (!flowNode(value)) return null;
  if (value.type === 'Identifier' && typeof value.name === 'string') return value.name;
  if (value.type === 'MemberExpression' && value.computed === false && flowNode(value.property)
    && value.property.type === 'Identifier') {
    const object = reference(value.object);
    return object === null ? null : `${object}.${String(value.property.name)}`;
  }
  return null;
}
function parseFlow(src: string): FlowNode[] {
  const ast = parse(src, { sourceType: 'module', plugins: ['typescript', 'jsx'] });
  const nodes: FlowNode[] = [];
  // Only the component render and its directly registered effects execute here.
  // Never collect registrations or bindings inside an unrelated unused function.
  function statements(value: unknown): void {
    for (const statement of Array.isArray(value) ? value.filter(flowNode) : []) {
      if (statement.type === 'ExportDefaultDeclaration' && flowNode(statement.declaration)) {
        statements([statement.declaration]);
      } else if (statement.type === 'FunctionDeclaration' && identifier(statement.id, 'TrayMenu')
        && flowNode(statement.body)) {
        statements(statement.body.body);
      } else if (statement.type === 'VariableDeclaration' && Array.isArray(statement.declarations)) {
        nodes.push(...statement.declarations.filter(flowNode));
      } else if (statement.type === 'ExpressionStatement' && flowNode(statement.expression)) {
        const expression = statement.expression;
        nodes.push(expression);
        if (expression.type === 'CallExpression' && ['useEffect', 'useLayoutEffect'].some(name => identifier(expression.callee, name))
          && Array.isArray(expression.arguments) && flowNode(expression.arguments[0])) {
          const effect = expression.arguments[0];
          if (effect.type === 'ArrowFunctionExpression' && flowNode(effect.body) && effect.body.type === 'BlockStatement') {
            statements(effect.body.body);
          }
        }
      } else if (statement.type === 'ReturnStatement' || statement.type === 'ThrowStatement') break;
    }
  }
  statements(ast.program.body);
  return nodes;
}
function callback(target: unknown, nodes: FlowNode[]): FlowNode | null {
  if (!flowNode(target)) return null;
  if (target.type === 'ArrowFunctionExpression' || target.type === 'FunctionExpression') return target;
  const name = reference(target);
  if (!name) return null;
  const declarations = nodes.filter(n => n.type === 'VariableDeclarator' && reference(n.id) === name);
  const assignments = nodes.filter(n => n.type === 'AssignmentExpression' && reference(n.left) === name);
  // Reassigned identifiers and multiple ref writes are intentionally not inferred.
  const values = name.endsWith('.current')
    ? assignments.filter(n => n.operator === '=').map(n => n.right)
    : assignments.length ? [] : declarations.map(n => n.init);
  if (values.length !== 1 || !flowNode(values[0])) return null;
  const value = values[0];
  return value.type === 'ArrowFunctionExpression' || value.type === 'FunctionExpression' ? value : null;
}
function directExpressions(body: unknown): FlowNode[] {
  if (!flowNode(body)) return [];
  if (body.type !== 'BlockStatement') return [body];
  const out: FlowNode[] = [];
  for (const statement of Array.isArray(body.body) ? body.body.filter(flowNode) : []) {
    if (statement.type === 'ExpressionStatement' && flowNode(statement.expression)) out.push(statement.expression);
    // Anything after return is unreachable. Never walk nested declarations or branches.
    if (statement.type === 'ReturnStatement') {
      if (flowNode(statement.argument)) out.push(statement.argument);
      break;
    }
    if (statement.type === 'ThrowStatement') break;
  }
  return out;
}
function reachesReset(target: unknown, nodes: FlowNode[], st: UnionState, depth = 0): boolean {
  if (depth > 4) return false;
  const fn = callback(target, nodes);
  if (!fn) return false;
  return directExpressions(fn.body).some(expression => {
    if (expression.type !== 'CallExpression' || !Array.isArray(expression.arguments)) return false;
    const args = expression.arguments;
    const reset = identifier(expression.callee, st.setter) && args.length === 1
      && (st.init === 'null' ? flowNode(args[0]) && args[0].type === 'NullLiteral'
        : stringValue(args[0], st.init.slice(1, -1)));
    return reset || (args.length === 0 && reachesReset(expression.callee, nodes, st, depth + 1));
  });
}
function focusCallbacks(nodes: FlowNode[]): unknown[] {
  return nodes.filter(n => n.type === 'CallExpression' && reference(n.callee) === 'window.addEventListener'
    && Array.isArray(n.arguments) && n.arguments.length === 2 && stringValue(n.arguments[0], 'focus'))
    .map(n => (n.arguments as unknown[])[1]);
}
function hasResetLeg(src: string, st: UnionState): boolean {
  const nodes = parseFlow(src);
  return focusCallbacks(nodes).some(target => reachesReset(target, nodes, st));
}
function hasShowResetLeg(src: string, st: UnionState): boolean {
  const nodes = parseFlow(src);
  const assignments = nodes.filter(n => n.type === 'AssignmentExpression' && n.operator === '='
    && reference(n.left) === 'window.__POLARIS_TRAY_REMEASURE__');
  if (assignments.length !== 1) return false;
  const fn = callback(assignments[0].right, nodes);
  if (!fn || !Array.isArray(fn.params) || fn.params.length !== 1 || !identifier(fn.params[0], 'reason')
    || !flowNode(fn.body) || !Array.isArray(fn.body.body)) return false;
  const statements = fn.body.body;
  // Keep geometry/unknown calls measurement-only, and only an exact show reason resets.
  if (statements.length !== 2 || !flowNode(statements[0]) || statements[0].type !== 'ExpressionStatement'
    || !flowNode(statements[0].expression) || statements[0].expression.type !== 'CallExpression'
    || !identifier(statements[0].expression.callee, 'requestRemeasure')) return false;
  const branch = statements[1];
  if (!flowNode(branch) || branch.type !== 'IfStatement' || branch.alternate !== null
    || !flowNode(branch.test) || branch.test.type !== 'BinaryExpression' || branch.test.operator !== '==='
    || !identifier(branch.test.left, 'reason') || !stringValue(branch.test.right, 'show')) return false;
  const consequent = branch.consequent;
  if (!flowNode(consequent) || consequent.type !== 'ExpressionStatement' || !flowNode(consequent.expression)
    || consequent.expression.type !== 'CallExpression' || reference(consequent.expression.callee) !== 'refreshOnShow.current'
    || !Array.isArray(consequent.expression.arguments) || consequent.expression.arguments.length) return false;
  return reachesReset(consequent.expression.callee, nodes, st);
}

/** Rust is not TS: inspect this single function's executable body, not the whole file.
 * The early return on show failure must precede the precise show bridge.
 */
function hostShowsOnlyAfterSuccess(source: string): boolean {
  const src = code(source);
  const start = src.indexOf('pub(super) fn show_ready_overlay(');
  if (start < 0) return false;
  const next = src.indexOf('\npub', start + 1);
  const body = src.slice(start, next < 0 ? undefined : next);
  const show = /if let Err\(e\) = win\.show\(\)\s*\{\s*log::warn!\("(?:\\.|[^"\\])*"\);\s*return;\s*\}/.exec(body);
  if (!show) return false;
  const tail = body.slice(show.index + show[0].length);
  return /^\s*reposition\(win\);\s*let _ = win\.eval\("window\.__POLARIS_TRAY_REMEASURE__\?\.\('show'\);"\);/.test(tail);
}

const STATES = FILES.flatMap((f) => parseUnionStates(f.rel, code(f.src)));

// ── 自曝 ────────────────────────────────────────────────────────────────────

if (FILES.length === 0) {
  throw new Error(`[overlay-view-reset] 浮层目录一个源文件都没扫到（${OVERLAY_DIRS.join(', ')}）`);
}
if (!FILES.some((f) => f.rel === 'tray/TrayMenu.tsx')) {
  throw new Error('[overlay-view-reset] 锚点缺失：tray/TrayMenu.tsx —— 被改名/移走了？');
}
if (STATES.length === 0) {
  throw new Error(
    '[overlay-view-reset] 浮层里一个联合字面量 state 都没解析出来 —— 解析器塌了，登记表会恒绿',
  );
}

// ── 登记表 ─────────────────────────────────────────────────────────────────

/**
 * 浮层里全部联合字面量 state。**表外出现新的 → 转红**（新加一个视图态必须显式登记复位口径）。
 *
 * `role`：`view` = 层级视图，必须有 show/focus 复位腿；`transient` = 在飞/静息二元态，不要求复位。
 * `reset`：`wired` = 磁盘上确实有复位腿；`missing` = 没有（逐条写清是待修还是有署名依据）。
 * 两个字段都与磁盘现状**双向**对账：修好了不改登记同样转红。
 */
interface Row {
  file: string;
  name: string;
  role: 'view' | 'transient';
  reset: 'wired' | 'missing';
  note: string;
}

const REGISTRY: readonly Row[] = [
  {
    file: 'tray/TrayMenu.tsx',
    name: 'view',
    role: 'view',
    reset: 'wired',
    note:
      '对齐原型 `proto:4009 openTray()`：focus 与宿主成功 show 都经 refreshOnShow.current 复位主视图。' +
      '托盘窗常驻不重建（tray.rs 「预建一次（隐藏）」）⇒ 组件永不卸载、state 永不自动复位，' +
      '两条入口共享复位实现；非激活窗可能不发 focus，宿主 show 桥必须保留。' +
      '切节点 / 切直连处的复位只覆盖「选完就关」那条路径，按返回键或点外部关闭时不经过它们，' +
      '故不能拿那两处顶替本条。**「实际停在哪」仍需真机确认**：窗显隐由 Rust 侧控，' +
      'focus 事件是否每次弹出都触发，这一层看不到。',
  },
  {
    file: 'tray/TrayMenu.tsx',
    name: 'pending',
    role: 'transient',
    reset: 'missing',
    note:
      '不要求复位：`\'start\' | \'stop\' | null` 是本窗发起的在飞启停**方向**，静息值就是 null，' +
      '由启停结果自己收敛（不是层级视图）。登记在此是为了「它哪天变成视图态时门会说话」。',
  },
  {
    file: 'tray/TrayMenu.tsx',
    name: 'updateResult',
    role: 'transient',
    reset: 'wired',
    note:
      '`latest | failed | null` 是单次展开会话里的检查更新回执，不是层级视图；共享 show/focus 回调通过 ' +
      '`setUpdateResult(null)` 明确清掉上一轮结果，避免 warm WebView 下次展开仍展示陈旧结论。',
  },
] as const;

// ── 断言 ────────────────────────────────────────────────────────────────────

describe('守卫自检：解析器正反两向都判得动（防「恒判缺失」让登记表恒真）', () => {
  it('联合字面量 state 解析器在合成样本上命中，且能区分含 null 的', () => {
    const sample = [
      "const [view, setView] = useState<'main' | 'nodes'>('main');",
      "const [pending, setPending] = useState<'start' | 'stop' | null>(null);",
      'const [n, setN] = useState<number>(0);',
      "const [s, setS] = useState<'only'>('only');",
    ].join('\n');
    expect(
      parseUnionStates('x.tsx', sample).map((s) => `${s.name}:${s.members.join('|')}:${s.nullable}`),
    ).toEqual(['view:main|nodes:false', 'pending:start|stop:true']);
  });

  it('复位腿检测器能判「有」——正向对照（否则它只会永远报缺失）', () => {
    const wired = [
      "const [view, setView] = useState<'main' | 'nodes'>('main');",
      'const onFocus = () => {',
      '  applyTheme();',
      "  setView('main');",
      '};',
      "window.addEventListener('focus', onFocus);",
    ].join('\n');
    const st = parseUnionStates('x.tsx', wired)[0];
    expect(hasResetLeg(wired, st), '具名 focus 处理器里的复位腿没被认出来').toBe(true);

    const inline = [
      "const [view, setView] = useState<'main' | 'nodes'>('main');",
      "window.addEventListener('focus', () => { setView('main'); });",
    ].join('\n');
    expect(hasResetLeg(inline, parseUnionStates('x.tsx', inline)[0]), '内联写法没被认出来').toBe(
      true,
    );
  });

  it('focus/ref and show/ref chains reject disconnected, conditional and stale decoys', () => {
    const sample = [
      "const [view, setView] = useState<'main' | 'nodes'>('main');",
      'const refreshOnShow = useRef(() => {});',
      "refreshOnShow.current = () => { setView('main'); };",
      'const onFocus = () => refreshOnShow.current();',
      "window.addEventListener('focus', onFocus);",
      "window.__POLARIS_TRAY_REMEASURE__ = (reason) => { requestRemeasure(); if (reason === 'show') refreshOnShow.current(); };",
    ].join('\n');
    const state = parseUnionStates('x.tsx', sample)[0];
    expect(hasResetLeg(sample, state)).toBe(true);
    expect(hasShowResetLeg(sample, state)).toBe(true);
    for (const broken of [
      sample.replace("refreshOnShow.current = () => { setView('main'); };", ''),
      sample.replace("setView('main');", "/* setView('main'); */"),
      sample.replace("setView('main');", "setView('nodes');"),
      sample.replace("setView('main');", "const unused = () => setView('main');"),
      sample.replace("setView('main');", "if (false) setView('main');"),
      sample.replace("setView('main');", "return; setView('main');"),
      sample + '\nrefreshOnShow.current = () => {};',
      'const unused = () => { ' + sample + ' };',
    ]) {
      expect(hasResetLeg(broken, state)).toBe(false);
      expect(hasShowResetLeg(broken, state)).toBe(false);
    }
    expect(hasResetLeg(sample.replace('() => refreshOnShow.current()', '() => {}'), state)).toBe(false);
    expect(hasResetLeg(sample.replace("'focus', onFocus", "'blur', onFocus"), state)).toBe(false);
    expect(hasShowResetLeg(sample.replace("reason === 'show'", "reason === 'geometry'"), state)).toBe(false);
    expect(hasShowResetLeg(sample.replace("if (reason === 'show') ", ''), state)).toBe(false);
    expect(hasShowResetLeg(sample.replace("if (reason === 'show') refreshOnShow.current();", "if (reason === 'show') requestRemeasure();"), state)).toBe(false);
  });

  it('host show notification follows successful show and never failure or geometry', () => {
    const host = readFileSync(join(SRC, '../../src-tauri/src/tray/window.rs'), 'utf8');
    expect(hostShowsOnlyAfterSuccess(host)).toBe(true);
    const start = host.indexOf('pub(super) fn show_ready_overlay(');
    const before = host.slice(0, start);
    const body = host.slice(start);
    expect(hostShowsOnlyAfterSuccess(before + body.replace("?.('show')", "?.('geometry')"))).toBe(false);
    expect(hostShowsOnlyAfterSuccess(before + body.replace('return;', ''))).toBe(false);
    expect(hostShowsOnlyAfterSuccess(before + body.replace('win.show()', 'win.hide()'))).toBe(false);
    expect(hostShowsOnlyAfterSuccess('/* pub(super) fn show_ready_overlay() { if let Err(e) = win.show() { return; } } */')).toBe(false);
  });

  it('复位腿检测器能判「无」——负向对照', () => {
    const missing = [
      "const [view, setView] = useState<'main' | 'nodes'>('main');",
      "const onFocus = () => { applyTheme(); };",
      "window.addEventListener('focus', onFocus);",
      "const switchNode = () => { setView('main'); };", // 不在 focus 腿里 ⇒ 不算
    ].join('\n');
    expect(hasResetLeg(missing, parseUnionStates('x.tsx', missing)[0])).toBe(false);
  });

  it('去注释真的生效（注释里写着 setView(\'main\') 不算实现）', () => {
    const commented = [
      "const [view, setView] = useState<'main' | 'nodes'>('main');",
      "const onFocus = () => { /* 这里应当 setView('main') */ };",
      "window.addEventListener('focus', onFocus);",
    ].join('\n');
    const src = code(commented);
    expect(hasResetLeg(src, parseUnionStates('x.tsx', src)[0])).toBe(false);
  });
});

describe('G14：浮层视图态必须登记，视图态必须有 show/focus 复位腿', () => {
  it('浮层里的联合字面量 state 集合与登记表逐条相等', () => {
    // 变异对照：在 TrayMenu 里加一个 `const [tab, setTab] = useState<'a'|'b'>('a')` → 本条转红。
    const actual = STATES.map((s) => `${s.file}::${s.name}`).sort();
    const registered = REGISTRY.map((r) => `${r.file}::${r.name}`).sort();
    expect(actual, '浮层里多了/少了联合 state —— 见 REGISTRY 头注的登记规则').toEqual(registered);
  });

  it('登记的 role 与磁盘上的类型形态一致（含 null 的不许登记成 view）', () => {
    const wrong = REGISTRY.filter((r) => {
      const st = STATES.find((s) => s.file === r.file && s.name === r.name);
      if (!st) return true;
      return r.role === 'view' ? st.nullable : !st.nullable;
    }).map((r) => `${r.file}::${r.name}(${r.role})`);
    expect(wrong, 'role 与 state 的实际类型对不上').toEqual([]);
  });

  it('登记的 reset 状态与磁盘现状一致（缺失被修好了 ⇒ 也转红，逼登记跟着改）', () => {
    // 变异对照（正向）：把 `setView('main');` 加进 TrayMenu 的 onFocus → 本条转红，
    // 提示「已经修好了，把 REGISTRY 里的 missing 改成 wired」。
    const mismatch = REGISTRY.filter((r) => {
      const st = STATES.find((s) => s.file === r.file && s.name === r.name);
      if (!st) return true;
      const file = FILES.find((f) => f.rel === r.file);
      const wired = !!file && hasResetLeg(file.src, st);
      return wired !== (r.reset === 'wired');
    }).map((r) => `${r.file}::${r.name} 登记=${r.reset}`);
    expect(mismatch, '登记的复位状态与磁盘现状不符 —— 改了实现就把登记一起改').toEqual([]);
  });

  it('托盘成功 show 桥也到达每一个已登记的复位，geometry 不恢复交互', () => {
    const tray = FILES.find((file) => file.rel === 'tray/TrayMenu.tsx')!;
    for (const row of REGISTRY.filter((row) => row.file === tray.rel && row.reset === 'wired')) {
      const state = STATES.find((state) => state.file === row.file && state.name === row.name)!;
      expect(hasShowResetLeg(tray.src, state), `${row.name} disconnected from successful show`).toBe(true);
    }
  });

  it('缺复位腿的视图态必须逐条写明理由，且不许诉诸「本仓先例」', () => {
    for (const r of REGISTRY.filter((x) => x.reset === 'missing')) {
      expect(r.note.length, `${r.file}::${r.name} 的理由太短`).toBeGreaterThan(30);
      expect(r.note, `${r.file}::${r.name} 的理由是自我循环`).not.toMatch(
        /本仓先例|既有惯例|历来如此/,
      );
      if (r.role === 'view') {
        expect(r.note, `${r.file}::${r.name} 是视图态缺复位，理由须以「待修」起头`).toMatch(/^待修/);
      }
    }
  });

  it('托盘至少有一条 focus 腿（复位腿要挂的地方还在）', () => {
    // 这条挡的是「onFocus 整个被删掉」——那时 hasResetLeg 恒假，上面几条会显得「一切照旧」。
    const tray = FILES.find((f) => f.rel === 'tray/TrayMenu.tsx')!;
    expect(focusCallbacks(parseFlow(tray.src)).length, '托盘的 window focus 监听没了').toBeGreaterThan(0);
  });
});
