/**
 * 移动端「节点」屏的门。射程 = 本屏的**屏级契约**，与相邻两道门不重叠：
 * `../mobile-entry.test.ts` 守入口链路（CSS 契约 A1 / 五个目的地 / 导航与路由同源），
 * `../../styles/css-feature-floor.test.ts` 守 CSS 特性下限。这里守的是内容契约本身。
 *
 * 权威判据：
 *  · `mobile-kit/component-specs/information-architecture.md` §2 前言 + §2.1（屏级契约本体）
 *  · `polaris-mobile-ia-adjudications-2026-09-04.md`（11 条裁定，已定不重开）
 *  · `polaris-mobile-ia-convergence-2026-09-04.md` §2.2（取消卡片/列表双视图）
 *
 * ── 手段：node 环境 + `react-dom/server` 真渲染 ──────────────────────────────
 * 本仓刻意不装 jsdom / testing-library（先例见 `SubInfoBar.progress.test.tsx` 头注），
 * 故呈现层被切成纯组件 `NodesScreenView`：门喂它 props、拿回真 HTML，再对着 DOM 断言。
 * 「组件源码里出现过 `<TsExitWarning/>` 这个名字」不是同一件事 —— 那种断言挡不住
 * 「它渲染在一个永不为真的分支里」。
 *
 * ── 每一组都先有自检 ────────────────────────────────────────────────────────
 * 纯否定式判据（「不许出现 X」）会被「什么都没发生」骗过：扫描器塌了、正则敲错一个字母、
 * 渲染抛异常被吞，都会让它们一路绿灯。故每组先断言取材面非空、量级合理，再断言正面等式。
 */
import { afterAll, describe, it, expect } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, relative, resolve as resolvePath } from 'node:path';
import type { ServerConfig, SubscriptionConfig } from '@/contracts/types';
import { nodeExplanationSections, NodesScreenView } from './NodesScreenView';
import meshRouteFixture from '@/contracts/mesh-route-report.fixture.json';
import { asMeshRouteReport } from '../MobileMeshRouteEvidence';
import { protocolLabel, transferSummary } from '@/components/screens/nodes/nodes-logic';
import {
  ADD_ACTIONS,
  BATCH_ACTIONS,
  BATCH_DELETE,
  BATCH_MOVE_TO_GROUP,
  MESH_JOIN_ACTIONS,
  MESH_JOIN_CHOICES,
  ROW_ACTIONS,
  SUB_MENU_ACTIONS,
  SUB_PENDING_FIELDS,
} from './absence-register';
import {
  buildAddItems,
  buildNodeMoreItems,
  buildRowItems,
  buildSubItems,
  buildSubSummary,
  speedTestBlockedReason,
} from './view-model';
import type { NodeRowVM, NodesScreenViewProps, SheetItem } from './view-model';
import { winners, winnersText } from '@/styles/css-cascade.test-support';
import { closeOracle, measure } from '@/styles/css-oracle.test-support';
import { inMobileShell } from '@/styles/mount.test-support';
import { mobileEditFormFor } from '../forms/form-store';

/** …/ui/src/ */
const SRC = resolvePath(fileURLToPath(new URL('.', import.meta.url)), '..', '..');
const MOBILE = join(SRC, 'mobile');

/**
 * `selector` 上每个语义分量的**胜出值**（共享解析器 `styles/css-cascade.test-support.ts`）。
 *
 * 迁移前这一组是 `new RegExp('\\.mn-lat\\.fast\\s*\\{[^}]*hsl\\(var\\(--ok\\)\\)').exec(css)` 这种写法，
 * 三个洞叠在一起（2026-09-05 实测三条都能溜过去）：
 *  · **非全局 exec ⇒ 首条命中即结论**：末尾追加 `.mn-lat.fast{color:hsl(var(--err))}`，浏览器里后者胜，门全绿；
 *  · **不剥注释**：把真规则改坏、在上方注释里留一段旧的正确规则，门被注释喂饱（SEAM-3）；
 *  · **`[^}]*` 只到第一个 `}`**：规则里一旦有嵌套或后续规则，射程当场截断。
 *
 * 换成胜出值之后三个洞一起堵上：收全部命中、按层叠决胜、保长度剥注释、同族属性折算。
 * `where` 默认只看无条件声明 —— 断点档由本文件 ⑰ 组另判（那一组本来就收全部容器块）。
 */
const cssWinners = (selector: string) => winners({ sel: selector, ctx: 'mobile' }).won;
const won = (selector: string) => winnersText({ sel: selector, ctx: 'mobile' });

/**
 * `selector` 上「有人从更窄处压你」的**选择器全集**（`file:line 选择器` 去重排序）。
 *
 * 🔴 **这个桶守得住什么、守不住什么，2026-09-05 第四轮把话说清楚**（R5-04 的成因）：
 *  · 守得住：**语法上严格更窄**的选择器（`.mn-lat.fast.x`、`.mn-row .mn-lat.fast`、
 *    `:is(.mn-row,.zz) .mn-lat.fast`）对本选择器的覆写。R5-02 起连「更窄规则**新增**一个
 *    本选择器没写过的属性」也报。
 *  · **守不住**：`.mn-row .mn-lat` 这种**与本选择器互不包含**、却命中同一元素的规则 ——
 *    它既不更窄也不更宽，进不了任何桶，`toEqual([])` 照样绿。上一轮的 ledger 把
 *    「narrower 桶已钉成 `[]`」当成了「那条变异现在会红」，两件事不是一件事：断言写了，
 *    但它接不住那条变异（实测 M01：追加 `.mn-row .mn-lat{color:hsl(var(--err))}`，本组全绿）。
 * 那一族的权威判据在下面 ⑱ 组（浏览器裁判）。本组这几条只钉「更窄覆写清单没变」。
 */
const narrowerOf = (selector: string): string[] =>
  [
    ...new Set(
      winners({ sel: selector, ctx: 'mobile' }).narrower.map((c) => c.decl.rawSels.join(', ')),
    ),
  ].sort();
const DESKTOP_NODES = join(SRC, 'components', 'screens', 'nodes');
const read = (p: string): string => readFileSync(p, 'utf8');
const rel = (p: string): string => relative(SRC, p).split('\\').join('/');

/**
 * 剥注释。**必须识别字符串**：本目录的头注里满是 `data-tip` / `data-exit-write` 这类反面示例与
 * 引文，不剥就会把文档说明当成真实代码抓进取材面，门于是对着自己的注释报红。
 * 同一个坑 `mobile-entry.test.ts` 踩过一次（`vite.config.ts` 里的 glob 同时含 `*​/` 与 `/​*`），
 * 那里的教训是：正则版剥法会把 glob 当块注释起点一路吃到文件末尾。
 * **保长度**：剥掉的字符换成空格、换行原样保留 ⇒ 偏移量与原文 1:1。
 */
function strip(src: string): string {
  let out = '';
  let i = 0;
  const blank = (s: string): string => s.replace(/[^\n]/g, ' ');
  while (i < src.length) {
    const c = src[i];
    const n = src[i + 1];
    if (c === '/' && n === '*') {
      const end = src.indexOf('*/', i + 2);
      const stop = end === -1 ? src.length : end + 2;
      out += blank(src.slice(i, stop));
      i = stop;
    } else if (c === '/' && n === '/') {
      const end = src.indexOf('\n', i);
      const stop = end === -1 ? src.length : end;
      out += blank(src.slice(i, stop));
      i = stop;
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

/** 递归收集目录下的源码文件。**走目录树而不是写死清单**：新加一个屏文件自动进面。 */
function walk(dir: string, ext: RegExp): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...walk(p, ext));
    else if (ext.test(name)) out.push(p);
  }
  return out;
}

/**
 * 取出源码里全部 **JSX 开标签**（含自闭合）及其文本。
 * 只认标签名首字母**小写**的那些 —— 那是真正会被手指点到的 DOM 元素；首字母大写的是组件，
 * 属性只是往下传的管道。标记该贴在被点的那颗上，不该贴在管道上。
 */
function domOpeningTags(src: string): { name: string; text: string; index: number }[] {
  const body = strip(src);
  const out: { name: string; text: string; index: number }[] = [];
  const NAME = /<([a-z][a-zA-Z0-9-]*)(?=[\s/>])/g;
  for (const m of body.matchAll(NAME)) {
    const start = m.index;
    let i = start + m[0].length;
    let depth = 0;
    while (i < body.length) {
      const c = body[i];
      if (c === '{') depth += 1;
      else if (c === '}') depth -= 1;
      else if (c === '>' && depth === 0) break;
      i += 1;
    }
    out.push({ name: m[1], text: body.slice(start, i + 1), index: start });
  }
  return out;
}

// ─────────────────────────────────────────────────────────────────────────────
// 渲染夹具
// ─────────────────────────────────────────────────────────────────────────────

/** `t` 桩：把插值原样拼回 key 后面，这样「计数没传进文案」这种变异也抓得到。 */
const t: NodesScreenViewProps['t'] = (key, vars) =>
  vars ? `${key}(${Object.entries(vars).map(([k, v]) => `${k}=${String(v)}`).join(',')})` : key;

const server = (id: string, over: Partial<ServerConfig> = {}): ServerConfig => ({
  id,
  name: `node-${id}`,
  protocol: 'vless' as ServerConfig['protocol'],
  address: '198.51.100.7',
  port: 443,
  ...over,
});

const row = (id: string, over: Partial<NodeRowVM> = {}): NodeRowVM => ({
  server: server(id),
  isCurrent: false,
  isExit: false,
  lanOnly: false,
  latencyMs: 42,
  latencyStale: false,
  speedTestable: true,
  stagedOnly: false,
  deletable: true,
  transport: 'reality · tcp',
  protocolLabel: 'VLESS',
  ...over,
});
const meshReport = asMeshRouteReport(meshRouteFixture);
if (meshReport === null) throw new Error('mesh report fixture is invalid');

const baseProps = (over: Partial<NodesScreenViewProps> = {}): NodesScreenViewProps => ({
  t,
  groups: [
    { id: 'manual', label: 'nodes.tab.manual', count: 2 },
    { id: 'mesh', label: 'nodes.tab.mesh', count: 1 },
  ],
  activeTab: 'manual',
  onSelectTab: () => {},
  search: '',
  onSearch: () => {},
  protoFilter: '',
  protoOptions: ['vless', 'trojan'],
  onProtoFilter: () => {},
  sortKey: 'default',
  onSortKey: () => {},
  rows: [row('a'), row('b', { isCurrent: true })],
  emptyKind: 'all',
  countSummary: 'nodes.stats(total=2)',
  tsExitWarning: 'none',
  onTsExitAction: () => {},
  meshTunnelHealth: 'none',
  coreRunning: true,
  testing: false,
  copyBusy: false,
  refreshBusy: false,
  testableTotal: 2,
  onTestAll: () => {},
  onTestVisible: () => {},
  addItems: [{ id: 'manual-add', label: 'nodes.manualAdd', onSelect: () => {} }],
  subItems: [{ id: 'rename', label: 'nodes.subRename', onSelect: () => {} }],
  onUseAsExit: () => {},
  rowItems: () => [],
  batchMode: false,
  onToggleBatchMode: () => {},
  selectedIds: new Set<string>(),
  onToggleSelect: () => {},
  onSelectAll: () => {},
  onBatchCopyLinks: () => {},
  onBatchSpeedTest: () => {},
  onBatchDelete: () => {},
  ...over,
});

const render = (over: Partial<NodesScreenViewProps> = {}): string =>
  renderToStaticMarkup(<NodesScreenView {...baseProps(over)} />);

it('Tailscale 行保留协议与状态，空传输摘要不留下占位 span', () => {
  const ts = server('ts-summary', { protocol: 'tailscale' });
  const html = render({ rows: [row(ts.id, {
    server: ts, protocolLabel: protocolLabel(ts.protocol), transport: transferSummary(ts), isCurrent: true,
  })] });
  expect(html).toContain('Tailscale');
  expect(html).toContain('aria-current="true"');
  expect(html).toContain('nodes.selectedChoice');
  expect(html).not.toContain('mn-xfer');
  expect(html).not.toContain('mesh · wg');
});

const MOBILE_TSX = walk(MOBILE, /\.tsx$/);
const MOBILE_SRC = walk(MOBILE, /\.(?:ts|tsx)$/).filter((p) => !/\.test\.tsx?$/.test(p));
const MOBILE_TEXT = MOBILE_SRC.map((p) => strip(read(p))).join('\n');

// ─────────────────────────────────────────────────────────────────────────────

describe('⓪ 自检：判据的取材面是活的', () => {
  it('剥注释挡住注释里的反面示例，且不把字符串当注释', () => {
    expect(domOpeningTags(`/* <button data-exit-write="x"> */\n<i />`).map((x) => x.name)).toEqual(['i']);
    expect(domOpeningTags(`const g = ['**/x/**'];\n<i />`).map((x) => x.name)).toEqual(['i']);
  });

  it('目录走查扫到了屏的源码（扫 0 个文件会让下面每条恒绿）', () => {
    expect(MOBILE_TSX.map(rel)).toContain('mobile/nodes/NodesScreenView.tsx');
    expect(MOBILE_TSX.length).toBeGreaterThan(3);
    expect(MOBILE_TEXT.length).toBeGreaterThan(5000);
  });

  it('屏真的渲染得出来（渲染抛异常被吞会让 DOM 类断言全部对着空串恒绿）', () => {
    const html = render();
    expect(html.length).toBeGreaterThan(800);
    expect(html).toContain('data-exit-scope="mobile-nodes"');
  });
});

describe('① IA §2.1 内容序：五个块按重要性序出现，且卡片/列表双视图不移植', () => {
  const html = render({ sub: { name: 'sub-1', nodeCount: 3, autoLabel: 'auto', autoHint: 'hint' } });

  it('DOM 序 = 分组段 → 订阅摘要 → 工具栏 → 列表（顺序即重要性序，不许被断点重排）', () => {
    const order = ['mn-segs', 'mn-sub', 'mn-toolbar', 'mn-list'];
    const at = order.map((cls) => html.indexOf(cls));
    expect(at.every((i) => i >= 0), `缺块：${order.filter((_, i) => at[i] < 0).join(',')}`).toBe(true);
    expect([...at].sort((x, y) => x - y)).toEqual(at);
  });

  it('页头与分组段同处一块 sticky（`.mn-top`），页头在段之前', () => {
    expect(html.indexOf('mn-top')).toBeGreaterThanOrEqual(0);
    expect(html.indexOf('mn-head')).toBeLessThan(html.indexOf('mn-segs'));
  });

  it('收敛 §2.2：全屏零「卡片/列表」视图切换控件（源码面 + 渲染面双取）', () => {
    for (const banned of ['viewMode', 'cardView', 'listView', 'seg2', 'node-grid']) {
      expect(MOBILE_TEXT, `移动端出现了视图切换的痕迹「${banned}」`).not.toContain(banned);
      expect(html).not.toContain(banned);
    }
    // 正面：只有一种连续分隔行列表；宽屏可按 DOM 顺序逐行分两列，无视图切换控件。
    expect(read(join(MOBILE, 'nodes', 'nodes.css'))).toContain('grid-template-columns: repeat(2, minmax(0, 1fr))');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ② IA 裁定 #3 —— `TsExitWarning` 的不变量。本文件里最重的一条。
 *
 * 移植的是「**紧贴每一个**能产生该状态的控件」，不是坐标。故门有两条正交的腿：
 *   A 源码腿：枚举**全部**写出口的控件，逐个断言带标记 —— 挡「新增一个入口而没配警示」；
 *   B 渲染腿：断言每个带标记的控件与警示同处一个 scope —— 挡「标记有了但警示渲染在别处」。
 * 只钉「本屏上有一处警示」守不住第二个入口，那正是这类静默直连泄漏活下来的方式。
 * ═════════════════════════════════════════════════════════════════════════ */

/** 「这颗控件会改写出口节点」的记号。整个移动端树上只认这几个名字，新造一个也要登记进来。 */
const EXIT_WRITE_IDENTS = ['onUseAsExit', 'useSwitchNode', 'switchServer', 'switchNode'];

/** 带标记的控件所在文件的登记表。渲染腿只覆盖登记过的屏，故未登记 = A 腿红。 */
const EXIT_WRITER_FILES = [
  /* 首页的节点选择器同样写 exit_node（F1，2026-09-04 登记）。裁定 #3 的不变量是「紧贴**每一个**
     能产生该状态的控件」——首页那处的同屏警示由 `mobile/home/home-screen.test.tsx` ② 断言。 */
  'mobile/home/HomeNodePickerList.tsx',
  'mobile/nodes/NodesScreenView.tsx',
];

describe('② TsExitWarning 不变量（IA 裁定 #3）', () => {
  /** 全树上「绑了出口写入回调」的 DOM 控件。 */
  const writers = MOBILE_TSX.flatMap((p) =>
    domOpeningTags(read(p))
      .filter((tag) => /\bon[A-Z][A-Za-z]*=/.test(tag.text))
      .filter((tag) => EXIT_WRITE_IDENTS.some((id) => new RegExp(`\\b${id}\\b`).test(tag.text)))
      .map((tag) => ({ file: rel(p), tag })),
  );

  it('自检：取材面非空（扫不到任何出口控件 ⇒ 下面两条恒绿，那才是真危险）', () => {
    expect(writers.length, '整个 ui/src/mobile 树上一颗写出口的控件都没扫到 —— 判据面塌了').toBeGreaterThan(
      0,
    );
    expect(writers.map((w) => w.file)).toContain('mobile/nodes/NodesScreenView.tsx');
  });

  it('A 腿：每一颗能写 exit_node 的控件都带 `data-exit-write`（新增一个入口而不带 ⇒ 红）', () => {
    const unmarked = writers.filter((w) => !w.tag.text.includes('data-exit-write'));
    expect(
      unmarked.map((w) => `${w.file}: <${w.tag.name} …>`),
      '出现了能写 exit_node 但没有 `data-exit-write` 标记的控件 —— ' +
        'IA 裁定 #3 要求「一处控件配一处警示」，不带标记的控件逃出下面那条同屏断言',
    ).toEqual([]);
  });

  it('A 腿的取材面边界成立：移动端不从桌面屏引入任何 `.tsx`（否则控件能藏在门看不见的地方）', () => {
    /**
     * 三个例外，都是共享的纯呈现原语，且都不含任何控件（前两个是外壳在用的，第三个见表内注释）：
     * `Icons` 是一组纯 SVG，`ErrorBoundary` 是渲染兜底。两者的模块图里都没有 CSS
     * （`../mobile-entry.test.ts` 的契约 A1 集合恰等断言会兜住这一点），
     * 也都不含 `on*` 事件绑定的业务控件。放行的是这两个具体文件，不是「`@/components` 整个目录」。
     */
    // 2026-09-25 第三个：`screens/rules/MatchDot`（网络场景命中态圆点，N4）。一个带 aria-label 的
    // `<span>`，零控件、零事件、零 CSS import（类名来自 `domain/network-profile`，两端样式表各自上色）；
    // 故意单独成文件，就是为了不把 `NetworkProfilePanel.tsx` 的桌面弹窗外壳带进移动端。
    const SHARED_PRIMITIVES = [
      '@/components/Icons',
      '@/components/ErrorBoundary',
      '@/components/screens/rules/MatchDot',
      '@/components/brand-icons',
    ];
    /**
     * 从桌面 `.tsx` 里**具名取纯函数**的豁免（合并态 2026-09-04 新增）。
     *
     * 本条断言的真实风险有两个：① 引进来的东西会**渲染**（控件因此藏到 A 腿取材面之外）；
     * ② 它的视觉住在桌面层叠链上（契约 A1）。而 `.tsx` 这个扩展名只是这两件事的**代理**——
     * 一个 `.tsx` 文件同样可以导出与渲染无关的纯函数，此时两个风险一个都不成立。
     *
     * 所以豁免按 **(模块, 符号)** 成对登记，而不是按文件放行整个模块：
     * 同一个文件里将来导出一个真组件时，它不在表里，照样红。
     * 另有下方一条断言钉住「这些符号在移动端树上从不以 JSX 形式出现」——
     * 那才是「它不是组件」这句话的可执行判据，不是靠这段注释保证的。
     */
    const HELPER_IMPORTS: ReadonlyArray<readonly [string, string]> = [
      ['@/components/hover-cards/AppRuleHoverCard', 'appPresetLabel'],
      ['@/components/hover-cards/ResourceRefsHoverCard', 'resourceRefs'],
      // F5（连接屏）：`pageWindow` 是一个把「请求页」夹到合法区间的纯函数，`ListPager.tsx`
      // 里与它同住的 `ListPager` 组件**没有**登记 ⇒ 谁哪天顺手把组件也 import 进来，本条照旧红。
      ['@/components/ListPager', 'pageWindow'],
    ];
    const offenders: string[] = [];
    for (const p of MOBILE_SRC) {
      // 整条 import 语句一起抓（不能只抓 `from '…'` 那半截 —— 具名清单在它前面）。
      for (const m of strip(read(p)).matchAll(
        /import\s+(?:\{([^}]*)\}\s+)?[^;]*?from\s*'(@\/components\/[^']+)'/g,
      )) {
        const spec = m[2];
        if (SHARED_PRIMITIVES.includes(spec)) continue;
        // 具名纯函数豁免：本条 import 的符号必须**全部**在 (模块, 符号) 表里登记过。
        // 只要有一个没登记（例如顺手多带了一个组件），本模块就不豁免，照旧计入违规。
        const named = m[1] === undefined ? null : ([m[0], m[1]] as const);
        if (named) {
          const syms = named[1]
            .split(',')
            .map((x) => x.trim().split(/\s+as\s+/)[0].trim())
            .filter(Boolean);
          const allowed = HELPER_IMPORTS.filter(([mod]) => mod === spec).map(([, sym]) => sym);
          if (syms.length > 0 && syms.every((sym) => allowed.includes(sym))) continue;
        }
        const base = join(SRC, spec.slice('@/'.length));
        // `.ts` = 纯逻辑/hook（可复用）；`.tsx` = 桌面屏组件（视觉全落在桌面层叠链上，
        // 且它内部的控件在本门的源码取材面之外 —— A 腿会漏掉藏在那里的出口入口）。
        let resolved = '';
        for (const cand of [`${base}.ts`, `${base}.tsx`, `${base}/index.ts`, `${base}/index.tsx`]) {
          try {
            if (statSync(cand).isFile()) {
              resolved = cand;
              break;
            }
          } catch {
            /* 下一个候选 */
          }
        }
        if (resolved.endsWith('.tsx')) offenders.push(`${rel(p)} → ${spec}`);
      }
    }
    expect(
      offenders,
      '移动端引入了桌面屏组件（.tsx）—— 它的视觉住在桌面层叠链上，且会把可能写出口的控件' +
        '藏到 A 腿的取材面之外',
    ).toEqual([]);
    // 自检两条：扫描真的在看东西，且豁免表不是僵尸（豁免了一个已经没人引的文件 = 白名单在腐烂）。
    expect(MOBILE_TEXT).toContain("from '@/components/screens/");
    for (const allowed of SHARED_PRIMITIVES) {
      expect(MOBILE_TEXT, `豁免表里的 ${allowed} 已经没人引了 —— 该把它从表里删掉`).toContain(
        `'${allowed}'`,
      );
    }
    // 具名纯函数豁免的两条自检：登记项不是僵尸，且**它确实不是组件**。
    // 后一条是这条豁免成立的全部理由：只要有人把它当 JSX 用，风险①（控件藏在取材面外）
    // 立刻回来，豁免当场作废。
    for (const [mod, sym] of HELPER_IMPORTS) {
      expect(MOBILE_TEXT, `具名豁免 ${mod}#${sym} 已经没人引了 —— 该把它从表里删掉`).toContain(sym);
      expect(
        new RegExp(`<${sym}[\\s/>]`).test(MOBILE_TEXT),
        `${sym} 在移动端被当成组件用了 —— 具名纯函数豁免的前提不成立，它会把控件藏到 A 腿取材面之外`,
      ).toBe(false);
    }
  });

  it('A 腿（跨屏义务）：带标记的控件只许出现在已登记的屏里（首页 F1 落地时必须来这里登记）', () => {
    expect([...new Set(writers.map((w) => w.file))].sort()).toEqual([...EXIT_WRITER_FILES].sort());
  });

  it('B 腿：警示态下，每个 `data-exit-write` 都与一条 `data-ts-exit-warning` 同处一个 scope', () => {
    const html = render({ tsExitWarning: 'no-exit-device' });
    const scopes = html.split('data-exit-scope=').slice(1);
    expect(scopes.length, '渲染出的 DOM 里没有 exit scope —— 判据面塌了').toBeGreaterThan(0);
    for (const scope of scopes) {
      const marks = (scope.match(/data-exit-write=/g) ?? []).length;
      expect(marks, '这个 scope 里一颗出口控件都没有 —— 夹具没渲染出行？').toBeGreaterThan(0);
      expect(
        scope,
        `一个含 ${marks} 颗出口控件的 scope 里没有 TsExitWarning —— ` +
          'TS 当出口未配 exit_node 时公网会静默走直连而无任何提示（safety）',
      ).toContain('data-ts-exit-warning');
    }
  });

  it('B 腿的四档都渲染得出（少一档 = 那一档的用户什么也看不到）', () => {
    for (const w of ['needs-auth', 'no-exit-device', 'exit-device-offline', 'exit-device-not-advertised'] as const) {
      const html = render({ tsExitWarning: w });
      expect(html, `${w} 档没渲染出警示`).toContain(`data-ts-exit-warning="${w}"`);
      expect(html, `${w} 档没渲染出文案`).toContain('home.tsExit');
    }
  });

  it('反向对照：`none` 档不渲染警示（证明 B 腿不是恒真）', () => {
    const html = render({ tsExitWarning: 'none' });
    expect(html).not.toContain('data-ts-exit-warning');
    expect(html, '反向对照本身要有信息量：这一轮仍然渲染出了出口控件').toContain('data-exit-write');
  });
});

describe('③ MeshTunnelHealth 落组网分组（IA 裁定 #3 后半）', () => {
  it('五档各自渲染得出，且用的是与 TsExitWarning **不同**的图标（两句话不是一件事）', () => {
    for (const h of ['ts-expired', 'ts-not-running', 'ts-peers-offline', 'vpn-error', 'vpn-disconnected'] as const) {
      const html = render({ meshTunnelHealth: h });
      expect(html, `${h} 档没渲染`).toContain(`data-mesh-tunnel-health="${h}"`);
      expect(html).toContain('home.meshTunnel');
    }
    const both = render({ meshTunnelHealth: 'vpn-error', tsExitWarning: 'no-exit-device' });
    // 两条注脚必须能同时在场：上面那条说「公网不经这条隧道」，本条说「这条隧道本身通不通」。
    expect(both).toContain('data-ts-exit-warning');
    expect(both).toContain('data-mesh-tunnel-health');
    expect(both).toContain('<circle');
    expect(both).toContain('M12 3.2L21 19H3z');
  });

  it('`none` / `unprobeable` 不渲染（后者是核跑着就恒亮的静态事实，常驻横幅是纯噪声）', () => {
    for (const h of ['none', 'unprobeable'] as const) {
      expect(render({ meshTunnelHealth: h })).not.toContain('data-mesh-tunnel-health');
    }
  });
});

describe('④ IA §4.10：核停时测速**在场且置灰且有理由**，不是消失', () => {
  it('页头只保留新增/更多；更多菜单保留全量测速的实际回调', () => {
    const html = render({ coreRunning: true });
    expect(html.match(/class="mn-act/g)?.length).toBeGreaterThan(1);
    expect(html).toContain('nodes.mobileMoreActions');
    const calls: string[] = [];
    const items = buildNodeMoreItems({
      t, batchMode: false, testing: false,
      onTestAll: () => calls.push('all'), onTestVisible: () => calls.push('visible'),
      onToggleBatchMode: () => calls.push('batch'),
    });
    expect(items.map((item) => item.id)).toEqual(['test-all', 'batch', 'test-visible']);
    expect(items[0].disabledReason).toBeUndefined();
    items[0].onSelect?.();
    expect(calls).toEqual(['all']);
  });

  it('核停了 ⇒ 菜单测速项仍在且各自带原因，范围说明紧贴当前可见项', () => {
    const reason = speedTestBlockedReason(t, false);
    const items = buildNodeMoreItems({
      t, batchMode: false, testing: false,
      testAllBlocked: reason, testVisibleBlocked: reason,
      onTestAll: () => {}, onTestVisible: () => {}, onToggleBatchMode: () => {},
    });
    expect(items[0].disabledReason).toBe('home.stubProxyStopped');
    expect(items[2].disabledReason).toBe('home.stubProxyStopped');
    expect(items[2].note).toBe('nodes.testVisibleHint');
    expect(items[1].disabledReason).toBeUndefined();
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑤ 批量条（§3.1 第 17 条 batch-action-bar + §3.3 第 2 条）。
 *
 * 上一版这里只钉了「移动到分组不渲染」，而**登记表本身**当时写着「批量条里唯一一条不照搬桌面的
 * 处置」—— 那句自述是错的：「删除」也没画，且既没登记也没置灰，正是 §3.3-2 要禁的那种
 * 「悄无声息地没了」。一句写在注释里的「其余逐条照搬」对实现零强制力，所以判据换成**三方对差**：
 *   桌面 `.batch-bar` 的 `<button` 数  ==  登记表条目数
 *   登记表里 `ported` 的条数           ==  这条条上真渲染出来的按钮数
 * 桌面加一颗、登记表漏一颗、移动端少画一颗，三种形态各自当场红。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑤ 批量条：桌面按钮 ↔ 登记表 ↔ 真渲染，三方对差', () => {
  /**
   * 桌面取材面 = `NodesBatchBar.tsx` 里 `.batch-bar` 起至文件末。
   * **不做 `<div>` 配平**：这条条里有一颗自闭合的 `<div className="sp" />`，配平写法会在它上面
   * 跑偏（同一个坑 ⑥ 的 `.nd-acts` 踩过一次，教训是切片型判据要有自检输出）。
   * 改用两条自检把「整段 = 这条条」钉住：段前没有任何按钮，且全文件只有一条 `.batch-bar`。
   */
  const batchBarSrc = (() => {
    const body = strip(read(join(DESKTOP_NODES, 'NodesBatchBar.tsx')));
    const start = body.indexOf('className="batch-bar"');
    expect(start, '桌面 `.batch-bar` 找不到了 —— 判据面塌了（改名了？）').toBeGreaterThan(-1);
    expect(
      body.slice(0, start),
      '`.batch-bar` 之前出现了 `<button` —— 取材面不再等于这条条，计数会偏高',
    ).not.toContain('<button');
    expect(body.indexOf('className="batch-bar"', start + 1), '文件里出现了第二条 `.batch-bar`').toBe(-1);
    return body.slice(start);
  })();

  const desktopButtons = (batchBarSrc.match(/<button/g) ?? []).length;

  /** 批选态下真渲染出来的那条条（`.mn-batch` 里没有嵌套 `div`，切到第一个 `</div>` 即整条）。 */
  const renderedBar = (() => {
    const html = render({ batchMode: true, selectedIds: new Set(['a']) });
    const s = html.indexOf('class="mn-batch"');
    expect(s, '批选条本身没渲染出来 ⇒ 下面每条计数都对着空串恒绿').toBeGreaterThan(-1);
    const e = html.indexOf('</div>', s);
    expect(e, '`.mn-batch` 没有闭合标签 —— 切片塌了').toBeGreaterThan(s);
    return html.slice(s, e);
  })();

  const renderedButtons = (renderedBar.match(/<button/g) ?? []).length;
  const ported = BATCH_ACTIONS.filter((a) => a.disposition.kind === 'ported');
  const missing = BATCH_ACTIONS.filter((a) => a.disposition.kind !== 'ported');

  it('自检：三个取材面都有量级（任一为 0 会让下面的等式恒绿）', () => {
    expect(batchBarSrc.length).toBeGreaterThan(500);
    expect(desktopButtons, `桌面 .batch-bar 只数到 ${desktopButtons} 颗按钮`).toBeGreaterThan(4);
    expect(renderedButtons, '移动端批量条一颗按钮都没渲染出来').toBeGreaterThan(2);
    expect(BATCH_ACTIONS.length).toBeGreaterThan(4);
  });

  it('桌面按钮数 == 登记表条目数（桌面加第七颗 ⇒ 红，逼一次显式处置决定）', () => {
    expect(
      BATCH_ACTIONS.length,
      `桌面 .batch-bar 有 ${desktopButtons} 颗按钮，登记表只列了 ${BATCH_ACTIONS.length} 条 —— ` +
        '差的那几颗就是「悄无声息地没了」的形态（§3.3 第 2 条）',
    ).toBe(desktopButtons);
  });

  it('登记表 `ported` 数 == 真渲染出来的按钮数（登记成移植却没画 ⇒ 红）', () => {
    expect(
      renderedButtons,
      `登记表说移植了 ${ported.length} 颗（${ported.map((a) => a.id).join(', ')}），` +
        `批量条上只画出 ${renderedButtons} 颗`,
    ).toBe(ported.length);
  });

  it('每条登记指向的桌面控件真的存在（防登记表编出一批不存在的按钮）', () => {
    for (const a of BATCH_ACTIONS)
      expect(batchBarSrc, `登记表里的 ${a.id} 在 .batch-bar 里找不到（desktopRef: ${a.desktopRef}）`).toContain(
        a.desktopRef,
      );
  });

  it('每条缺席都带一句已登记的理由（缺席是产品边界，缺席而不给理由才是缺陷）', () => {
    const zh = JSON.parse(read(join(SRC, 'i18n', 'locales', 'zh-CN.json'))) as Record<string, unknown>;
    const has = (key: string): boolean =>
      typeof key.split('.').reduce<unknown>((c, s) => (c as Record<string, unknown>)?.[s], zh) === 'string';
    expect(missing.length, '一条缺席都没有 ⇒ 本条恒真').toBeGreaterThan(0);
    for (const a of missing) {
      const reasonKey = a.disposition.kind === 'ported' ? '' : a.disposition.reasonKey;
      expect(reasonKey, `${a.id} 没给理由 key`).toBeTruthy();
      expect(has(reasonKey), `${a.id} 的理由 key ${reasonKey} 在 zh-CN 里不存在`).toBe(true);
    }
    /* 2026-09-06（批 2 / W-03）：「删除」从缺席变成 `ported`（腿接上了，见
       `node-deletion.ts`），条上只剩「移动到分组」一条缺席 —— 而那一条的归因是**数据模型**
       （桌面同一颗也永久灰着，`nodes-logic#moveToGroupTargets` 恒返 `[]`），不是移动交付缺口。
       钉住这个集合本身：下次谁把删除又拿掉、或多出一条新缺席，都要在这里显式改一次。 */
    expect(missing.map((a) => a.id).sort()).toEqual(['move-to-group']);
  });

  it('反向对照：两个数确实不同（缺席登记是承重的，不是一张恒等的表）', () => {
    expect(renderedButtons).not.toBe(desktopButtons);
    expect(desktopButtons - renderedButtons).toBe(missing.length);
  });

  it('IA §4.14：条上真的没有「移动到分组」那颗（自检 + 否定同批）', () => {
    expect(BATCH_MOVE_TO_GROUP.kind).toBe('absent');
    expect(BATCH_MOVE_TO_GROUP).toHaveProperty('reasonKey', 'nodes.batchMoveUnavailable');
    expect(renderedBar).toContain('nodes.selectAll');
    expect(renderedBar).not.toContain('nodes.batchMove');
    /* 正面对照（W-03）：删除**在**条上，且登记表说它是 `ported` —— 两处必须同时成立。
       只留上面那条否定会被「整条条没渲染出来」骗过（那时两个 `not.toContain` 都真）。 */
    expect(BATCH_DELETE.kind, '批量删除的登记码变了 —— 与条上画不画那颗必须同步').toBe('ported');
    expect(renderedBar, '登记成 ported 却没画出来 —— 批量删除又变成悄无声息的缺席了').toContain(
      'common.delete',
    );
  });
});

describe('⑥ 能力缺席登记表对得上桌面（登记表变僵尸 / 桌面新增控件而移动端漏判）', () => {
  const nodeCard = read(join(DESKTOP_NODES, 'NodeCard.tsx'));
  /**
   * `.nd-acts` 的取材面按 `<div>` / `</div>` 配平取。
   * 上一版用「切到下一个 `nd-check` 为止」，结果多数了一颗：那个字面量出现在**复选框按钮的
   * className 里**，位置在那颗 `<button` 之后 ⇒ 切点落在按钮之后，批选复选框被算成第七颗行动作。
   * 教训与本仓已登记的那条同类：**切片型判据要有自检输出**，「多一颗」正是靠下面那条计数断言
   * 当场暴露出来的。
   */
  const actsBlock = (() => {
    const body = strip(nodeCard);
    const start = body.indexOf('<div className="nd-acts">');
    expect(start, '桌面 NodeCard 的 `.nd-acts` 块找不到了 —— 判据面塌了（改名了？）').toBeGreaterThan(-1);
    let i = start;
    let depth = 0;
    while (i < body.length) {
      if (body.startsWith('<div', i)) depth += 1;
      else if (body.startsWith('</div>', i)) {
        depth -= 1;
        if (depth === 0) return body.slice(start, i + 6);
      }
      i += 1;
    }
    return body.slice(start);
  })();

  it('自检：桌面 `.nd-acts` 块取到了，且量级合理', () => {
    expect(actsBlock.length).toBeGreaterThan(500);
    expect(actsBlock).toContain('<button');
  });

  it('桌面行动作数 == 登记表条目数（桌面加第七颗 ⇒ 红，逼一次显式处置决定）', () => {
    const count = (actsBlock.match(/<button/g) ?? []).length;
    expect(count, `桌面 .nd-acts 有 ${count} 颗按钮，登记表只列了 ${ROW_ACTIONS.length} 条`).toBe(
      ROW_ACTIONS.length,
    );
  });

  it('每条登记指向的桌面控件真的存在（防登记表编出一批不存在的控件）', () => {
    for (const a of ROW_ACTIONS) {
      expect(nodeCard, `登记表里的 ${a.id} 在 NodeCard.tsx 里找不到（desktopRef: ${a.desktopRef}）`).toContain(
        a.desktopRef,
      );
    }
  });

  it('每条 absent / disabled 都带一句已登记的理由（置灰不给理由 = 没说话）', () => {
    const zh = JSON.parse(read(join(SRC, 'i18n', 'locales', 'zh-CN.json'))) as Record<string, unknown>;
    const has = (key: string): boolean =>
      typeof key.split('.').reduce<unknown>((c, s) => (c as Record<string, unknown>)?.[s], zh) === 'string';
    for (const a of ROW_ACTIONS) {
      if (a.disposition.kind === 'ported') continue;
      expect(a.disposition.reasonKey, `${a.id} 没给理由 key`).toBeTruthy();
      expect(has(a.disposition.reasonKey), `${a.id} 的理由 key 在 zh-CN 里不存在`).toBe(true);
    }
    expect(has(BATCH_MOVE_TO_GROUP.kind === 'ported' ? 'common.close' : BATCH_MOVE_TO_GROUP.reasonKey)).toBe(
      true,
    );
  });

  /**
   * 🔴 **这一条 2026-09-06 补牙（批 2）。**
   *
   * 上一版**自造夹具**：它把 `rowItems` 传成一个现编的数组，于是证明的只有「视图收到一条置灰项时
   * 画得出来」——**从不读生产的那张表**。生产忘了把某一条放进表里、或理由键与登记表对不上，
   * 它一律绿。现在改成调**生产的构造器** `view-model.ts#buildRowItems`，喂真实的登记表。
   *
   * ⚠️ 抽函数会造出新的缝（测了纯函数 ≠ 生产在用它）⇒ 下面 ⑥-b 那条**成对**判据对着
   * `MobileNodesScreen.tsx` 的源码断言那个 `useCallback` 真的在调它。两条缺任一条，缝就留着。
   */
  it('未移植的行动作**在动作面里在场且置灰**，不是凭空消失（§3.3 第 2 条）—— 读生产的表', () => {
    const target = row('a');
    const items = buildRowItems({
      t: (k: string) => k,
      row: target,
      coreRunning: true,
      disposition: (id: string) => ROW_ACTIONS.find((a) => a.id === id)?.disposition,
      handlers: {
        onSpeedTest: () => {},
        onConnect: () => {},
        onCopyLink: () => {},
        onClone: () => {},
        onEdit: () => {},
        onDelete: () => {},
      },
    });
    // 自检：构造器真的产出了东西（空数组会让下面每条恒真）。
    expect(items.length, '生产的行动作表是空的 ⇒ 下面每条恒真').toBeGreaterThan(3);

    /* ① 登记表里**每一条**非 ported 的处置，都必须在生产的表里在场、置灰、且理由键逐字相同。 */
    const missing = ROW_ACTIONS.filter((a) => a.disposition.kind !== 'ported');
    for (const a of missing) {
      const item = items.find((i: SheetItem) => i.id === a.id);
      expect(item, `登记表说 ${a.id} 缺席，而生产的行动作表里根本没有这一条 —— 那是「悄无声息地没了」`).toBeDefined();
      expect(
        item?.disabledReason,
        `${a.id} 在生产表里没有置灰理由（缺席而不给理由才是缺陷）`,
      ).toBe(a.disposition.kind === 'ported' ? undefined : a.disposition.reasonKey);
    }

    /* ② 登记成 `ported` 的三条必须**可点**（有 onSelect、无置灰理由）。
       只写 ① 会被「什么都没发生」骗过：把三条腿全拔掉、全部改成置灰，① 照样绿。 */
    for (const id of ['clone', 'edit', 'delete'] as const) {
      const d = ROW_ACTIONS.find((a) => a.id === id)?.disposition;
      if (d?.kind !== 'ported') continue;
      const item = items.find((i: SheetItem) => i.id === id);
      expect(item?.disabledReason, `${id} 登记成 ported 却在生产表里被置灰了`).toBeUndefined();
      expect(typeof item?.onSelect, `${id} 登记成 ported 却没有动作`).toBe('function');
    }

    /* ③ 「编辑得动吗」那一档 **2026-09-06（批 3）换了形态**：三张组网表落地之后
       `mobileEditFormFor` 不再返回 `null`，`editable` 那个入参随之摘掉。
       判据不放宽，改成一条**更强的正面断言**：拿真实节点逐个协议族跑分流函数，
       每一个都必须落到宿主真的有 case 的那一支上（分流本体是两端共用的 `nodeEditForm`）。
       ⚠️ 只写「编辑那一条可点」会被「分流函数返回一个宿主没有的 kind」骗过 —— 那时按钮亮着、
       点下去 `MobileFormHost` 的 `never` 兜底在运行期抛异常，而这条判据一样绿。 */
    const HOST_KINDS = new Set(
      [...strip(read(join(MOBILE, 'forms', 'MobileFormHost.tsx'))).matchAll(/case '([a-z-]+)':/g)].map(
        (m) => m[1],
      ),
    );
    expect(HOST_KINDS.size, '宿主一个 case 都没解析出来 ⇒ 下面恒真').toBeGreaterThan(5);
    const editTargets: ReadonlyArray<readonly [string, ServerConfig]> = [
      ['vless', server('a', { protocol: 'vless' })],
      ['openconnect', server('b', { protocol: 'openconnect' })],
      ['wireguard', server('c', { protocol: 'wireguard' })],
      ['WireGuard 大小写', server('d', { protocol: 'WireGuard' as ServerConfig['protocol'] })],
      ['tailscale', server('e', { protocol: 'tailscale' })],
      [
        'WARP（protocol 也是 wireguard）',
        server('f', {
          protocol: 'wireguard',
          wireguardSettings: {
            privateKey: 'k',
            localAddress: ['10.0.0.2/32'],
            peerPublicKey: 'p',
            warpDevice: { deviceId: 'd', token: 't' },
          },
        }),
      ],
    ];
    for (const [name, node] of editTargets) {
      const desc = mobileEditFormFor(node);
      expect(desc, `${name} 没有分流结果`).toBeDefined();
      expect(HOST_KINDS.has(desc.kind), `${name} 落到了宿主没有 case 的 kind: ${desc.kind}`).toBe(true);
    }
    // 正面对照：三个组网协议**不许**都落回通用节点表（那会丢 wireguardSettings / tailscaleSettings）。
    expect(mobileEditFormFor(editTargets[2]![1]).kind).toBe('wg');
    expect(mobileEditFormFor(editTargets[4]![1]).kind).toBe('ts-settings');
    expect(mobileEditFormFor(editTargets[5]![1]).kind).toBe('warp');
    // 反向对照：普通协议仍然落通用节点表（不是「一律当组网」）。
    expect(mobileEditFormFor(editTargets[0]![1]).kind).toBe('node');
    // 旧的那条理由键必须**从五份 locale 里消失**：它说的是「那三张表还没接」，本批之后是假的。
    for (const loc of ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa']) {
      expect(
        read(join(SRC, 'i18n', 'locales', `${loc}.json`)),
        `${loc} 里还留着 nodes.mobileMeshFormUnavailable —— 那句话现在是假的`,
      ).not.toContain('mobileMeshFormUnavailable');
    }

    /*
     * ③-b **合成处置**：`ROW_ACTIONS` 今天六条全是 `ported` ⇒ 上面 ① 那个循环是**空转**的，
     * 而 `buildRowItems` 里「登记成缺席 ⇒ 在场置灰 + 理由」那一支在今天的仓里**没有实例**。
     * 实测：把那一支整条删掉（`continue` 掉），本组仍然全绿 —— 那正是「门在但没牙」。
     * 故用一个合成的处置表把那一支驱动起来：下一条 `absent` 登记进来时它已经有牙了，
     * 而不是等到那天才发现这一支从来没被测过。
     */
    const synthetic = buildRowItems({
      t: (k: string) => k,
      row: target,
      coreRunning: true,
      disposition: (id: string) =>
        id === 'clone' ? { kind: 'absent', reasonKey: 'synthetic.reason' } : { kind: 'ported' },
      handlers: {
        onSpeedTest: () => {},
        onConnect: () => {},
        onCopyLink: () => {},
        onClone: () => {},
        onEdit: () => {},
        onDelete: () => {},
      },
    });
    const syntheticClone = synthetic.find((i: SheetItem) => i.id === 'clone');
    expect(
      syntheticClone,
      '登记成缺席的那一条在生产的表里**根本没有** —— 那是「悄无声息地没了」（§3.3 第 2 条）',
    ).toBeDefined();
    expect(syntheticClone?.disabledReason, '缺席的那一条在场但没给理由').toBe('synthetic.reason');
    expect(syntheticClone?.onSelect, '置灰了却还挂着动作').toBeUndefined();
    // 反向对照：同一份合成输入下，登记成 ported 的那两条仍然可点（不是「一律置灰」）。
    expect(synthetic.find((i: SheetItem) => i.id === 'edit')?.disabledReason).toBeUndefined();

    /* ⑤ 订阅托管的那一行**没有删除入口**（2026-09-06 复审 major，见 `buildRowItems` 里那段注释）。
       与上面 ①③ 那一档**不同形**：那些是「移动端没移植 ⇒ 在场置灰给理由」，这一条是
       「这一行上它本就不该存在 ⇒ 入口不渲染」（桌面 `NodeCard.tsx:476 {deletable && …}` 同口径）。
       此前 `buildRowItems` 从头到尾没读 `row.deletable`，delete 那一条只看 `ROW_ACTIONS` 的处置
       无条件入表 ⇒ 订阅分组里每一行都画得出一颗能点的「删除」，删完下次对账原样拉回来。 */
    const args = (over: Partial<Parameters<typeof buildRowItems>[0]>) =>
      buildRowItems({
        t: (k: string) => k,
        row: target,
        coreRunning: true,
        disposition: (id: string) => ROW_ACTIONS.find((a) => a.id === id)?.disposition,
        handlers: {
          onSpeedTest: () => {},
          onConnect: () => {},
          onCopyLink: () => {},
          onClone: () => {},
          onEdit: () => {},
          onDelete: () => {},
        },
        ...over,
      });
    const subRow = args({ row: row('s1', { deletable: false }) }).map((i: SheetItem) => i.id);
    expect(
      subRow,
      '订阅托管的节点上画出了「删除」—— 删了下次订阅刷新的 reconcile 会照原样拉回来：' +
        '操作没有净效果、只剩误删风险（陈先生 2026-07-29 裁定）',
    ).not.toContain('delete');
    // 正向对照：自建节点必须**有**这一条（否则上一条会被「删除干脆从不入表」骗过）。
    expect(
      args({ row: row('m1', { deletable: true }) }).map((i: SheetItem) => i.id),
      '自建节点上也没有「删除」了 —— 那不是修，是把功能删了',
    ).toContain('delete');
    // 射程自曝：摘掉的**只有**删除那一条，其余五条在两种行上逐条同集。
    expect(subRow).toEqual(
      args({ row: row('m1', { deletable: true }) })
        .map((i: SheetItem) => i.id)
        .filter((id: string) => id !== 'delete'),
    );
    /* 成对的接线腿：`deletable` 这一位由接线层算，构造器只消费它。构造器判对了而接线层恒传 true
       （或按别的东西算）时，上面三条一律绿 —— 那道缝与 ⑥-b 守的是同一类。 */
    expect(
      strip(read(join(MOBILE, 'nodes', 'MobileNodesScreen.tsx'))),
      '接线层不再按「有没有订阅归属」算 `deletable` —— 构造器那条判据随之失去输入',
    ).toContain('deletable: server.subscriptionId === undefined');

    /* ④ 渲染腿：把**生产的表**交给视图，置灰项真的画得出来（构造对了但画不出来 = 门在但没牙）。 */
    const html = renderToStaticMarkup(
      <NodesScreenView {...baseProps({ rowItems: () => items, addItems: items })} />,
    );
    expect(html).toContain('mn-row-more');
    for (const a of missing) expect(html).toContain(a.disposition.kind === 'ported' ? '' : a.disposition.reasonKey);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑥-b 「抽出来的构造器」与「生产在用它」是**两条判据**，必须成对交。
 * ═════════════════════════════════════════════════════════════════════════ */
describe('⑥-b 三张动作表：生产真的在调那三个构造器（抽函数造出来的新缝）', () => {
  const screenSrc = strip(read(join(MOBILE, 'nodes', 'MobileNodesScreen.tsx')));

  it('自检：接线层源码取到了，且量级合理（读空文件会让下面每条恒绿）', () => {
    expect(screenSrc.length).toBeGreaterThan(2000);
  });

  it('`addItems` / `subItems` / `rowItems` 三处都调了 `view-model` 的构造器', () => {
    for (const [prop, fn] of [
      ['addItems', 'buildAddItems('],
      ['subItems', 'buildSubItems('],
      ['rowItems', 'buildRowItems('],
    ] as const) {
      expect(
        screenSrc,
        `${prop} 没有调 ${fn} —— 构造器还在，但生产另写了一份，两份必然漂开`,
      ).toContain(fn);
    }
    // 反向对照：谓词认得出「没调」（否则上面三条可能只是 `toContain` 永远真）。
    expect(screenSrc).not.toContain('buildNothingItems(');
  });

  it('三处都真的把构造器的结果交给了视图（构造了却没传下去 = 门在但没牙）', () => {
    for (const prop of ['addItems={addItems}', 'subItems={subItems}', 'rowItems={rowItems}']) {
      expect(screenSrc, `视图没有收到 ${prop}`).toContain(prop);
    }
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑥-c 「添加四条」「订阅更多五条」「组网接入五个」——桌面菜单 ↔ 登记表 ↔ 生产表 三方对差。
 *
 * 🔴 本批之前这三块**一条都不在表里、也没有任何门**：桌面多一项、或移动端顺手删一项，
 *    都不会有任何东西红。形态与 ⑤ / ⑥ 完全一致（那两块正是因为有这道门才守得住）。
 * ═════════════════════════════════════════════════════════════════════════ */
describe('⑥-c 添加 / 订阅更多 / 组网接入：桌面 ↔ 登记表 ↔ 生产表', () => {
  /** 桌面「添加」菜单：`NodesHeader.tsx` 里那块 `.mini-menu` 的 `role="menuitem"` 数。 */
  const addMenuSrc = (() => {
    const body = strip(read(join(DESKTOP_NODES, 'NodesHeader.tsx')));
    const start = body.indexOf('className="mini-menu"');
    expect(start, '桌面「添加」菜单找不到了 —— 判据面塌了（改名了？）').toBeGreaterThan(-1);
    expect(body.indexOf('className="mini-menu"', start + 1), '文件里出现了第二块 mini-menu').toBe(-1);
    return body.slice(start);
  })();
  const desktopAddItems = (addMenuSrc.match(/role="menuitem"/g) ?? []).length;

  /** 桌面订阅「更多」菜单：`SubInfoBar.tsx` 里那块 `.mini-menu` 的 `role="menuitem"` 数。 */
  const subMenuSrc = (() => {
    const body = strip(read(join(DESKTOP_NODES, 'SubInfoBar.tsx')));
    const start = body.indexOf('className="mini-menu"');
    expect(start, '桌面订阅「更多」菜单找不到了 —— 判据面塌了').toBeGreaterThan(-1);
    expect(body.indexOf('className="mini-menu"', start + 1), '文件里出现了第二块 mini-menu').toBe(-1);
    return body.slice(start);
  })();
  const desktopSubItems = (subMenuSrc.match(/role="menuitem"/g) ?? []).length;

  /**
   * 桌面组网接入面的**逻辑**选择个数。
   *
   * 取材面不是「`<Choice` 标签有几个」—— 2026-09-12 桌面把 Tailscale 那一支改成按节点数分两支
   * （`tsNodes.length > 1 ? tsNodes.map(…<Choice…>) : <Choice…>`），于是同一个逻辑选择有了**两个
   * 渲染点**，而逻辑上仍是一个。直接数标签会让这条恒等式凭空多一条、逼出一条不存在的登记。
   *
   * 故减掉「按节点展开」那一支贡献的额外渲染点：数 `tsNodes.map(` 的个数，**不写死 1**，
   * 这样那一支哪天被删掉，减数会自己归零。下面另有一条自检钉住「那一支里确实只有一个 `<Choice`」——
   * 没有它，这个减法就可能悄悄减多。
   */
  const meshJoinSrc = strip(read(join(SRC, 'components', 'dialogs', 'MeshJoinDialog.tsx')));
  const meshChoiceTags = (meshJoinSrc.match(/<Choice\b/g) ?? []).length;
  const perNodeBranches = (meshJoinSrc.match(/tsNodes\.map\(/g) ?? []).length;
  // 已有 TS 账号的逐节点卡片与单账号卡片是设置入口；新增接入项由另一张卡承担。
  const singleNodeBranches = (meshJoinSrc.match(/singleTsNode \? \(/g) ?? []).length;
  const desktopMeshChoices = meshChoiceTags - perNodeBranches - singleNodeBranches;

  const noop = () => {};
  const prodAdd = buildAddItems((k: string) => k, {
    onManualAdd: noop,
    onMeshJoin: noop,
    onImport: noop,
    onAddSubscription: noop,
  });
  const prodSub = buildSubItems((k: string) => k, {
    onRename: noop,
    onEditUrl: noop,
    onCopyUrl: noop,
    onInterval: noop,
    onDelete: noop,
  });

  it('自检：按节点展开那一支只贡献一个额外渲染点（否则上面的减法会减多/减少）', () => {
    /* 逐字取出 `tsNodes.map(` 到它那个三元分支结束（`) : ` 那一跳）之间的片段，数里面的 `<Choice`。
       减数的正确性靠这条，不靠「我记得那里只有一个」。 */
    const i = meshJoinSrc.indexOf('tsNodes.map(');
    if (perNodeBranches === 0) {
      expect(i, '没有 `tsNodes.map(` 却数出了按节点展开的支').toBe(-1);
      return;
    }
    const j = meshJoinSrc.indexOf(') : ', i);
    expect(j, '按节点展开那一支不是三元的真支了 —— 减法的前提变了').toBeGreaterThan(i);
    expect(
      (meshJoinSrc.slice(i, j).match(/<Choice\b/g) ?? []).length,
      '按节点展开那一支里的 `<Choice` 不是一个 —— 上面的减法会算错',
    ).toBe(1);
  });

  it('自检：三个桌面取材面都有量级（任一为 0 会让下面的等式恒绿）', () => {
    expect(desktopAddItems, '桌面「添加」菜单一项都没数到').toBeGreaterThan(2);
    expect(desktopSubItems, '桌面订阅「更多」菜单一项都没数到').toBeGreaterThan(3);
    expect(desktopMeshChoices, '桌面组网接入面一个选择都没数到').toBeGreaterThan(3);
  });

  it('桌面菜单项数 == 登记表条数（桌面加一项 ⇒ 红，逼一次显式处置决定）', () => {
    expect(ADD_ACTIONS.length, `桌面「添加」有 ${desktopAddItems} 项`).toBe(desktopAddItems);
    expect(SUB_MENU_ACTIONS.length, `桌面订阅「更多」有 ${desktopSubItems} 项`).toBe(desktopSubItems);
    expect(MESH_JOIN_CHOICES.length, `桌面组网接入有 ${desktopMeshChoices} 个选择`).toBe(
      desktopMeshChoices,
    );
  });

  it('每条登记指向的桌面控件真的存在（防登记表编出一批不存在的菜单项）', () => {
    for (const a of ADD_ACTIONS)
      expect(addMenuSrc, `${a.id} 在 NodesHeader 的菜单里找不到（${a.desktopRef}）`).toContain(a.desktopRef);
    for (const a of SUB_MENU_ACTIONS)
      expect(subMenuSrc, `${a.id} 在 SubInfoBar 的菜单里找不到（${a.desktopRef}）`).toContain(a.desktopRef);
    for (const c of MESH_JOIN_CHOICES)
      expect(meshJoinSrc, `${c.id} 在 MeshJoinDialog 里找不到（${c.desktopRef}）`).toContain(c.desktopRef);
  });

  it('生产表的 id 与登记表**逐条同序**（移动端顺手删一项 ⇒ 红）', () => {
    expect(prodAdd.map((i: SheetItem) => i.id)).toEqual(ADD_ACTIONS.map((a) => a.id));
    expect(prodSub.map((i: SheetItem) => i.id)).toEqual(SUB_MENU_ACTIONS.map((a) => a.id));
  });

  it('登记成 ported 的每一条在生产表里都**可点**（正面断言：不许只有「没出现 X」）', () => {
    for (const a of ADD_ACTIONS) {
      if (a.disposition.kind !== 'ported') continue;
      const item = prodAdd.find((i: SheetItem) => i.id === a.id);
      expect(item?.disabledReason, `${a.id} 登记成 ported 却是灰的`).toBeUndefined();
      expect(typeof item?.onSelect, `${a.id} 登记成 ported 却没有动作`).toBe('function');
    }
    for (const a of SUB_MENU_ACTIONS) {
      if (a.disposition.kind !== 'ported') continue;
      const item = prodSub.find((i: SheetItem) => i.id === a.id);
      expect(item?.disabledReason, `${a.id} 登记成 ported 却是灰的`).toBeUndefined();
      expect(typeof item?.onSelect, `${a.id} 登记成 ported 却没有动作`).toBe('function');
    }
  });

  it('反向对照：谓词认得出「被置灰」（否则上一条可能恒真）', () => {
    const greyed = buildAddItems((k: string) => k, {
      onManualAdd: noop,
      onMeshJoin: noop,
      onImport: noop,
      onAddSubscription: noop,
    }).map((i: SheetItem) => ({ ...i, disabledReason: 'x' }));
    expect(greyed.every((i: SheetItem) => i.disabledReason !== undefined)).toBe(true);
  });

  /**
   * 🔴 **2026-09-06（批 3）改判据**：五个选择全部接通之后，上一版那条
   * `expect(absent.length).toBeGreaterThan(0)` 会红 —— 那是「本组只在有缺席时才有话说」的写法。
   * 判据**不放宽**，改成两条并列：
   *  ① 缺席那一档的循环保留（今天空转），并**另用合成条目证明它认得出缺席**（正向对照）；
   *  ② 新增一条**更强的正面等式**：登记成 `ported` 的每一个选择，都必须在生产的面板里
   *     真的映射到一个宿主有 case 的表单 kind —— 「接通了」这句话第一次有了可执行的落点。
   *     上一版只断言「面板源码里出现过 openconnect 这个词」，那连注释都能满足。
   */
  it('组网接入面：缺席带已登记的理由；ported 的每一条都真的映射到一张宿主认得的表', () => {
    const zh = JSON.parse(read(join(SRC, 'i18n', 'locales', 'zh-CN.json'))) as Record<string, unknown>;
    const has = (key: string): boolean =>
      typeof key.split('.').reduce<unknown>((c, s) => (c as Record<string, unknown>)?.[s], zh) === 'string';

    /* ① 缺席那一档。**2026-09-13（批 16）起两张表全部 `ported`** ⇒ 这个循环今天空转。
       自检随之换形态（与本 it 头注那次改判同一条理由，只是这次轮到 `MESH_JOIN_ACTIONS`）：
       `toBeGreaterThan(0)` 会在「接完了」那天红，而接完不是缺陷 —— 那是本门要的结果。
       改成**两条并列**：缺席集恰等于今天的答案（空集，且哪天回来一条会红，逼一次显式处置），
       外加一次**双向**的谓词对照，证明下面那个循环一旦有条目时真的判得动。 */
    const absent = [...MESH_JOIN_CHOICES, ...MESH_JOIN_ACTIONS].filter(
      (c) => c.disposition.kind !== 'ported',
    );
    expect(
      absent.map((c) => c.id),
      '两张组网登记表的缺席集变了 —— 多一条：接上它或写清理由键；少一条：把这里的答案调低',
    ).toEqual([]);
    for (const c of absent) {
      const key = c.disposition.kind === 'ported' ? '' : c.disposition.reasonKey;
      expect(has(key), `${c.id} 的理由键 ${key} 在 zh-CN 里不存在`).toBe(true);
    }
    /* 谓词的双向对照（循环空转时，它是「上面那条循环还判得动吗」的唯一证据）：
       对一个**真的在用**的理由键说存在，对一个不存在的说不存在。 */
    expect(has('nodes.batchMoveUnavailable'), '理由键谓词对真实存在的键说不存在 —— 它坏了').toBe(
      true,
    );
    expect(has('nodes.__no_such_key__'), '理由键谓词对不存在的键也说存在 —— 上面那条循环无信息量').toBe(
      false,
    );

    /* ② `ported` 的每一条都要有落点。取材面是**生产面板的 `formFor` 分流**：
       逐个 id 在源码里找 `case '<id>':`，并要求它紧跟的那一段里出现一个宿主真有的 `kind`。
       只断言「面板里出现过这个词」不行 —— 那连一句注释都能满足（上一版就是那么写的）。 */
    const panel = strip(read(join(MOBILE, 'forms', 'MeshJoinPanel.tsx')));
    const hostKinds = [
      ...strip(read(join(MOBILE, 'forms', 'MobileFormHost.tsx'))).matchAll(/case '([a-z-]+)':/g),
    ].map((m) => m[1]);
    expect(hostKinds.length, '宿主一个 case 都没解析出来 ⇒ 下面恒真').toBeGreaterThan(5);
    for (const c of MESH_JOIN_CHOICES) {
      if (c.disposition.kind !== 'ported') continue;
      const at = panel.indexOf(`case '${c.id}':`);
      expect(at, `${c.id} 登记成 ported，而 MeshJoinPanel 的分流里没有它这一支`).toBeGreaterThan(-1);
      const arm = panel.slice(at, panel.indexOf("case '", at + 6) === -1 ? undefined : panel.indexOf("case '", at + 6));
      const kinds = [...arm.matchAll(/kind: '([a-z-]+)'/g)].map((m) => m[1]);
      expect(kinds.length, `${c.id} 那一支没有给出任何表单 kind —— 点下去什么都不会开`).toBeGreaterThan(0);
      for (const k of kinds) {
        expect(hostKinds, `${c.id} 落到了宿主没有 case 的 kind: ${k}`).toContain(k);
      }
    }
    // WARP 有单例槽；Tailscale 已支持多节点，接入卡每次打开新登录表单。
    expect(panel, 'WARP 那一支没有按「槽位占没占」分流').toContain('edit: warpNode !== undefined');
    expect(panel, 'Tailscale 新节点入口不应猜一个现有节点来编辑').toContain("case 'tailscale':\n      return { kind: 'ts-login' };");
    expect(panel, 'Tailscale 接入入口不能退回单例寻址').not.toContain('tsNode.id');
  });

  /**
   * ⑥-c-2 组网接入面**卡片里的次动作**（批 3 新登记）。
   *
   * 桌面在 WARP / Tailscale 两张卡片上挂着 5 颗 `btn ghost sm`。上一版这 5 颗一颗都不在任何面上
   * ——那时两张卡片本身是灰的，次动作是「灰按钮底下的灰按钮」。两张卡片接通之后它们露出来了，
   * 故与 `ROW_ACTIONS` / `BATCH_ACTIONS` 同形登记，并在这里与桌面对差。
   */
  describe('⑥-c-2 组网接入面的次动作：桌面 ↔ 登记表', () => {
    /** 桌面那两处 `actions={…}` 里的 `btn ghost sm` 颗数。 */
    const desktopActions = (meshJoinSrc.match(/className="btn ghost sm/g) ?? []).length;

    it('自检：桌面取材面有量级（为 0 会让下面的等式恒绿）', () => {
      expect(desktopActions, '桌面组网卡片上一颗次动作都没数到').toBeGreaterThan(3);
    });

    it('桌面次动作数 == 登记表条数（桌面加一颗 ⇒ 红，逼一次显式处置决定）', () => {
      expect(MESH_JOIN_ACTIONS.length, `桌面组网卡片上有 ${desktopActions} 颗次动作`).toBe(
        desktopActions,
      );
    });

    it('每条登记指向的桌面控件真的存在（防登记表编出一批不存在的按钮）', () => {
      for (const a of MESH_JOIN_ACTIONS)
        expect(meshJoinSrc, `${a.id} 在 MeshJoinDialog 里找不到（${a.desktopRef}）`).toContain(
          a.desktopRef,
        );
    });

    /**
     * **五颗全部 `ported`（2026-09-13 批 16）**，逐颗要有落点。
     *
     * 🔴 落点**不在同一张表里**，这正是移动端对「接入面塞不下每条 2–3 颗次动作」的处置：
     * 五颗各自收进**它作用对象所在的那张表** —— Tailscale 三颗进 `TsSettingsPanel`，
     * WARP 两颗进 `WarpPanel` 的编辑态。故本条按落点分两组断言，而不是拿一张表去兜五颗。
     *
     * 每一颗都要求一条**它自己的**证据（开哪张表 / 调哪条腿），不是「面板里出现过这个词」——
     * 那连一句注释都能满足（⑥-c 的头注为同一条记过一次）。
     */
    it('登记成 ported 的五条在移动端各有落点（TS 三颗在 TsSettingsPanel，WARP 两颗在 WarpPanel）', () => {
      const tsPanel = strip(read(join(MOBILE, 'forms', 'TsSettingsPanel.tsx')));
      const warpPanel = strip(read(join(MOBILE, 'forms', 'WarpPanel.tsx')));
      const ported = MESH_JOIN_ACTIONS.filter((a) => a.disposition.kind === 'ported').map((a) => a.id);
      expect(ported, '五颗次动作没有全部登记成 ported').toEqual([
        'warp-reregister',
        'warp-deregister',
        'taildrop',
        'ts-switch-account',
        'ts-logout',
      ]);

      /* ── Tailscale 三颗 → `TsSettingsPanel` 末尾那一行 ── */
      expect(tsPanel, '切换账号那颗没有开登录表').toContain(
        "open({ kind: 'ts-login', serverId })",
      );
      expect(tsPanel, '退出登录那颗没有走后端登出腿').toContain('api.server.tailscaleLogout(');
      expect(tsPanel, '退出登录没有 staged-only 拦截 —— 盘上没有这个节点时它没有作用对象').toContain(
        'splitStagedOnly(',
      );
      expect(tsPanel, '破坏性动作没有二次确认').toContain("kind: 'confirm'");
      /* Taildrop 那颗：**必须带 `serverId`**。不带 id = 收件箱绑在「任意一个」Tailscale 节点上，
         那正是桌面本轮把那张卡改成多节点分行要消灭的缺陷；移动端靠这一跳结构性地避开它。 */
      expect(tsPanel, 'Taildrop 那颗没有按 serverId 开收件箱').toContain(
        "open({ kind: 'taildrop', serverId })",
      );
      expect(tsPanel, 'Taildrop 那颗没有未读角标（判据须与桌面同一函数）').toContain(
        'taildropBadgeCount(',
      );

      /* ── WARP 两颗 → `WarpPanel` 编辑态末尾 ── */
      expect(warpPanel, 'WARP 两颗没有复用节点屏那条删除编排').toContain(
        'deletion.removeWarpNode(',
      );
      expect(warpPanel, '重新注册那颗没有用它自己的确认/成功文案').toContain(
        "t('nodes.meshWarpReRegisterTitle')",
      );
      expect(warpPanel, '注销那颗没有用它自己的确认/成功文案').toContain(
        "t('nodes.meshWarpDeregisterTitle')",
      );
      /* 「重新注册」= 删完**再开注册表**。少了这一跳它就退化成第二颗「注销」。 */
      expect(warpPanel, '重新注册没有在删成之后打开注册表').toContain(
        "open({ kind: 'warp', edit: false })",
      );
    });
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑥-d 「构造器收到的到底是哪条腿」—— 十五条 handler 逐键钉死。
 *
 * 🔴 **本条 2026-09-06 补（批 2 复审 blocker）。** ⑥-b 只断言「源码里出现 `buildAddItems(`」
 * 与「`addItems={addItems}` 传给了视图」，而 ⑥ / ⑥-c 调构造器时喂的是**判据自己的 stub**
 * ⇒ 「接线层交给构造器的是什么」一条判据都没有。W-01 / W-02 / W-03 十五条 handler 里，
 * 此前只有 `onConnect` 被 ⑯-b 的「在场腿②」钉住。
 *
 * 实测两轮变异，各自 `tsc` rc=0 + 全量 vitest 全绿 + `report-wiring.sh` 逐字不变：
 *  ① `onManualAdd` / `onMeshJoin` / `onImport` 三条 `openMobileForm({…})` 换成 `() => undefined`
 *    （= 陈先生截图那三项点下去什么都不发生）；
 *  ② `onClone` / `onEdit` / `onDelete` 换成 `() => undefined`（克隆 / 编辑 / 删除全哑，
 *    而这三条刚从 `WIRING_DEBT_BASELINE_IDS` 里销掉，账本会继续报「已接」）。
 *
 * 切法与 ⑯-b 同源（`objectLiteralAt` 花括号配平），再多一步 [`objectEntry`] 把**单个键的值**
 * 切出来 —— 「这个文件里出现过 `cloneNode(`」不算数，那串字符在 `useCallback` 的 deps 里也有一份。
 * ═════════════════════════════════════════════════════════════════════════ */

/**
 * 对象字面量本体（含两端花括号）里，`key` 那一项的**值文本**。
 *
 * 从 `key:` 之后扫到**本层**的 `,` 或收口 `}`（`{}` / `[]` / `()` 三种括号一起配平，
 * 故值里的嵌套对象、数组、带逗号的实参列表都不会把切点提前）。切不出来就抛：
 * 返回空串会让下面每条 `toContain` 恒绿。
 */
function objectEntry(literal: string, key: string, what: string): string {
  const at = literal.indexOf(`${key}:`);
  if (at < 0) throw new Error(`${what}：对象里没有 \`${key}\` 这一项 —— 结构变了？`);
  const start = at + key.length + 1;
  let depth = 0;
  for (let i = start; i < literal.length; i += 1) {
    const c = literal[i];
    if (c === '{' || c === '[' || c === '(') depth += 1;
    else if (c === ')' || c === ']') depth -= 1;
    else if (c === '}') {
      if (depth === 0) return literal.slice(start, i).trim(); // 外层对象收口
      depth -= 1;
    } else if (c === ',' && depth === 0) return literal.slice(start, i).trim();
  }
  throw new Error(`${what}：\`${key}\` 的值没有收口 —— 切法失效`);
}

/**
 * 逐键登记：`[组, 定位锚, 键, 这条腿的值里必须出现的片段…]`。
 *
 * 每条「必须出现的片段」都取**动作本身**（`openMobileForm({ kind: 'x' })` / `cloneNode(` /
 * `deleteNode(` …），不取变量名 —— 换成 `() => undefined`、或包一层不干活的壳，都会红。
 */
const HANDLER_WIRING: ReadonlyArray<{
  readonly group: string;
  readonly anchor: string;
  readonly entries: ReadonlyArray<{ readonly key: string; readonly must: readonly string[] }>;
}> = [
  {
    group: '添加四条（W-01）',
    anchor: 'onManualAdd:',
    entries: [
      /* 三条产物落点不同的路径都要**切 tab**，否则表单一关屏上零变化（桌面
         `NodesHeader.tsx:81/:111`、`NodesScreen.tsx:491` 逐条如此）。 */
      { key: 'onManualAdd', must: ["setTab('manual')", "openMobileForm({ kind: 'node' })"] },
      { key: 'onMeshJoin', must: ["setTab('mesh')", "kind: 'mesh-join'"] },
      { key: 'onImport', must: ["kind: 'import'", "onAdded: () => setTab('manual')"] },
      { key: 'onAddSubscription', must: ["kind: 'sub'", 'onAdded: (id) => setTab(id)'] },
    ],
  },
  {
    group: '订阅更多五条（W-04）',
    anchor: 'onRename:',
    entries: [
      { key: 'onRename', must: ["kind: 'sub'", "focus: 'name'"] },
      { key: 'onEditUrl', must: ["kind: 'sub'", "focus: 'url'"] },
      { key: 'onCopyUrl', must: ['clipboard.writeText(sub.url)'] },
      { key: 'onInterval', must: ["navigateMobile('settings', 'update')"] },
      { key: 'onDelete', must: ['deleteSubscription(sub)'] },
    ],
  },
  {
    group: '行动作六条（W-02 / W-03）',
    anchor: 'onConnect:',
    entries: [
      { key: 'onSpeedTest', must: ['runSpeedTest('] },
      /* `onConnect` 的深一层（与首页同一条起核腿）由 ⑯-b 守；这里只守「接的是它」。 */
      { key: 'onConnect', must: ['connectFromRow'] },
      { key: 'onCopyLink', must: ['copyLink(row)'] },
      { key: 'onClone', must: ['cloneNode(row.server)'] },
      { key: 'onEdit', must: ['mobileEditFormFor(row.server)', 'openMobileForm('] },
      { key: 'onDelete', must: ['deleteNode(row.server)'] },
    ],
  },
];

describe('⑥-d 十五条 handler 逐键：接线层交给构造器的就是那条真腿', () => {
  const WIRING = strip(read(join(MOBILE, 'nodes', 'MobileNodesScreen.tsx')));

  it('自检：切法对「没有这一项」「值没收口」都抛，对嵌套括号不提前收口', () => {
    const lit = "{ a: f({ x: 1, y: [2, 3] }), b: g(1, 2), c: 3 }";
    expect(objectEntry(lit, 'a', '自检')).toBe('f({ x: 1, y: [2, 3] })');
    expect(objectEntry(lit, 'b', '自检')).toBe('g(1, 2)');
    expect(objectEntry(lit, 'c', '自检')).toBe('3');
    expect(() => objectEntry(lit, 'zz', '自检')).toThrow();
    expect(() => objectEntry('{ a: f(', 'a', '自检')).toThrow();
  });

  it.each(HANDLER_WIRING.map((g) => [g.group, g] as const))(
    '%s：每一条都接到了它自己那条腿',
    (_name, group) => {
      const literal = objectLiteralAt(WIRING, group.anchor, `${group.group} 的 handlers 对象`);
      // 切片自检：切出来的必须是那个对象本身（含它全部的键），不是外面某个更大的块。
      for (const e of group.entries) {
        expect(literal, `切出来的对象里没有 ${e.key} —— 切法失效`).toContain(`${e.key}:`);
      }
      for (const e of group.entries) {
        const value = objectEntry(literal, e.key, `${group.group} 的 ${e.key}`);
        expect(value.length, `${e.key} 的值切成了空串 ⇒ 下面恒绿`).toBeGreaterThan(2);
        for (const needle of e.must) {
          expect(
            value,
            `${group.group} 的 \`${e.key}\` 没有接到 \`${needle}\` —— ` +
              '菜单项照旧在场、标签对、灰不灰也对，按下去却什么都不发生',
          ).toContain(needle);
        }
      }
    },
  );

  it('🔴 反向自检：换成 `() => undefined` 的空壳、以及「引用了但没调」的壳，都必须报得出来', () => {
    for (const group of HANDLER_WIRING) {
      /* 合成一份**同形**的 handlers 对象，每一条都是不干活的壳 —— 这正是两轮实测变异的形态
         （① 添加三条改成 `() => undefined`；② 克隆/编辑/删除改成 `() => undefined`）。
         同一套切法 + 同一张 `must` 表跑过去，每一条都必须判否。 */
      const gutted = `{ ${group.entries.map((e) => `${e.key}: () => undefined`).join(', ')} }`;
      for (const e of group.entries) {
        const value = objectEntry(gutted, e.key, `空壳里的 ${e.key}`);
        expect(
          e.must.every((needle) => value.includes(needle)),
          `把 ${group.group} 的 ${e.key} 换成空壳之后判据仍然全绿 —— 它认不出「什么都没发生」`,
        ).toBe(false);
      }
      /* 第二种形态：**引用了那条腿但没调用它**（`() => void cloneNode`）。
         判据取的是带实参的调用而不是变量名，故这一形态同样必须判否 —— 只有 `onConnect`
         例外（它接的本来就是裸引用 `connectFromRow`，那是对的形态，故不喂这一组）。 */
      const referenced = `{ ${group.entries
        .filter((e) => e.key !== 'onConnect')
        .map((e) => `${e.key}: () => void ${e.must[0].replace(/\(.*$/, '')}`)
        .join(', ')} }`;
      for (const e of group.entries) {
        if (e.key === 'onConnect') continue;
        const value = objectEntry(referenced, e.key, `裸引用壳里的 ${e.key}`);
        expect(
          e.must.every((needle) => value.includes(needle)),
          `${group.group} 的 ${e.key} 只是引用了那条腿而没有调它，判据却是绿的`,
        ).toBe(false);
      }
    }
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑦ IA §4.12 —— hover 承载的解释必须换常驻通道。
 * 验收口径是 spec 自己定的：「逐条枚举该屏的 data-tip，给出每一条的落点。**是计数，不是印象。**」
 * ═════════════════════════════════════════════════════════════════════════ */

/**
 * 桌面节点屏 `data-tip` 里出现过、而移动端**不以同名 key 复用**的那些，逐条登记落点。
 * 不在此表且不在移动端源码里出现 ⇒ 红。
 */
const TIP_DISPOSITION: Record<string, string> = {
  'home.stagedOnlyHint':
    '节点卡的待保存 badge 保留，i 明细使用 mobileHelp.stagedNodeDetails，明确底部保存与运行时应用的区别。',
  /* 2026-09-06（批 2）：这两条的处置理由**换了**，因为它们原来的理由已经不成立 ——
     删除与订阅编辑都接上了。留在表里是因为移动端**仍然不复用这两句 key**，而不是因为缺口还在。 */
  'common.confirmAgain':
    '桌面的原地二次确认（按钮翻红 + 再点一次）在触屏上不成立：翻红那一下在拇指底下被自己的手指' +
    '挡住，第二击极易被读成误触重复。移动端改叠一层确认面板（forms/ConfirmPanel.tsx），' +
    '标题与正文说清「要删的是哪一个」，故不复用这句「再点一次」。',
  'nodes.subEditTitle':
    '订阅编辑已接（forms/SubFormPanel.tsx）。桌面这句是那颗铅笔图标按钮的 tooltip，' +
    '移动端把它做成订阅「更多」里带**文字标签**的两行（重命名 / 编辑）—— 标签自己就是解释，' +
    '不需要再挂一句同义的 tip。',
  'nodes.subMenuDisabledHint':
    '桌面这句是「调用方没给更多菜单」的自述；移动端那张 sheet 恒有五行可点的菜单项，' +
    '这句话在移动端结构上永远为假，故不复用。',
  'nodes.subUpdatingHint':
    '正在更新的状态与阶段直接写在订阅摘要的状态徽标；重复解释后台进程的 tooltip 没有额外决策信息。',
  'nodes.subAutoUpdateActiveHint':
    '订阅摘要用短的自动更新状态与上次更新时间说明当前状态；具体间隔在可达的订阅编辑设置里。',
  'nodes.shadowedHint':
    '旧句只覆盖字面 CIDR 去重，移动端改用来源与有效网段报告，在同一颗 i 解释作用层、前置例外和未知范围。',
  'nodes.meshRouteBlockedHint':
    '移动节点行展示组网证据状态，详情层 MobileMeshRouteEvidence 逐项列出被先行节点遮盖的网段与节点名称。',
  'nodes.meshRouteIncomplete':
    '移动组网证据详情明确提示候选条目不完整，未知覆盖不会被显示为零覆盖。',
  'nodes.meshRouteUnknownHint':
    '移动节点行展示未知状态，详情层列出报告和候选的未知原因及加载证据。',
};

/*
 * ⚠️ 这张表**只许放真正承重的条目**。给一条移动端其实已经复用了的 key 也写上处置，
 * 表面无害，实则是给门留了一个后门：将来那处复用被删掉时，本表会替它继续答「有落点」，
 * 而用户那边的解释已经没了。初版写了 9 条、其中 6 条是这种冗余，已删。
 */

describe('⑦ IA §4.12：桌面 data-tip 的每一条解释都有移动端落点（计数，不是印象）', () => {
  /** 桌面节点屏五个文件里，真实 `data-tip={...}` 属性中出现的字面 i18n key。 */
  const tipKeys = (() => {
    const keys = new Set<string>();
    for (const f of ['NodeCard.tsx', 'NodesTabs.tsx', 'NodesToolbar.tsx', 'NodesBatchBar.tsx', 'SubInfoBar.tsx']) {
      const body = strip(read(join(DESKTOP_NODES, f)));
      let i = body.indexOf('data-tip={');
      while (i !== -1) {
        let j = i + 'data-tip={'.length;
        let depth = 1;
        while (j < body.length && depth > 0) {
          if (body[j] === '{') depth += 1;
          else if (body[j] === '}') depth -= 1;
          j += 1;
        }
        for (const m of body.slice(i, j).matchAll(/t\('([\w.]+)'/g)) keys.add(m[1]);
        i = body.indexOf('data-tip={', j);
      }
    }
    return [...keys].sort();
  })();

  it('自检：桌面的 data-tip 取材面非空且有量级（扫 0 条会让下面那条恒绿）', () => {
    expect(tipKeys.length, `只从桌面节点屏扫到 ${tipKeys.length} 条 data-tip key —— 取材面塌了`).toBeGreaterThan(
      10,
    );
    expect(tipKeys).toContain('nodes.shadowedHint');
  });

  it('逐条有落点：要么移动端复用同一句 key，要么在处置表里登记', () => {
    const orphan = tipKeys.filter(
      (k) => !MOBILE_TEXT.includes(`'${k}'`) && TIP_DISPOSITION[k] === undefined,
    );
    expect(
      orphan,
      '这些解释在桌面只由 hover 承载，移动端既没复用也没登记落点 —— ' +
        '一个够不着含义的角标比没有角标更坏：它会被读成装饰，用户于是断定没出问题',
    ).toEqual([]);
  });

  it('处置表不留僵尸条目（登记了一条桌面上已经不存在的 tip）', () => {
    for (const k of Object.keys(TIP_DISPOSITION)) {
      expect(tipKeys, `处置表登记了 ${k}，但桌面节点屏的 data-tip 里已经没有它了`).toContain(k);
    }
  });

  it('卡片保留关键错误和状态，只用一枚 i 聚合背景解释，不在选择按钮内嵌帮助按钮', () => {
    const html = render({
      rows: [
        row('x', {
          invalidReason: 'why.invalid',
          speedTestable: false,
          speedTestBlockedHint: 'why.blocked',
          lanOnly: true,
          stagedOnly: true,
          meshRouteReport: meshReport,
        }),
      ],
    });
    for (const expected of ['why.invalid', 'mobileHelp.stagedSpeedTest', 'nodes.lanOnly', 'mobileHelp.stagedNode', 'mobileMeshRouteEvidence.summary.preview']) expect(html).toContain(expected);
    for (const explanation of ['why.blocked', 'nodes.lanOnlyHint', 'home.stagedOnlyHint', 'nodes.shadowedHint']) expect(html).not.toContain(explanation);
    expect((html.match(/class="mn-note"/g) ?? []).length).toBe(1);
    expect((html.match(/class="m-info-trigger"/g) ?? []).length).toBe(1);
    const main = html.slice(html.indexOf('class="mn-row-main"'), html.indexOf('class="mn-row-actions"'));
    expect((main.match(/<button/g) ?? []).length).toBe(0);
    expect(main).not.toContain('m-info-trigger');
  });

  it('当前编辑与运行报告失配时，节点卡不沿用旧报告的无重叠结论', () => {
    const html = render({ rows: [row('a', {
      meshRouteReport: meshReport,
      meshRouteContextMismatch: true,
    })] });
    expect(html).toContain('mobileMeshRouteEvidence.summary.unknown');
    expect(html).not.toContain('mobileMeshRouteEvidence.summary.full');
    expect(html).not.toContain('mobileMeshRouteEvidence.summary.preview');
  });

  it('同一卡片的 i 保留全部适用解释，待保存文案区分保存与应用，错误仍只在卡面', () => {
    const node = row('info', { invalidReason: 'invalid visible', speedTestable: false,
      speedTestBlockedHint: 'why.blocked', lanOnly: true, meshRouteReport: meshReport });
    const sections = nodeExplanationSections(node, baseProps().t);
    const html = renderToStaticMarkup(<>{sections.map((section, index) => <section key={index}><h3>{section.title}</h3><p>{section.text}</p></section>)}</>);
    expect(sections).toHaveLength(2);
    for (const text of ['why.blocked', 'nodes.lanOnlyHint']) expect(html).toContain(text);
    expect(html).not.toContain('invalid visible');
    const staged = nodeExplanationSections(row('staged-info', { stagedOnly: true, speedTestable: false, speedTestBlockedHint: 'old save-only wording' }), baseProps().t);
    expect(staged).toEqual([{ title: 'home.stagedOnlyBadge', text: 'mobileHelp.stagedNodeDetails' }]);
    expect(nodeExplanationSections(row('timeout-info', { latencyMs: null }), baseProps().t)).toEqual([{ title: 'nodes.timeout', text: 'nodes.timeoutHint' }]);
  });
});

describe('⑧ IA §2.1：没有登记数据源的位置一律不画数字', () => {
  /**
   * 🔴 **2026-09-06（批 2 / W-06）翻面。** 用量 / 到期 / 上次更新三处的来源已登记进
   * `mobile-kit/data-contract.json#sources.subscription-summary` ⇒ 它们从「标 pending」变成
   * **真的画**。本组随之从「断言那句 pending 在」改成两条成对的判据：
   *  · **有源就画**（正面）：三处给了值就必须出现在 DOM 上；
   *  · **没源不画**（反面，仍是本组的原始使命）：面板没下发那个字段 ⇒ 整格不出现，
   *    不画 `0 B / 0 B` 也不画一个占位 —— 「面板没给」与「用了 0」是两件事。
   * 登记表 `SUB_PENDING_FIELDS` 随之为空集，并由第三条钉住：它不是「这张表没用了」，
   * 而是这一格上的当前答案。
   */
  /** 生产的算值腿。**不自造夹具**：喂真实 `SubscriptionConfig` 跑 `buildSubSummary`。 */
  const summaryOf = (over: Partial<SubscriptionConfig>) =>
    buildSubSummary({
      t,
      sub: { id: 's1', name: 'sub-1', url: 'https://e/x', ...over } as SubscriptionConfig,
      nodeCount: 3,
      progress: undefined,
      config: undefined,
    });

  it('三处有值就画出来（登记了来源之后不许再藏）—— 值由生产的 `buildSubSummary` 算', () => {
    const vm = summaryOf({
      userInfo: { upload: 600_000_000, download: 600_000_000, total: 100_000_000_000, expire: 1_798_761_600 },
      lastUpdated: new Date(Date.now() - 5 * 60_000).toISOString(),
    });
    // 自检：三格真的算出来了（算不出来会让下面的渲染断言对着 undefined 恒假）。
    expect(vm.usage, '生产没算出用量 ⇒ 下面的渲染断言没有输入').toBeDefined();
    expect(vm.expiry, '生产没算出到期').toBeDefined();
    expect(vm.lastUpdated, '生产没算出上次更新').toBeDefined();
    expect(vm.usage?.text, '用量的单位没走 fmtBytes（自带单位，别再拼一个）').toMatch(/\d.*\/.*\d/);

    const html = render({ sub: vm });
    expect(html, '订阅摘要没渲染 ⇒ 下面每条恒假').toContain('mn-sub');
    expect(html, '用量没画出来').toContain(vm.usage!.text);
    expect(html, '用量条没画出来').toContain('mn-sub-bar');
    expect(html, '到期没画出来').toContain(vm.expiry!);
    expect(html, '上次更新没画出来').toContain(vm.lastUpdated!);
    // 那句 pending 说明已随来源登记一并删除，键也从五份 locale 里去掉了。
    expect(html).not.toContain('nodes.mobileSubPendingFields');
  });

  it('反向对照：面板没下发就整格不出现（不画 0，也不画占位）', () => {
    const vm = summaryOf({});
    expect([vm.usage, vm.expiry, vm.lastUpdated], '没有数据源却算出了值').toEqual([
      undefined,
      undefined,
      undefined,
    ]);
    const html = render({ sub: vm });
    expect(html, '订阅摘要没渲染 ⇒ 下面那些否定恒真').toContain('mn-sub');
    for (const banned of ['mn-sub-bar', 'nodes.subExpiry', '0 B']) {
      expect(html, `没有数据源却画了：${banned}`).not.toContain(banned);
    }
  });

  it('成对腿：接线层真的在用 `buildSubSummary`（抽函数造出的那道缝）', () => {
    const wiring = strip(read(join(MOBILE, 'nodes', 'MobileNodesScreen.tsx')));
    expect(
      wiring,
      '接线层没调 `buildSubSummary` —— 纯函数对了而生产另算一份，两份必然漂开',
    ).toContain('buildSubSummary({');
    expect(wiring, '算出来的 `sub` 没交给视图').toContain('sub={sub}');
  });

  it('登记表在这一格上的当前答案是空集（不是「这张表没用了」）', () => {
    expect(SUB_PENDING_FIELDS).toEqual([]);
  });

  /**
   * 「字节数有登记数据源」的文件登记表（2026-09-04，F1 首页落地时加）。
   *
   * 本组判据说的是「**节点行上**没有流量数字」—— 因为数据面里没有按节点归因的字节数。
   * 而下面那条源码腿的取材面是**整棵** `ui/src/mobile` 树，两者不等宽：首页的流量构成 /
   * 域名 Top 5 印的是按**连接**聚合的字节数，那是 `data-contract.json#cards` 里
   * `availability: available` 且 `verified: true` 的登记源，与本组要防的「无源画数字」不是一回事。
   *
   * 处置取**登记**而不是把取材面缩回 `mobile/nodes/`：缩回去等于放弃「别的屏无源画字节数」这张网，
   * 而那张网是真的有用（它挡的是下一个屏，不只是这一个）。登记表保留整树覆盖，代价是新增一个
   * 印字节数的屏必须来这里写一行，并说得出它的源在 `data-contract.json` 的哪一条。
   */
  const BYTES_SOURCED_FILES = [
    // F1 首页：traffic-composition / host-traffic-top，源 = connections-detail 的活动连接窗口。
    'mobile/home/HomeScreenView.tsx',
    /* 节点屏视图模型（批 2 / W-06）：订阅摘要的用量 `已用 / 总量`，源 =
       `data-contract.json#sources.subscription-summary`（`SubscriptionConfig.userInfo` 的
       upload/download/total，由订阅面板随更新下发）。判据与桌面 `SubInfoBar.tsx:191` 同一条
       （`nodes-logic#subUsage` + `fmtBytes`）。**它不是按节点归因的字节数** —— 本组防的是那个，
       而这一处印的是订阅面板自报的配额，两者不是一回事。 */
    'mobile/nodes/view-model.ts',
    // F5 连接屏：活动/已结束行的上下行字节，源 = `ConnectionEntry.upload/download`
    //（`data-contract.json#sources.connections-detail`）。缺席渲染成「—」而不是 0 B ——
    // absent 不是 measured zero，印 0 是在断言一件我们不知道的事。
    'mobile/connections/ConnectionsView.tsx',
    // F3 规则屏「资源」段：资源行的文件大小，源 = `RuleResourceListItem.size`
    //（`data-contract.json#sources.rule-resources`，2026-09-06 登记）。它是**盘上那个文件的大小**，
    // 不是流量归因，与本组要防的「按节点归因的字节数」不是一回事；同一条源里的
    // `downloadedAt` 在下载中 / 失败 / 已取消三态下不画，理由在那条登记的 traps 里。
    'mobile/screens/rules/ResourcesSegment.tsx',
    /* F5 连接屏接线侧（批 13）：W26 前遗留的 `singbox.log` 有多大，源 =
       `api.logs.legacyInfo()` 的 `bytes`（后端 `logs_legacy_info` 读 `fs::metadata(...).len()`，
       只读、单一真值）。它是**盘上那个文件的大小**，与本组要防的「按节点归因的字节数」
       不是一回事；探测不到时**整块提示不画**（`legacyLog` 停在 `null`），而不是印 0 B。 */
    'mobile/connections/MobileConnectionsScreen.tsx',
    /* Taildrop 收件箱（批 16）：待处理文件的大小、接收中的 `已收 / 总量`、发件任务的
       `已发 / 总量`。源 = 后端 `taildrop_list` / `taildrop_tasks`（sing-box 管理 API 的
       `SubscribeTaildropInbox` 首帧投影 + `TaildropTaskSnapshot`），逐字段镜像在
       `contracts/taildrop.ts` 上。它是**这一个文件有多大 / 传了多少**，不是按节点归因的流量 ——
       本组防的是后者。拉不到时整块不画（`inbox` 停在 `EMPTY`、`tasks` 停在空表），
       而不是印 0 B。 */
    'mobile/forms/TaildropPanel.tsx',
  ];

  it('每个节点行上没有任何流量数字（数据面里没有按节点归因的字节数，IA 明记「no source」）', () => {
    const html = render();
    for (const banned of ['fmtBytes', 'nd-usage', 'mn-bytes']) {
      expect(html).not.toContain(banned);
    }
    // 自检：登记表不是僵尸（登记了一个已经不印字节数的文件 = 白名单在腐烂）。
    for (const f of BYTES_SOURCED_FILES) {
      expect(strip(read(join(SRC, f))), `${f} 已经不用 fmtBytes 了 —— 该把它从登记表里删掉`).toContain(
        'fmtBytes',
      );
    }
    const offenders = MOBILE_SRC.map(rel)
      .filter((f) => !BYTES_SOURCED_FILES.includes(f))
      .filter((f) => strip(read(join(SRC, f))).includes('fmtBytes'));
    expect(
      offenders,
      '这些移动端文件印了字节数，但没在 BYTES_SOURCED_FILES 里登记数据源 —— ' +
        '数据面里没有按节点归因的字节数，无源画数字就是在断言一件我们不知道的事',
    ).toEqual([]);    /*
     * 两条线各自撞上同一堵墙，处置取 F1 的形状（主会话 2026-09-05 裁）。
     *
     * F5 当时的做法是把取材面**收窄**到 `mobile/nodes/**`，理由是「判据说的是节点屏的事实，
     * 射程要从判据读」——那句话对，但它推出的结论只在「另一个屏也确实有源」时成立。
     * 收窄的代价是**放弃「别的屏无源画字节数」那张网**，而那张网挡的是**下一个**屏，不只是这一个。
     * 登记表两头都要：全树覆盖保住那张网，登记项让有源的屏显式说出它的源在
     * `data-contract.json` 的哪一条，僵尸自检让过期登记自己冒出来。
     */
  });
});

describe('⑨ 空态四档齐（IA §2.1 States 前四行）', () => {
  it('四档各自出对应文案，且都不是同一句', () => {
    const seen = new Set<string>();
    for (const [kind, key] of [
      ['all', 'nodes.empty'],
      ['filtered', 'nodes.emptyFiltered'],
      ['sub', 'nodes.emptySub'],
      ['mesh', 'nodes.meshEmpty'],
    ] as const) {
      const html = render({ rows: [], emptyKind: kind });
      expect(html, `${kind} 档没出 ${key}`).toContain(key);
      seen.add(key);
    }
    expect(seen.size).toBe(4);
  });

  it('延迟陈旧渲染成「陈旧」而不是一个数（IA §2.1：never as a number）', () => {
    const html = render({ rows: [row('s', { latencyMs: 42, latencyStale: true })] });
    expect(html).toContain('nodes.mobileLatencyStale');
    expect(html).not.toContain('42 ms');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑩ IA 裁定 #14 —— 写操作失败必须可见。
 *
 * 移动外壳既无 `DialogHost` 也无 toast 宿主 ⇒ 一次写失败今天的默认表现是
 * 「状态自己弹回去，一句话都没有」（`contracts/action-failure-visibility.test.ts` 守的正是这个形态，
 * W10/W14 真机首曝过）。本屏的处置是行内错误 + **唯一写出口** `runWrite`。
 *
 * 判据刻意**不只钉「有 catch」**：有 catch 而 catch 里什么都不做，正是要抓的那一种。
 * 故第三条断言直接看 `runWrite` 的 catch 体里有没有写进错误通道。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑩ 写操作失败必有可见回显（IA 裁定 #14）', () => {
  const wiringSrc = strip(read(join(MOBILE, 'nodes', 'MobileNodesScreen.tsx')));

  /** 「这一处会写盘 / 会调后端 / 会碰系统资源」的记号。新增一类写操作也要登记进来。 */
  const WRITE_CALLS = [
    /\bapi\.[a-zA-Z]+\.[a-zA-Z]+\s*\(/g,
    /\bswitchServer\s*\(/g,
    /\bnavigator\.clipboard\.[a-zA-Z]+\s*\(/g,
    /\brefreshSubscriptionWithToast\s*\(/g,
  ];

  /** `runWrite(` 每一次调用的括号平衡区间。 */
  const runWriteSpans = (() => {
    const spans: [number, number][] = [];
    const RE = /\brunWrite\s*\(/g;
    for (const m of wiringSrc.matchAll(RE)) {
      let i = m.index + m[0].length;
      let depth = 1;
      while (i < wiringSrc.length && depth > 0) {
        if (wiringSrc[i] === '(') depth += 1;
        else if (wiringSrc[i] === ')') depth -= 1;
        i += 1;
      }
      spans.push([m.index, i]);
    }
    return spans;
  })();

  const writeSites = WRITE_CALLS.flatMap((re) =>
    [...wiringSrc.matchAll(re)].map((m) => ({ text: m[0], index: m.index })),
  ).filter((site) => site.text !== 'api.config.meshRouteReport('); // read-only diagnostic query

  it('自检：本屏真的有一批写调用，且 `runWrite` 真的被调过（扫 0 处会让下面两条恒绿）', () => {
    expect(writeSites.length, `只扫到 ${writeSites.length} 处写调用 —— 判据面塌了`).toBeGreaterThan(4);
    expect(runWriteSpans.length, '一次 `runWrite(` 调用都没扫到').toBeGreaterThan(3);
  });

  it('每一处写调用都在 `runWrite(` 的参数区间内（新增一处绕过唯一出口的写 ⇒ 红）', () => {
    const escaped = writeSites.filter(
      (site) => !runWriteSpans.some(([a, b]) => site.index > a && site.index < b),
    );
    expect(
      escaped.map((x) => x.text),
      '这些写调用不在 `runWrite` 里 —— 它们失败时用户看不到任何东西：' +
        '状态弹回原值，一句话都没有（IA 裁定 #14 要抓的正是这个形态）',
    ).toEqual([]);
  });

  it('`runWrite` 的 catch 体真的写进了错误通道 —— 不是一个什么都不做的 catch', () => {
    const at = wiringSrc.indexOf('const runWrite');
    expect(at, '`runWrite` 不见了 —— 判据面塌了（改名了？）').toBeGreaterThan(-1);
    const body = wiringSrc.slice(at, at + 900);
    const catchAt = body.indexOf('catch');
    expect(catchAt, '`runWrite` 里没有 catch').toBeGreaterThan(-1);
    const catchBody = body.slice(catchAt, body.indexOf('},', catchAt));
    expect(catchBody, 'catch 里没有把失败送进 `setNotice` —— 空 catch 与没有 catch 等价').toContain(
      'setNotice',
    );
    expect(catchBody, 'catch 里报的不是错误档（tone: err）—— 失败被说成了别的语气').toContain(
      "tone: 'err'",
    );
  });

  it('错误档真的渲染得出来（通道存在但画不出来 = 门在但没牙）', () => {
    const html = render({ notice: { tone: 'err', text: 'why.write.failed' } });
    expect(html).toContain('mn-notice err');
    expect(html).toContain('why.write.failed');
  });

  it('反向对照：没有 notice 时不渲染这条（证明上一条不是恒真）', () => {
    expect(render()).not.toContain('mn-notice err');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑪ 订阅刷新的**失败详情**必须真的显示出来 —— 与 `domain/subscription-refresh.desktop-toast.test.ts`
 *    成对的那一半。
 *
 * # 为什么必须成对
 *
 * 2026-09-05 把 `refreshSubscriptionWithToast` 的返回从 `boolean` 拓宽成
 * `{ok:true} | {ok:false; detail}`，理由是移动外壳没有 toast 宿主、详情被吞在 toast 里。
 * 那次同时建了一道**行为门**（domain 侧：三态 toast 通道/实参逐条、14 个 errorKind 的详情与
 * toast 副标题逐字同源）。但那道门只证明「A 把详情**交出来**了」。
 *
 * 协调者随后跑的变异证明这不够：把消费侧改成
 * ```ts
 * if (!r.ok) setNotice({ tone: 'err', text: t('nodes.subRefreshFail') });   // r.detail 丢弃
 * ```
 * `tsc rc=0`，`src/mobile/` + `src/domain/` + `src/styles/` **938 条全绿**。
 * 用户看到的东西与改动前一模一样：「刷新失败」四个字，为什么失败一个字都没有 ——
 * 缺陷只是从「吞在 toast 里」搬到了「吞在消费侧」。
 *
 * 这是本轮第四次撞见同一个形状（原生把安全区送进 web 层有门 / 贴边容器有没有用它没门；
 * `visibilitychange` 重读接线；抽成可测函数后调用点没门；现在是 domain 返回详情 / 消费侧有没有用）。
 * 结论写在这里当作纪律：**凡是「把信息从 A 交到 B」的修复，A 侧的行为门与 B 侧的接线判据必须成对交。**
 *
 * # 判据形态：为什么是 AST，以及 AST 那条怎么才算有牙
 *
 * 行为断言在这一格走不通：`refreshSub` 是 `MobileNodesScreen`（接线层）里的闭包，
 * 要驱动它得渲染一个挂着 store / IPC / i18n 的组件，而本仓 vitest 是 `environment:'node'`、
 * 刻意不装 jsdom（见文件头）。呈现层已经切成纯组件，接线层按定义切不出来 —— 切得出来就不叫接线了。
 *
 * 故走 AST，并且**判据必须能区分「用了 `r.detail`」与「调用了 `t(...)` 兜底」**：
 * 「源码里出现过 `r.detail`」这种字符串级断言挡不住「出现在一条永不执行的分支里」，
 * 也挡不住「`text` 里其实写的是 `t(...)`，`r.detail` 只出现在旁边的注释/日志里」。
 * 下面的分析器沿 `await refreshSubscriptionWithToast(…)` → 绑定名 → `if (!<绑定>.ok)` →
 * 该分支里的 `setNotice({…})` → `text` 那一格的**表达式种类**逐跳走下来，并把种类分成
 * `detail` / `t-call` / `literal` / `other` / `absent` 五档报出来。
 *
 * 分析器自身是**纯函数**，故它有真正的行为对照：下面拿 5 份合成夹具喂它（1 好 4 坏），
 * 断言它对「好」放行、对四种「坏」各自给出正确的档位 —— 这条证明它不恒绿也不恒红。
 * ═════════════════════════════════════════════════════════════════════════ */

import * as ts from '@/test/ts-compiler';

/** `text:` 那一格装的是什么。 */
type TextKind = 'detail' | 't-call' | 'literal' | 'other' | 'absent';

interface DetailSite {
  /** `await refreshSubscriptionWithToast(…)` 绑定到的那个名字（`const r = await …` 里的 `r`）。 */
  readonly binding: string | null;
  /** 有没有一条以该绑定的 `.ok` 为条件的分支。 */
  readonly guarded: boolean;
  /** 该分支里有没有 `setNotice(`。 */
  readonly notified: boolean;
  readonly tone: string | null;
  readonly textKind: TextKind;
}

/**
 * 沿「调用 → 绑定 → `.ok` 分支 → `setNotice.text`」逐跳分析。
 * 返回每一处调用点的结论；**不做任何断言**，好让它能被合成夹具直接驱动。
 */
function analyzeDetailWiring(fileName: string, source: string): DetailSite[] {
  const sf = ts.parseSourceFile(fileName, source);
  const all: ts.Node[] = [];
  const walk = (n: ts.Node): void => {
    all.push(n);
    ts.forEachChild(n, walk);
  };
  walk(sf);

  const calleeName = (c: ts.CallExpression): string | null => {
    const e = c.expression;
    if (ts.isIdentifier(e)) return e.text;
    if (ts.isPropertyAccessExpression(e) && ts.isIdentifier(e.name)) return e.name.text;
    return null;
  };
  const under = (root: ts.Node): ts.Node[] => {
    const out: ts.Node[] = [];
    const w = (n: ts.Node): void => {
      out.push(n);
      ts.forEachChild(n, w);
    };
    w(root);
    return out;
  };

  const calls = all.filter(
    (n): n is ts.CallExpression =>
      ts.isCallExpression(n) && calleeName(n) === 'refreshSubscriptionWithToast',
  );

  return calls.map((call): DetailSite => {
    // ① 绑定名：`const <b> = await refreshSubscriptionWithToast(…)`。
    //    `await` 在中间，故往上找两层（CallExpression → AwaitExpression → VariableDeclaration）。
    let p: ts.Node | undefined = call.parent;
    while (p !== undefined && !ts.isVariableDeclaration(p) && !ts.isStatement(p)) p = p.parent;
    const binding =
      p !== undefined && ts.isVariableDeclaration(p) && ts.isIdentifier(p.name) ? p.name.text : null;
    if (binding === null) return { binding, guarded: false, notified: false, tone: null, textKind: 'absent' };

    // ② 以 `<binding>.ok` 为条件的分支（`!r.ok` / `r.ok === false` 都算）。
    //    射程收在**同一个函数体**内，避免跨函数误配到另一处同名绑定。
    let fn: ts.Node | undefined = p;
    while (
      fn !== undefined &&
      !ts.isArrowFunction(fn) &&
      !ts.isFunctionExpression(fn) &&
      !ts.isFunctionDeclaration(fn)
    )
      fn = fn.parent;
    const scope = fn ?? sf;
    const guard = under(scope).find(
      (n): n is ts.IfStatement =>
        ts.isIfStatement(n) &&
        under(n.expression).some(
          (x) =>
            ts.isPropertyAccessExpression(x) &&
            ts.isIdentifier(x.expression) &&
            x.expression.text === binding &&
            ts.isIdentifier(x.name) &&
            x.name.text === 'ok',
        ),
    );
    if (guard === undefined) return { binding, guarded: false, notified: false, tone: null, textKind: 'absent' };

    // ③ 该分支里的 `setNotice({ tone, text })`。
    const notice = under(guard.thenStatement).find(
      (n): n is ts.CallExpression => ts.isCallExpression(n) && calleeName(n) === 'setNotice',
    );
    if (notice === undefined) return { binding, guarded: true, notified: false, tone: null, textKind: 'absent' };

    const arg = notice.arguments[0];
    if (arg === undefined || !ts.isObjectLiteralExpression(arg))
      return { binding, guarded: true, notified: true, tone: null, textKind: 'other' };
    const propOf = (name: string): ts.Expression | null => {
      for (const pr of arg.properties)
        if (
          ts.isPropertyAssignment(pr) &&
          (ts.isIdentifier(pr.name) || ts.isStringLiteral(pr.name)) &&
          pr.name.text === name
        )
          return pr.initializer;
      return null;
    };
    const toneExpr = propOf('tone');
    const tone =
      toneExpr !== null && ts.isStringLiteral(toneExpr) ? toneExpr.text : toneExpr === null ? null : '<非字面量>';

    // ④ `text` 那一格的**表达式种类** —— 本判据的核心：`<binding>.detail` 与 `t(…)` 必须分得开。
    const textExpr = propOf('text');
    let textKind: TextKind = 'absent';
    if (textExpr !== null) {
      if (
        ts.isPropertyAccessExpression(textExpr) &&
        ts.isIdentifier(textExpr.expression) &&
        textExpr.expression.text === binding &&
        ts.isIdentifier(textExpr.name) &&
        textExpr.name.text === 'detail'
      )
        textKind = 'detail';
      else if (ts.isCallExpression(textExpr) && calleeName(textExpr) === 't') textKind = 't-call';
      else if (ts.isStringLiteral(textExpr) || ts.isNoSubstitutionTemplateLiteral(textExpr))
        textKind = 'literal';
      else textKind = 'other';
    }
    return { binding, guarded: true, notified: true, tone, textKind };
  });
}

/* ── 合成夹具：分析器自身的行为对照（1 好 4 坏）────────────────────────────── */

const fixture = (body: string): string => `
declare const runWrite: (op: () => Promise<void>, d: () => string) => Promise<void>;
declare const refreshSubscriptionWithToast: (id: string, t: unknown) => Promise<{ ok: boolean; detail: string }>;
declare const setNotice: (n: { tone: string; text: string }) => void;
declare const t: (k: string) => string;
export const refreshSub = (subId: string) => {
  void runWrite(async () => {
${body}
  }, () => t('nodes.subRefreshFail'));
};
`;

const FIXTURES: ReadonlyArray<{ name: string; body: string; want: TextKind; guarded: boolean }> = [
  {
    name: '好：详情进屏级 notice',
    body: `    const r = await refreshSubscriptionWithToast(subId, t);
    if (!r.ok) setNotice({ tone: 'err', text: r.detail });`,
    want: 'detail',
    guarded: true,
  },
  {
    name: '坏①：退回笼统文案（协调者跑的那个变异）',
    body: `    const r = await refreshSubscriptionWithToast(subId, t);
    if (!r.ok) setNotice({ tone: 'err', text: t('nodes.subRefreshFail') });`,
    want: 't-call',
    guarded: true,
  },
  {
    name: '坏②：写死一句话',
    body: `    const r = await refreshSubscriptionWithToast(subId, t);
    if (!r.ok) setNotice({ tone: 'err', text: 'refresh failed' });`,
    want: 'literal',
    guarded: true,
  },
  {
    name: '坏③：三元绕过（`r.detail` 出现了，但不是这一格的值）',
    body: `    const r = await refreshSubscriptionWithToast(subId, t);
    if (!r.ok) setNotice({ tone: 'err', text: r.ok ? r.detail : t('nodes.subRefreshFail') });`,
    want: 'other',
    guarded: true,
  },
  {
    name: '坏④：根本不看返回值（失败静默）',
    body: `    const r = await refreshSubscriptionWithToast(subId, t);
    void r;`,
    want: 'absent',
    guarded: false,
  },
];

describe('⑪ 订阅刷新失败详情：domain 交出来了，消费侧必须真的显示它', () => {
  const wiringPath = join(MOBILE, 'nodes', 'MobileNodesScreen.tsx');
  const sites = analyzeDetailWiring(wiringPath, read(wiringPath));

  it('自检：分析器真的在接线层扫到了调用点（扫 0 处会让下面两条恒绿）', () => {
    expect(sites.length, '一处 `refreshSubscriptionWithToast(` 都没扫到 —— 判据面塌了').toBe(1);
    expect(sites[0].binding, '调用结果没有绑定到一个名字上 —— 返回值被直接丢弃了？').not.toBeNull();
  });

  it('分析器自身的行为对照：1 好 4 坏各判成对应的档（证明它不恒绿也不恒红）', () => {
    for (const f of FIXTURES) {
      const got = analyzeDetailWiring(`fixture-${FIXTURES.indexOf(f)}.tsx`, fixture(f.body));
      expect(got.length, `${f.name}：夹具里应恰好一处调用点`).toBe(1);
      expect(got[0].guarded, `${f.name}：`).toBe(f.guarded);
      expect(got[0].textKind, `${f.name}：分析器把 text 判成了 ${got[0].textKind}`).toBe(f.want);
    }
    // 正面：五档确实各不相同（不是「怎么喂都返回同一个值」）。
    expect(new Set(FIXTURES.map((f) => f.want)).size).toBe(5);
  });

  it('失败分支存在，且报的是错误档（tone: err）', () => {
    expect(sites[0].guarded, '接线层没有对 `ok:false` 分流 —— 刷新失败时什么都不会发生').toBe(true);
    expect(sites[0].notified, '失败分支里没有 `setNotice(` —— 失败没有进本屏唯一那条可见通道').toBe(true);
    expect(sites[0].tone, '失败被报成了别的语气').toBe('err');
  });

  it('`text` 那一格装的就是 `<绑定>.detail`，不是 `t(…)` 兜底', () => {
    expect(
      sites[0].textKind,
      `接线层把失败详情丢了：\`setNotice.text\` 装的是 **${sites[0].textKind}** 而不是 ` +
        '`r.detail`。domain 侧已经把后端分类过的详情交出来了（见 ' +
        '`domain/subscription-refresh.desktop-toast.test.ts` 的「每个 errorKind 的详情都 ≠ 笼统文案」），' +
        '这里再回落成 `t(\'nodes.subRefreshFail\')` 等于把它重新吞掉一次 —— ' +
        '用户读到的仍是「刷新失败」四个字，而 domain 那道门照样全绿。' +
        '（若你把写法换成解构 `const { ok, detail } = …`，本判据要跟着改：它今天只认属性访问形态。）',
    ).toBe('detail');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑫ IA §3.1 第 18 条 —— 行内的**颜色角色**原样过来。
 *
 * 这一组守的是「实现把某一层写丢了」这一类：延迟位此前只有一条 `color: --fg-dim`，四十行灰色
 * 等宽数字，40ms 与 900ms 长得一模一样 —— 挑快节点这件事失去了唯一的扫视通道。
 * 判据落在**真渲染出的 class** 上而不是 CSS 文本里：CSS 里写了五条规则、而 JSX 一条都不挂，
 * 是这类缺陷最常见的活法（「机制有门、接线没门」）。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑫ 行内颜色角色：延迟分档色 + 角标族', () => {
  /** 取一行渲染结果里 `.mn-lat` 的档位类名。 */
  const latClassOf = (over: Partial<NodeRowVM>): string | null => {
    const html = render({ rows: [row('x', over)] });
    const m = /class="mn-lat ([a-z]+)"/.exec(html);
    return m === null ? null : m[1];
  };

  it('自检：延迟位真的渲染得出来（读不到 class 会让下面每条恒绿）', () => {
    expect(latClassOf({ latencyMs: 42 }), '一行的 `.mn-lat` 没渲染出档位类名').not.toBeNull();
  });

  it('五档逐个落到真实 DOM 上，阈值取桌面 `latLevel`（fast<80 / mid<150 / slow<300 / 其余 dead）', () => {
    expect(latClassOf({ latencyMs: 40 })).toBe('fast');
    expect(latClassOf({ latencyMs: 120 })).toBe('mid');
    expect(latClassOf({ latencyMs: 260 })).toBe('slow');
    expect(latClassOf({ latencyMs: 900 })).toBe('dead');
    // 真实超时（`null`）也是 dead —— 与桌面 `latLevel(null)` 同档。
    expect(latClassOf({ latencyMs: null })).toBe('dead');
    // 边界各取一格：79/80 与 149/150 必须落在两侧，证明用的是同一套阈值而不是「差不多」。
    expect(latClassOf({ latencyMs: 79 })).toBe('fast');
    expect(latClassOf({ latencyMs: 80 })).toBe('mid');
    expect(latClassOf({ latencyMs: 149 })).toBe('mid');
    expect(latClassOf({ latencyMs: 150 })).toBe('slow');
  });

  it('`none` 三档：没测过 / 陈旧 / 结构上不可测都不许染语义色（那是替用户断言一件不知道的事）', () => {
    expect(latClassOf({ latencyMs: 42, latencyStale: true })).toBe('none');
    expect(latClassOf({ speedTestable: false, speedTestBlockedHint: 'why' })).toBe('none');
    // 没测过（`undefined`）连文案都没有 ⇒ 整个位不渲染，这也是「不画一个数」的一种。
    expect(latClassOf({ latencyMs: undefined })).toBeNull();
  });

  it('文案与档位色同源：写着「超时」的那一格必须是 dead，不是中性色', () => {
    const html = render({ rows: [row('x', { latencyMs: null })] });
    expect(html).toContain('nodes.timeout');
    expect(html).toContain('class="mn-lat dead"');
    // 反向对照：陈旧那一格写的是「陈旧」且色退回 none —— 两句话不许互相拆台。
    const stale = render({ rows: [row('x', { latencyMs: 42, latencyStale: true })] });
    expect(stale).toContain('nodes.mobileLatencyStale');
    expect(stale).toContain('class="mn-lat none"');
  });

  it('CSS 侧五档都有规则，且颜色角色逐条对齐桌面（`--ok`/`--flow-hi`/`--warn`/`--err`/`--fg-faint`）', () => {
    for (const [cls, token] of [
      ['fast', '--ok'],
      ['mid', '--flow-hi'],
      ['slow', '--warn'],
      ['dead', '--err'],
      ['none', '--fg-faint'],
    ] as const) {
      expect(
        won(`.mn-lat.${cls}`),
        `.mn-lat.${cls} 没有落到 ${token} —— 档位在 DOM 上分开了，颜色却没分开`,
      ).toContain(`hsl(var(${token}))`);
      // 「更窄选择器从旁边压下来」那一族（2026-09-05 MAJ-3 实测：追加一条
      // `.mn-row .mn-lat.fast{ color: hsl(var(--err)) }` 特异度更高必胜，真机上 fast 档变红，
      // 而本组只调 `winners`、当时那个桶对它等于不存在 ⇒ 全量 rc=0）。
      expect(
        narrowerOf(`.mn-lat.${cls}`),
        `有更窄的选择器覆写了 .mn-lat.${cls} 的分量 —— 上面那条断言在它命中的元素上不成立`,
      ).toEqual([]);
    }
  });

  it('角标族：协议挂 `proto`、可作出口挂 `exit`（此前两颗被降成同一颗中性标签）', () => {
    const html = render({ rows: [row('x', { isExit: true, isCurrent: false })] });
    expect(html, '协议角标没挂 proto').toContain('class="mn-pill proto"');
    expect(html, '「可作出口」角标没挂 exit').toContain('class="mn-pill exit"');
    // 反向对照：非组网行不该出现 exit 那颗（证明上一条不是恒真）。
    expect(render({ rows: [row('y', { isExit: false })] })).not.toContain('mn-pill exit');
  });

  it('已保存选择以互动色标识，不暗示运行核已经切到该出口', () => {
    expect(render({ rows: [row('x', { isCurrent: true })] })).toContain('class="mn-pill cur"');
    expect(render({ rows: [row('x', { isCurrent: true })] })).toContain('nodes.selectedChoice');
    const body = won('.mn-pill.cur');
    expect(body).toContain('var(--flow-hi)');
    expect(body).toContain('var(--flow-weak)');
    expect(body).not.toContain('var(--ok)');
  });

  it('角标族的**填充**逐条落到角色色（此前两颗被降成同一颗中性标签，正是「类名在、色是中性」）', () => {
    // 与上面 `.mn-lat` 那条同一套写法：DOM 腿钉类名、CSS 腿钉**胜出值**，两条都在才算有牙。
    // 只钉类名的门守不住本条缺陷自己的复发向量 —— 删掉这三条规则，类名照挂，渲染面全绿。
    for (const [sel, tokens] of [
      ['.mn-pill.proto', ['--flow-weak', '--flow-hi']],
      ['.mn-pill.exit', ['--aurora-weak', '--aurora-hi']],
    ] as const) {
      for (const token of tokens)
        expect(won(sel), `${sel} 没有落到 ${token}`).toContain(`var(${token})`);
    }
    // 基础态那句中性底也在射程内：删掉它，`.mn-pill.warn`/`.err` 之外的角标会退成透明底。
    expect(won('.mn-pill'), '角标基础态丢了 surface-3 底').toContain('var(--surface-3)');
    // 更窄覆写钉成**具名清单**：基础态本来就被这五个变体压着（那是设计），
    // 但多出第六个（或少一个）都必须当场红 —— 否则「基础态是中性底」这句话在那一档不成立。
    expect(
      narrowerOf('.mn-pill'),
      '`.mn-pill` 的更窄覆写清单变了 —— 上面几条基础态断言的射程跟着变了',
    ).toEqual(['.mn-pill.cur', '.mn-pill.err', '.mn-pill.exit', '.mn-pill.proto', '.mn-pill.warn']);
    for (const sel of ['.mn-pill.cur', '.mn-pill.proto', '.mn-pill.exit'])
      expect(narrowerOf(sel), `${sel} 被更窄的选择器压了 —— 上面的角色色断言在那一档不成立`).toEqual(
        [],
      );
  });

  it('角标的几何链不许改（`styles/text-fit.test.ts` 现场读它推算宽度，那道门不在本批射程里）', () => {
    // 逐**分量**判：`padding: 2px 6px` 只是今天的写法，补一句 `padding-inline: 20px` 同样改宽度，
    // 而盯着 `'padding: 2px 6px'` 这串原文的旧写法看不见它。
    const geo = cssWinners('.mn-pill');
    const at = (k: string) => geo.get(k)?.value;
    expect([at('padding-top'), at('padding-right'), at('padding-bottom'), at('padding-left')],
      '.mn-pill 的 padding 变了 —— text-fit 的 MN_PILL_AVAIL(258) 要同批重算',
    ).toEqual(['2px', '6px', '2px', '6px']);
    for (const edge of ['top', 'right', 'bottom', 'left'] as const)
      expect(at(`border-${edge}-width`), '.mn-pill 的 border 宽度变了 —— 同上').toBe('1px');
    expect(at('border-radius'), '.mn-pill 不再是胶囊（radius.pill = 999）').toBe('999px');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑬ IA §3.1 第 9 条 —— search-field 的两件新东西：前导字形与清除键。
 *
 * 清除键那半条 spec 自己写了理由（触屏没有 Esc），而 `type="search"` 并不兑现它：
 * Android WebView 默认不渲染原生清除键。所以判据要看**真画出来的那颗按钮**，
 * 不是「input 的 type 对不对」。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑬ 搜索框：前导字形 + 清除键（§3.1 第 9 条）', () => {
  /** `.mn-search` 这一块的渲染结果（里面只有 svg / input / button，切到 `</label>` 即整块）。 */
  const searchBlock = (search: string): string => {
    const html = render({ search });
    const s = html.indexOf('class="mn-search"');
    expect(s, '搜索框没渲染出来 ⇒ 下面每条恒绿').toBeGreaterThan(-1);
    const e = html.indexOf('</label>', s);
    expect(e, '`.mn-search` 没有闭合标签 —— 切片塌了').toBeGreaterThan(s);
    return html.slice(s, e);
  };

  it('前导字形在场，且用的是登记过的 `action.search` 字形（逐字对桌面 NodesToolbar 的 path）', () => {
    const block = searchBlock('');
    expect(block, '搜索框没有前导字形 —— 它在一排同样圆角边框的控件里没有身份标识').toContain('<svg');
    expect(block, '前导字形不是登记过的 action.search').toContain('M20 20l-3-3');
    // 字形逐字取自桌面，不是重画的（icon-registry: allowRedesign=false）。
    expect(read(join(SRC, 'components', 'screens', 'nodes', 'NodesToolbar.tsx'))).toContain('M20 20l-3-3');
  });

  it('有输入时清除键在场，带可读标签与登记过的 `action.close` 字形', () => {
    const block = searchBlock('hk');
    expect(block, '输入后没有清除键 —— 触屏没有 Esc，只能逐字退格').toContain('mn-search-clear');
    expect(block).toContain('aria-label="mobileConnections.clearSearch"');
    expect(block, '清除键的字形不是登记过的 action.close').toContain('M5 5l14 14M19 5L5 19');
  });

  it('反向对照：搜索框为空时不画清除键（证明上一条不是恒真）', () => {
    const empty = searchBlock('');
    expect(empty).not.toContain('mn-search-clear');
    expect(empty, '反向对照本身要有信息量：这一轮仍然渲染出了搜索框与前导字形').toContain('<svg');
  });

  it('CSS：容器是三段式且保住 48 触控目标（input 自己不再有框，焦点环挂容器）', () => {
    const css = read(join(MOBILE, 'nodes', 'nodes.css'));
    expect(/\.mn-search\s*\{[^}]*min-height:\s*var\(--tap-min\)/.test(css), '搜索框丢了 48 最小高').toBe(true);
    expect(/\.mn-search:focus-within\s*\{[^}]*outline:/.test(css), 'input 去了原生框却没补焦点环').toBe(true);
    expect(/\.mn-search-clear\s*\{[^}]*min-height:\s*var\(--tap-min\)/.test(css), '清除键不足 48').toBe(true);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑭ IA §4.12 + §2.1 States —— 分组 tab 上的订阅更新失败标记必须**带文案**。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑭ 分组段的订阅更新失败标记（§2.1 States + §4.12）', () => {
  const withFailure = (): string =>
    render({
      groups: [
        { id: 'manual', label: 'nodes.tab.manual', count: 2 },
        { id: 'sub-1', label: 'sub-1', count: 3, failureDetail: 'why.sub.failed' },
      ],
    });

  it('失败的分组带的是与桌面同键的文案，不是一个裸「!」', () => {
    const html = withFailure();
    expect(html, '分组段没渲染 ⇒ 下面的断言恒绿').toContain('mn-segs');
    expect(html, '失败标记没有文案 —— 读屏软件读出来只是一个符号').toContain('nodes.subUpdateFailed');
    expect(html, '失败标记还是那个裸感叹号').not.toContain('>!<');
    // 与桌面同一句（`NodesTabs.tsx:71`），不是移动端自造的词。
    expect(read(join(DESKTOP_NODES, 'NodesTabs.tsx'))).toContain("t('nodes.subUpdateFailed')");
  });

  it('反向对照：没有失败时分组段上不出现这句（证明上一条不是恒真）', () => {
    expect(render()).not.toContain('nodes.subUpdateFailed');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑮ IA §3.1 第 6 条 —— segmented-tabs 的**选中项滚进视野**。
 *
 * 这条腿驱动不了：段条要真滚起来需要一个带布局的 DOM，而本仓 vitest 是 `environment:'node'`、
 * 刻意不装 jsdom（见文件头）。故走源码分析，并按本文件 ⑪ 的同一套纪律办：
 * 分析器是**纯函数**，先拿 1 好 4 坏的合成夹具证明它既不恒绿也不恒红，再对生产源码断言。
 * 「文件里出现过 scrollIntoView 这个名字」不算判据 —— 它挡不住「写在一个永不执行的地方」。
 * ═════════════════════════════════════════════════════════════════════════ */

interface RevealWiring {
  /** `GroupSegments` 函数体长度（取材面自检：0 = 切片塌了）。 */
  readonly bodyLen: number;
  /** 段按钮上真的挂了 `ref`（没有引用就没有可滚的目标）。 */
  readonly refOnSegButton: boolean;
  /** 收集器用 `g.id` 存、用 `activeTab` 取 —— 两边键空间必须是同一个。 */
  readonly sameKeySpace: boolean;
  /** `useEffect` 的依赖数组原文（`null` = 根本没有 effect）。 */
  readonly effectDeps: string | null;
  /** `scrollIntoView(` 就在那个 effect 的括号区间内，不是在别处。 */
  readonly scrollInEffect: boolean;
}

function analyzeRevealWiring(src: string): RevealWiring {
  const body = strip(src);
  const at = body.indexOf('function GroupSegments');
  const miss: RevealWiring = {
    bodyLen: 0,
    refOnSegButton: false,
    sameKeySpace: false,
    effectDeps: null,
    scrollInEffect: false,
  };
  if (at < 0) return miss;
  /* 函数体的起点是**参数表之后**那个 `{`：直接取 `indexOf('{')` 会落在解构参数
     `GroupSegments({ groups, … })` 的那一对上，切出来的"函数体"只有参数表那么长 ——
     它足够让下面每条断言恒假，也就恒红，而红的理由与真实缺陷无关。先用圆括号深度走到参数表末尾。 */
  let p = body.indexOf('(', at);
  if (p < 0) return miss;
  let pd = 0;
  for (; p < body.length; p += 1) {
    if (body[p] === '(') pd += 1;
    else if (body[p] === ')') {
      pd -= 1;
      if (pd === 0) break;
    }
  }
  const open = body.indexOf('{', p);
  if (open < 0) return miss;
  let depth = 0;
  let end = -1;
  for (let i = open; i < body.length; i += 1) {
    if (body[i] === '{') depth += 1;
    else if (body[i] === '}') {
      depth -= 1;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  if (end < 0) return miss;
  const fn = body.slice(at, end + 1);

  const refOnSegButton = domOpeningTags(fn).some(
    (tag) => tag.name === 'button' && /\bref=/.test(tag.text) && tag.text.includes('mn-seg'),
  );
  const sameKeySpace = /\.set\(\s*g\.id\s*,/.test(fn) && /\.get\(\s*activeTab\s*\)/.test(fn);

  const ue = fn.indexOf('useEffect(');
  if (ue < 0) return { bodyLen: fn.length, refOnSegButton, sameKeySpace, effectDeps: null, scrollInEffect: false };
  let d = 0;
  let j = fn.indexOf('(', ue);
  for (; j < fn.length; j += 1) {
    if (fn[j] === '(') d += 1;
    else if (fn[j] === ')') {
      d -= 1;
      if (d === 0) break;
    }
  }
  const span = fn.slice(ue, j + 1);
  const deps = /\[([^[\]]*)\]\s*\)$/.exec(span);
  return {
    bodyLen: fn.length,
    refOnSegButton,
    sameKeySpace,
    effectDeps: deps === null ? null : deps[1].trim(),
    scrollInEffect: span.includes('.scrollIntoView('),
  };
}

/** 合成夹具：一份能过、四份各坏一处。 */
const REVEAL_FIXTURES: ReadonlyArray<{
  name: string;
  src: string;
  want: Omit<RevealWiring, 'bodyLen'>;
}> = [
  {
    name: '好：ref + 以 activeTab 为依赖的 effect + effect 内 scrollIntoView',
    src: `function GroupSegments({ groups, activeTab }) {
  const segRefs = useRef(new Map());
  useEffect(() => {
    segRefs.current.get(activeTab)?.scrollIntoView({ block: 'nearest' });
  }, [activeTab]);
  return <nav>{groups.map((g) => (
    <button className="mn-seg" ref={(el) => { segRefs.current.set(g.id, el); }} />
  ))}</nav>;
}`,
    want: { refOnSegButton: true, sameKeySpace: true, effectDeps: 'activeTab', scrollInEffect: true },
  },
  {
    name: '坏①：按钮上没有 ref（effect 拿不到任何可滚的目标）',
    src: `function GroupSegments({ groups, activeTab }) {
  const segRefs = useRef(new Map());
  useEffect(() => {
    segRefs.current.get(activeTab)?.scrollIntoView({ block: 'nearest' });
  }, [activeTab]);
  return <nav>{groups.map((g) => (<button className="mn-seg" />))}</nav>;
}`,
    want: { refOnSegButton: false, sameKeySpace: false, effectDeps: 'activeTab', scrollInEffect: true },
  },
  {
    name: '坏②：effect 只跑一次（依赖数组空 ⇒ 切分组后不再滚）',
    src: `function GroupSegments({ groups, activeTab }) {
  const segRefs = useRef(new Map());
  useEffect(() => {
    segRefs.current.get(activeTab)?.scrollIntoView({ block: 'nearest' });
  }, []);
  return <nav>{groups.map((g) => (
    <button className="mn-seg" ref={(el) => { segRefs.current.set(g.id, el); }} />
  ))}</nav>;
}`,
    want: { refOnSegButton: true, sameKeySpace: true, effectDeps: '', scrollInEffect: true },
  },
  {
    name: '坏③：`scrollIntoView` 只出现在点击回调里（进屏 / 恢复分组时不滚）',
    src: `function GroupSegments({ groups, activeTab }) {
  const segRefs = useRef(new Map());
  useEffect(() => { segRefs.current.get(activeTab); }, [activeTab]);
  return <nav>{groups.map((g) => (
    <button className="mn-seg" ref={(el) => { segRefs.current.set(g.id, el); }}
      onClick={() => segRefs.current.get(g.id)?.scrollIntoView({ block: 'nearest' })} />
  ))}</nav>;
}`,
    want: { refOnSegButton: true, sameKeySpace: true, effectDeps: 'activeTab', scrollInEffect: false },
  },
  {
    name: '坏④：根本没有 effect（回到本轮之前的形态）',
    src: `function GroupSegments({ groups, activeTab }) {
  return <nav>{groups.map((g) => (<button className="mn-seg" />))}</nav>;
}`,
    want: { refOnSegButton: false, sameKeySpace: false, effectDeps: null, scrollInEffect: false },
  },
];

describe('⑮ segmented-tabs：选中项滚进视野（§3.1 第 6 条 touch-adapted 的第二项）', () => {
  const wiring = analyzeRevealWiring(read(join(MOBILE, 'nodes', 'NodesScreenView.tsx')));

  it('自检：`GroupSegments` 的函数体真的切出来了（切片塌了会让下面每条恒红/恒绿）', () => {
    expect(wiring.bodyLen, '`GroupSegments` 切不出函数体 —— 改名了？').toBeGreaterThan(300);
  });

  it('分析器自身的行为对照：1 好 4 坏各判成对应形态（证明它不恒绿也不恒红）', () => {
    for (const f of REVEAL_FIXTURES) {
      const got = analyzeRevealWiring(f.src);
      expect(
        { refOnSegButton: got.refOnSegButton, sameKeySpace: got.sameKeySpace, effectDeps: got.effectDeps, scrollInEffect: got.scrollInEffect },
        `${f.name}：分析器判错了`,
      ).toEqual(f.want);
    }
    // 正面：四种坏各自坏在不同的一格（不是「怎么喂都返回同一个值」）。
    expect(new Set(REVEAL_FIXTURES.map((f) => JSON.stringify(f.want))).size).toBe(REVEAL_FIXTURES.length);
  });

  it('生产接线四条腿齐：段按钮挂 ref、键空间同一、effect 依赖 activeTab、滚动就在那个 effect 里', () => {
    expect(wiring.refOnSegButton, '段按钮上没有 ref —— effect 拿不到可滚的目标').toBe(true);
    expect(wiring.sameKeySpace, 'ref 用 `g.id` 存却不是用 `activeTab` 取 —— 两个键空间对不上，永远查不到').toBe(
      true,
    );
    expect(wiring.effectDeps, '没有以 `activeTab` 为依赖的 effect —— 切分组后不会再滚').toContain('activeTab');
    expect(
      wiring.scrollInEffect,
      '`scrollIntoView` 不在那个 effect 里 —— 进屏或恢复上次分组时选中项仍可能停在屏外',
    ).toBe(true);
  });

  it('渲染腿：每颗段都画出来了，且选中的那颗可辨认（`aria-current`）', () => {
    const html = render();
    expect((html.match(/class="mn-seg"/g) ?? []).length, '段没渲染出来 ⇒ 上面的接线断言没有落点').toBe(2);
    expect((html.match(/<button[^>]*class="mn-seg"[^>]*aria-current="true"/g) ?? []).length, '选中的那颗不可辨认').toBe(1);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑯ IA §4.10 —— 测速置灰理由的**单一来源**。
 *
 * 此前页头判了核、行动作面没判：`isSpeedTestable` 里 `caps.mainCorePool` 只对 Tailscale 生效，
 * 于是核停着时一个普通 VLESS 节点在行动作面里仍然可点，点下去换回一条后端红字 ——
 * 而同屏页头已经提前说清了「请先在首页连接」。判据两条腿：
 *   A 行为腿：判据函数本身的真值表（纯函数，可直接驱动）；
 *   B 接线腿：两个消费点都消费**这一个**函数，不许任何一侧就地重写。
 * 只有 A 会被「函数写对了但没人用」骗过（本仓已登记的「抽函数会造出新的缝」）。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑯ 测速置灰理由：判据单一来源（IA §4.10）', () => {
  const tt = (k: string): string => k;

  it('A 腿：真值表 —— 核这一档排在结构可测性之前，且对屏级入口同样成立', () => {
    const testable = { speedTestable: true, speedTestBlockedHint: undefined };
    const blocked = { speedTestable: false, speedTestBlockedHint: 'why.blocked' };
    /* 核停：不论这一行结构上可不可测，理由都是同一句 —— 用户下一步要做的是同一件事。
       文案 2026-09-05 由 `nodes.mobileSpeedTestCoreStopped`（「请先在**首页**连接后再测」）换成
       中性的 `home.stubProxyStopped`（「代理未运行」）：同一张面板里已经有了「连接」那颗，
       再把用户指去首页就是一句假话。旧键随之从五份 locale 里删掉（不新造键是硬约束）。 */
    expect(speedTestBlockedReason(tt, false, testable)).toBe('home.stubProxyStopped');
    expect(speedTestBlockedReason(tt, false, blocked)).toBe('home.stubProxyStopped');
    expect(speedTestBlockedReason(tt, false)).toBe('home.stubProxyStopped');
    // 核在跑：回到结构可测性那一档。
    expect(speedTestBlockedReason(tt, true, testable)).toBeUndefined();
    expect(speedTestBlockedReason(tt, true, blocked)).toBe('why.blocked');
    expect(speedTestBlockedReason(tt, true)).toBeUndefined();
    // 正面对照：三个结果确实互不相同（不是「怎么喂都返回同一个值」）。
    expect(
      new Set([
        String(speedTestBlockedReason(tt, false, testable)),
        String(speedTestBlockedReason(tt, true, testable)),
        String(speedTestBlockedReason(tt, true, blocked)),
      ]).size,
    ).toBe(3);
  });

  /** 接线层 `rowItems` 里 `id: 'speed-test'` 那一项的 `disabledReason` 装的是什么表达式。 */
  const speedTestDisabledExpr = (src: string): string | null => {
    const body = strip(src);
    const at = body.indexOf("id: 'speed-test'");
    if (at < 0) return null;
    const stop = body.indexOf('onSelect', at);
    const seg = body.slice(at, stop < 0 ? at + 400 : stop);
    const m = /disabledReason:\s*([^\n]+)/.exec(seg);
    return m === null ? null : m[1].trim().replace(/,$/, '');
  };

  it('分析器自身的行为对照：好 / 旧形态 / 缺席三种各判对（证明 B 腿不恒绿）', () => {
    const good = `const items = [{ id: 'speed-test', label: t('x'),
      disabledReason: speedTestBlockedReason(t, proxyRunning, row),
      onSelect: () => {} }];`;
    const old = `const items = [{ id: 'speed-test', label: t('x'),
      disabledReason: row.speedTestable ? undefined : row.speedTestBlockedHint,
      onSelect: () => {} }];`;
    const gone = `const items = [{ id: 'speed-test', label: t('x'), onSelect: () => {} }];`;
    expect(speedTestDisabledExpr(good)).toBe('speedTestBlockedReason(t, proxyRunning, row)');
    expect(speedTestDisabledExpr(old)).toBe('row.speedTestable ? undefined : row.speedTestBlockedHint');
    expect(speedTestDisabledExpr(gone)).toBeNull();
    expect(speedTestDisabledExpr('const nothing = 1;')).toBeNull();
  });

  it('B 腿：行动作面的置灰理由就是那个函数算出来的，不是就地重判一遍', () => {
    /* 🔴 取材面 2026-09-06 跟着**代码搬家**改了一次：那张表从接线层的内联 `useCallback` 搬进了
       `view-model.ts#buildRowItems`（理由见那份文件里那段「抽出来是为了让判据够得着」）。
       判据的射程从来是「行动作面那一项的置灰理由」，不是「MobileNodesScreen.tsx 这个文件」——
       写死旧文件名会让本条对着一个不再含那一项的文件恒 null，而 `not.toBeNull()` 会把它报成红，
       那是**假红**，但下一步很可能被人删掉。故这里跟着搬。 */
    const expr = speedTestDisabledExpr(read(join(MOBILE, 'nodes', 'view-model.ts')));
    expect(expr, "行动作面构造器里找不到 `id: 'speed-test'` 那一项 —— 判据面塌了").not.toBeNull();
    expect(
      expr,
      `行动作面的测速项 disabledReason 装的是 **${String(expr)}**。` +
        '只判 `row.speedTestable` 挡不住核停这一档（`isSpeedTestable` 的 `caps.mainCorePool` ' +
        '只对 tailscale 生效）⇒ 核没跑时普通节点照样可点，点下去吃一条后端红字，' +
        '而同屏页头已经说了「请先连接」。',
    ).toMatch(/^speedTestBlockedReason\(/);
  });

  it('B 腿：呈现层的屏级入口消费同一个函数，且这句文案全树只有一处真值', () => {
    expect(strip(read(join(MOBILE, 'nodes', 'NodesScreenView.tsx')))).toContain(
      'speedTestBlockedReason(t, props.coreRunning)',
    );
    // 单一真值：核停这句理由只许出现在判据函数里。第三处入口若再抄一遍，
    // 这条当场红 —— 那正是本条缺陷此前的活法。
    const holders = MOBILE_SRC.map(rel).filter((f) =>
      strip(read(join(SRC, f))).includes("'home.stubProxyStopped'"),
    );
    expect(holders, '核停这句理由被抄到了不止一处 —— 下一次只会有一处跟着改').toEqual([
      'mobile/nodes/view-model.ts',
    ]);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑯-b IA §4.10 的第三件事 —— “plus an action that connects”（K3-RV03）。
 *
 * 上一轮这条记的是**待裁决**：真正卡住它的不是接线（`withProxyStartClaim` 与 `api` 都是共享模块），
 * 是**用户可见文案** —— 核停这一档的理由五语种写的都是「请先在**首页**连接后再测」，
 * 在同一张面板里再放一颗「连接」会让那句话当场变假。2026-09-05 裁定：做，但**只复用已有的
 * 五语种键，不新造键**。故理由改指中性的 `home.stubProxyStopped`，旧键从五份 locale 里删掉。
 *
 * 三条腿一起钉，缺任一条这颗按钮就是「看着在、按了没用」或「屏幕自相矛盾」：
 *   1. **在场**：核停时出现、核在跑时不出现（后者不是省略，是「已连接的屏上不该有连接键」）；
 *   2. **接线**：与首页**同一条**起核腿（`withProxyStartClaim(() => startProxy())`），
 *      不是就地重写一份 —— 不认领的话提权门那两码会被事件腿再报一遍；
 *   3. **文案**：标签与理由都取自**已存在**的键，且理由里不许再指向首页（读 locale 原文判，
 *      不是从机制反推 —— 承诺的真值在用户看见的那句话里）。
 * ═════════════════════════════════════════════════════════════════════════ */

/**
 * 包住 `needle` 的那一对花括号之间的**对象字面量本体**（含两端花括号）。
 *
 * 2026-09-05 MAJ-8：⑯-b 的三条腿此前全是「这个文件里出现过这串字符」，
 * 在场腿只判 `id: 'connect'` 前 260 字符匹配守卫形态 —— 三条之间的缝正是
 * 「这颗按钮的 onSelect 到底挂没挂 `connectFromRow`」。实测把它改成
 * `onSelect: () => void connectFromRow`（按钮照旧在场、标签对、守卫对、变量仍被 deps 引用）
 * ⇒ tsc rc=0、全量 rc=0，而核停时按下去永远不起核。
 * 形态抄同批 `home-screen.test.tsx` 的 `jsxAttrExpr`：**花括号配平**切出本体，切不出来就抛
 *（空串会让下面每条 `toContain` 恒绿）。
 */
function objectLiteralAt(src: string, needle: string, what: string): string {
  const at = src.indexOf(needle);
  if (at < 0) throw new Error(`${what}：源码里找不到 \`${needle}\` —— 结构变了？`);
  let depth = 0;
  let open = -1;
  for (let i = at; i >= 0; i -= 1) {
    if (src[i] === '}') depth += 1;
    else if (src[i] === '{') {
      if (depth === 0) {
        open = i;
        break;
      }
      depth -= 1;
    }
  }
  if (open < 0) throw new Error(`${what}：\`${needle}\` 不在任何对象字面量里 —— 结构变了？`);
  depth = 0;
  for (let i = open; i < src.length; i += 1) {
    if (src[i] === '{') depth += 1;
    else if (src[i] === '}') {
      depth -= 1;
      if (depth === 0) return src.slice(open, i + 1);
    }
  }
  throw new Error(`${what}：对象字面量没有收口 —— 切法失效`);
}

/** `const <name> = useCallback(` 到**圆括号配平**处的函数体（含实参列表）。切不出来就抛。 */
function callbackBody(src: string, name: string, what: string): string {
  const head = `const ${name} = useCallback(`;
  const at = src.indexOf(head);
  if (at < 0) throw new Error(`${what}：源码里找不到 \`${head}\` —— 结构变了？`);
  const open = at + head.length - 1;
  let depth = 0;
  for (let i = open; i < src.length; i += 1) {
    if (src[i] === '(') depth += 1;
    else if (src[i] === ')') {
      depth -= 1;
      if (depth === 0) return src.slice(open + 1, i);
    }
  }
  throw new Error(`${what}：\`${name}\` 的 useCallback 没有收口 —— 切法失效`);
}

describe('⑯-b 核停时行动作面里有「连接」，且文案与首页不打架（K3-RV03）', () => {
  /*
   * 🔴 **取材面 2026-09-06 一分为二（批 2）。** 行动作那张表搬进了 `view-model.ts#buildRowItems`
   * （抽出来是为了让 ⑥ 那条判据够得着生产的表），而起核那条腿 `connectFromRow` 仍住在接线层 ——
   * 于是「这颗按钮按下去会起核」这句话现在**跨两个文件**，中间多了一道缝：
   *   构造器里 `onSelect: handlers.onConnect` ←→ 接线层里 `onConnect: connectFromRow`。
   * 两侧各判一条、**成对交**才等于原来那一条；只判任一侧都留着那道缝
   *（构造器对了而接线层挂了个空函数 / 接线层对了而构造器压根没把它接出去）。
   */
  const BUILDER = strip(read(join(MOBILE, 'nodes', 'view-model.ts')));
  const WIRING = strip(read(join(MOBILE, 'nodes', 'MobileNodesScreen.tsx')));
  const LOCALES = ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa'] as const;
  const copy = (loc: string, key: string): unknown =>
    key
      .split('.')
      .reduce<unknown>(
        (o, k) => (typeof o === 'object' && o !== null ? (o as Record<string, unknown>)[k] : undefined),
        JSON.parse(read(join(SRC, 'i18n', 'locales', `${loc}.json`))),
      );

  const GUARD = /\.\.\.\(\s*coreRunning\s*\?\s*\[\]\s*:\s*\[\{\s*$/;

  it('在场腿①（构造器）：核停时那颗动作在场、核在跑时不在，且它的 onSelect 真的接出去了', () => {
    const at = BUILDER.indexOf("id: 'connect'");
    expect(at, "行动作构造器里找不到 `id: 'connect'` 那一项 —— §4.10 的第三件事又不见了").toBeGreaterThan(0);
    // 分支：它挂在 `coreRunning ? [] : [...]` 上。恒在场（忘了判核）与恒缺席都会让这条红。
    const guard = BUILDER.slice(Math.max(0, at - 260), at);
    expect(
      guard,
      "`id: 'connect'` 那一项没挂在 `coreRunning` 的分支上 —— 要么核在跑时也画出来（噪音），" +
        '要么核停时也不画（回到缺席）',
    ).toMatch(GUARD);
    // 自检：同一套读法对着「没有守卫」与「守反了」两种形态都必须报得出来（否则它恒绿）。
    const cut = (src: string) => {
      const i = src.indexOf("id: 'connect'");
      return src.slice(Math.max(0, i - 260), i);
    };
    expect(GUARD.test(cut("const items = [{ id: 'connect', label: x }];"))).toBe(false);
    expect(GUARD.test(cut("const items = [...(coreRunning ? [{ id: 'connect' }] : [])];"))).toBe(false);
    // 切出那颗按钮的对象字面量**本体**，断言它自己的 `onSelect` 接的是入参里的那条腿。
    // 「文件里出现过 handlers.onConnect」不算数。
    const item = objectLiteralAt(BUILDER, "id: 'connect'", '连接动作那一项');
    expect(item, '切出来的不是那颗按钮的对象字面量 —— 切法失效').toContain("id: 'connect'");
    expect(
      item,
      '「连接」这颗按钮的 `onSelect` 没有接到 `handlers.onConnect` —— 按钮在场、标签对、守卫对，' +
        '按下去却什么都不发生',
    ).toMatch(/onSelect:\s*handlers\.onConnect\b/);
    const mutated = objectLiteralAt(
      "const a = [{ id: 'connect', label: x, onSelect: () => void handlers.onConnect }];",
      "id: 'connect'",
      '自检',
    );
    expect(/onSelect:\s*handlers\.onConnect\b/.test(mutated)).toBe(false);
    expect(() => objectLiteralAt('const a = 1;', "id: 'connect'", '自检')).toThrow();
    expect(() => objectLiteralAt("id: 'connect'", "id: 'connect'", '自检')).toThrow();
    // 🔴 **射程声明**：本条判的是**源码接线**。行动作面要选中某一行之后才展开，本文件的
    // 渲染夹具（`NodesScreenView` 直渲）够不到那一层 DOM ⇒ 「这颗按钮在真机上按下去会起核」
    // 这一跳没有判据，归真机验收。
  });

  it('在场腿②（接线层，与①成对）：交给构造器的 `onConnect` 就是那条起核腿', () => {
    /* 抽函数造出的那道缝落在这里：构造器把 `handlers.onConnect` 接了出去，而**接的是什么**
       由接线层决定。切出 `handlers:` 那个对象字面量本体再判，「文件里出现过 connectFromRow」
       不算数 —— 那串字符在 `useCallback` 的 deps 数组里也有一份。 */
    const handlers = objectLiteralAt(WIRING, 'onConnect:', '行动作的 handlers 对象');
    expect(handlers, '切出来的不是 handlers 对象 —— 切法失效').toContain('onSpeedTest:');
    expect(
      handlers,
      '`onConnect` 没有挂 `connectFromRow` —— 构造器把它接出去了，接线层却给了别的东西',
    ).toMatch(/onConnect:\s*connectFromRow\b/);
    // 反向自检：挂成一个不起核的包装时报得出来。
    expect(
      /onConnect:\s*connectFromRow\b/.test('{ onSpeedTest: a, onConnect: () => void connectFromRow }'),
    ).toBe(false);
  });

  it('接线腿：与首页**同一条**起核腿，不是就地重写（认领腿丢了会双报提权失败）', () => {
    // 切出 `connectFromRow` 的**函数体**再判：此前是整份文件 `toContain`，
    // 那串字符出现在任何地方（哪怕另一个未被引用的函数里）都算数（2026-09-05 MAJ-8）。
    const body = callbackBody(WIRING, 'connectFromRow', '`connectFromRow` 的函数体');
    expect(body.length, '`connectFromRow` 的函数体没切出来 —— 判据面塌了').toBeGreaterThan(40);
    expect(
      body,
      '起核没走 `withProxyStartClaim` —— 提权门那两码后端是双出口，不认领会被事件腿再报一遍',
    ).toContain('withProxyStartClaim(() => startProxy())');
    // 反向自检：切法对「这串字符只在函数体**外面**」报得出来，对「函数没了」抛。
    const decoy = callbackBody(
      'const other = () => withProxyStartClaim(() => startProxy());\n' +
        'const connectFromRow = useCallback(() => { void runWrite(() => startProxy()); }, []);',
      'connectFromRow',
      '自检',
    );
    expect(decoy.includes('withProxyStartClaim')).toBe(false);
    expect(() => callbackBody('const a = 1;', 'connectFromRow', '自检')).toThrow();
    // 首页那条腿逐字相同：两处分叉的话，下一次只会有一处跟着改。
    expect(
      strip(read(join(MOBILE, 'home', 'MobileHomeScreen.tsx'))),
      '首页那条起核腿改写法了 —— 本屏这条要同批跟上',
    ).toContain('withProxyStartClaim(() => startProxy())');
  });

  it('文案腿：标签与进行态都复用已有键，且理由**不再指向首页**（读 locale 原文判）', () => {
    // 标签 = 首页那颗连接键；进行态 = 首页那档「启动中」。两个都是既有键，本轮一个新键都没造。
    expect(BUILDER, '连接动作的标签没复用首页那颗的键').toContain("label: t('mobileHome.connect')");
    expect(WIRING, '起核那几秒屏上没有任何回显 —— 用户会以为没点着').toContain(
      "text: t('home.statusStarting')",
    );
    for (const key of ['mobileHome.connect', 'home.statusStarting', 'home.stubProxyStopped'])
      for (const loc of LOCALES)
        expect(copy(loc, key), `${loc} 缺键 ${key} —— 复用的键必须五语种齐备`).toBeTruthy();
    // 反面：旧那条「请先在首页连接后再测」必须真的没了（留着 = 同一屏上两句互相拆台的话）。
    for (const loc of LOCALES)
      expect(
        copy(loc, 'nodes.mobileSpeedTestCoreStopped'),
        `${loc} 里 \`nodes.mobileSpeedTestCoreStopped\` 还在 —— 它把用户指去首页，` +
          '而按钮就在同一张面板上',
      ).toBeUndefined();
    // 正面：换上的这句在中文里确实**不含**「首页」，且它确实在说「没运行」。
    const zh = String(copy('zh-CN', 'home.stubProxyStopped'));
    expect(zh, '理由文案又指回首页了').not.toContain('首页');
    expect(zh, '理由文案不再是在说「核没跑」').toMatch(/未运行/);
    // 自检：同一套读法对着一个已知含「首页」的键确实报得出来（否则上面那条否定断言没有信息量）。
    expect(String(copy('zh-CN', 'settings.coreVersion.sameVersionGoManage'))).toContain('请到');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑰ 断点档位的**值**：仅够每列阅读时才分栏，行高仍由内容决定。
 *
 * 与 `styles/cascade-dead-rules.test.ts` 分工：那道门是全仓的，只判「断点块有没有机会胜出」
 * （顺序），判不了「值还在不在」—— 把整条 expanded 规则删掉，它照样绿。本组补的正是另一半：
 * 值在场 + 顺序正确，两条合起来才等于「宽屏分栏且触控目标不小于 48px」。
 *
 * 此前这一层一条门都没有：把 `column-gap` 退回 `var(--section-gap)`、把 expanded 行高整条删掉，
 * 全量测试零红。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑰ nodes.css 的断点档位落地（阅读宽度与触控下限）', () => {
  /** 剥掉块注释再取材：注释里的示例规则不算数。**保长度** ⇒ 下面的偏移量比较仍对得上原文。 */
  const cssOf = (): string =>
    read(join(MOBILE, 'nodes', 'nodes.css')).replace(/\/\*[\s\S]*?\*\//g, (m) =>
      m.replace(/[^\n]/g, ' '),
    );

  /**
   * 同一个条件的容器查询块在本文件里**有好几个**（列表、页头、行各写各的）。
   * 所以这里收**全部**，不是首条命中即返回 —— 「取首条」正是上一轮那批 `declsOf` 式判据
   * 抓不到层叠回归的原因。
   */
  const containerBlocks = (css: string, em: string): { at: number; body: string }[] => {
    const head = new RegExp(`@container\\s+mscreen\\s*\\(\\s*min-width:\\s*${em}em\\s*\\)\\s*\\{`, 'g');
    const out: { at: number; body: string }[] = [];
    for (const m of css.matchAll(head)) {
      const open = m.index + m[0].length;
      let depth = 1;
      let i = open;
      for (; i < css.length && depth > 0; i += 1) {
        if (css[i] === '{') depth += 1;
        else if (css[i] === '}') depth -= 1;
      }
      expect(depth, `${em}em 块（起于 ${m.index}）没有闭合 —— 切片塌了`).toBe(0);
      out.push({ at: m.index, body: css.slice(open, i - 1) });
    }
    expect(out.length, `nodes.css 里没有 ${em}em 的容器查询块 —— 取材面塌了，下面每条恒绿`).toBeGreaterThan(0);
    return out;
  };

  /** 声明了 `<selector> { … <decl> … }` 的那些块。返回空 = 这一档没写这条规则。 */
  const blocksDeclaring = (css: string, em: string, rule: RegExp): { at: number; body: string }[] =>
    containerBlocks(css, em).filter((b) => rule.test(b.body));

  it('节点行保持 48px 触控下限，宽屏由内容决定高度而非强制撑成 64px', () => {
    const css = cssOf();
    const base = /\.mn-row-main\s*\{[^}]*min-height:\s*var\(--tap-min\)/.exec(css);
    expect(base, '`.mn-row-main` 丢失 48px 触控下限').not.toBeNull();
    const carriers = blocksDeclaring(css, '52\\.5', /\.mn-row-main\s*\{[^}]*min-height:\s*64px/);
    expect(carriers, '宽屏又以固定高拉大了节点行').toEqual([]);
  });

  it('600–839 单列，840 起按 DOM 行序两列且中缝 24px；大字号按 em 阈值回退', () => {
    const css = cssOf();
    expect(blocksDeclaring(css, '37\\.5', /\.mn-list\s*\{[^}]*grid-template-columns:\s*repeat\(2/)).toEqual([]);
    expect(blocksDeclaring(css, '52\\.5', /\.mn-list\s*\{[^}]*grid-template-columns:\s*repeat\(2,\s*minmax\(0,\s*1fr\)\)[^}]*column-gap:\s*24px/).length).toBe(1);
    // 反向：段间距标量退回来就红 —— 那是纵向块距，不是多列中缝，两者数值不同源。
    expect(css, '列间距又退回 `--section-gap`（段间距），两屏的栏距会重新对不上').not.toMatch(
      /\.mn-list\s*\{[^}]*column-gap:\s*var\(--section-gap\)/,
    );
  });

  it('浏览器实算：共享 MobileSelect 的可见 trigger 在窄屏隐藏、宽屏显示', async () => {
    const asks = [
      { select: 'button.mn-from-medium.m-select-trigger', props: ['display'] },
      { select: 'button.mn-only-expanded.m-select-trigger', props: ['display'] },
      { select: '.mn-until-expanded', props: ['display'] },
      { select: '.mn-row', props: ['display'], many: 'all' as const },
    ];
    const html = inMobileShell(render(), 'nodes');
    const narrow = await measure({ ctx: 'mobile', html, viewport: { width: 390, height: 844 } }, asks);
    expect(narrow.get('button.mn-from-medium.m-select-trigger', 'display')).toBe('none');
    expect(narrow.get('button.mn-only-expanded.m-select-trigger', 'display')).toBe('none');
    expect(narrow.get('.mn-until-expanded', 'display')).not.toBe('none');
    const narrowRows = narrow.rectAll('.mn-row');
    expect(narrowRows[0].x).toBeCloseTo(narrowRows[1].x, 0);
    expect(narrowRows[1].y).toBeGreaterThan(narrowRows[0].y);
    const wide = await measure({ ctx: 'mobile', html, viewport: { width: 980, height: 740 } }, asks);
    expect(wide.get('button.mn-from-medium.m-select-trigger', 'display')).toBe('flex');
    expect(wide.get('button.mn-only-expanded.m-select-trigger', 'display')).toBe('flex');
    expect(wide.get('.mn-until-expanded', 'display')).toBe('none');
    const wideRows = wide.rectAll('.mn-row');
    expect(wideRows[1].x).toBeCloseTo(wideRows[0].x, 0);
    expect(wideRows[1].y).toBeGreaterThan(wideRows[0].y);
  });
});

// ─────────────────────────────────────────────────────────────────────────────

/**
 * ⑱ **浏览器裁判**：延迟五档与角标族的角色色由 Chrome 算，不由静态模型算。
 *
 * ⑫ 组里那几条 `won('.mn-lat.fast').toContain('hsl(var(--ok))')` 守的是「源码里写着哪个令牌名」。
 * 它挡不住两类真缺陷，两类都是 2026-09-05 第四轮实测过的：
 *  · **跨选择器压制**（R5-01）：`.mn-row .mn-lat{ color: hsl(var(--err)) }` —— `.mn-lat` 确实住在
 *    `.mn-row` 里（`NodesScreenView.tsx:244`/`:260`），特异度更高必胜，Chrome 152 里 fast 档变红，
 *    而静态模型把这一族声明为契约外、`narrower` 桶一句话都不说；
 *  · **令牌的值被覆写**（R5-07）：`:root{ --ok: 0 84% 60% }` —— 源码里 `hsl(var(--ok))` 一个字没变，
 *    渲染出来是红的。静态模型只比字面量，全线无门。
 *
 * 本组把判据换成「浏览器对**真组件 DOM** 算出来的 computed 值」，并且比的是**两个 rgb**：
 * 目标元素的色 vs 同一条链上 `hsl(var(--ok))` 探针算出来的色。令牌调色不误伤，角色对调必红。
 * 祖先链存不存在也由真 DOM 说了算 —— 那本来就不该由人在评审里断言。
 */
describe('⑱ 浏览器裁判：行内颜色角色（真 DOM + 整条移动端 CSS 链）', () => {
  // 收尾走 CDP 的 `Browser.close`，让 Chrome 自己退（全程不 kill 进程）。
  // 超时给到 30s：全量并行跑时机器争用，默认的 10s hookTimeout 会在收尾这一步假红。
  afterAll(closeOracle, 30_000);

  /** 五档 → 角色令牌。与 `nodes.css:527-554` 同源。 */
  const LAT_ROLE = {
    fast: '--ok',
    mid: '--flow-hi',
    slow: '--warn',
    dead: '--err',
    none: '--fg-faint',
  } as const;

  /**
   * 五档 → **用户看得见的色族**。
   *
   * 与上面那条「对拍探针」是两条互补的腿，缺一不可：
   *  · 探针腿答「这一档拿到的是不是**该角色**的令牌」—— 挡跨选择器压制（R5-01）、挡角色对调；
   *    但令牌自己的值被改写时（R5-07：`:root{--ok: 0 84% 60%}`）两边一起变，它看不见。
   *  · 色族腿答「用户看到的是不是绿/蓝/琥珀/红/中性」—— 那才是这五档存在的理由。
   *    令牌在同一族内调色不误伤，把 `--ok` 改成红的当场红。
   */
  const LAT_FAMILY: Readonly<Record<keyof typeof LAT_ROLE, (c: readonly [number, number, number]) => boolean>> = {
    fast: ([r, g, b]) => g > r && g > b, // status.success —— 绿
    mid: ([r, g, b]) => g > r && b > r, // interactive/info —— 青绿
    slow: ([r, g, b]) => r > g && g > b, // status.warning —— 琥珀
    dead: ([r, g, b]) => r > g && r > b, // status.danger —— 红
    // `none` = 没测过 / 陈旧 / 结构上不可测：**不许**染成任何语义色（`nodes.css:550-554`）。
    none: (c) => spread(c) <= NEUTRAL_SPREAD,
  };
  /** 中性档允许的最大通道极差。四个语义档在两条主题腿上都远超它（见下面的正向对照）。 */
  const NEUTRAL_SPREAD = 48;
  const spread = (c: readonly [number, number, number]) => Math.max(...c) - Math.min(...c);
  const rgb = (v: string): readonly [number, number, number] => {
    const m = /^rgba?\((\d+),\s*(\d+),\s*(\d+)/.exec(v);
    if (m === null) throw new Error(`不是 rgb 记号，读不出通道：${v}`);
    return [Number(m[1]), Number(m[2]), Number(m[3])];
  };

  /** 一屏里同时摆出五档 + 三种角标，让一次渲染答完全部问题。 */
  const paintedHtml = (): string =>
    render({
      rows: [
        row('fast', { latencyMs: 40 }),
        row('mid', { latencyMs: 120 }),
        row('slow', { latencyMs: 260 }),
        row('dead', { latencyMs: 900 }),
        row('stale', { latencyMs: 42, latencyStale: true }),
        // `exit` 与 `cur` 互斥（`NodesScreenView.tsx:265-268`），故各摆一行。
        row('exit', { latencyMs: 40, isExit: true, isCurrent: false }),
        row('cur', { latencyMs: 40, isCurrent: true, lanOnly: true }),
      ],
    });

  const PROBES = {
    '--ok': 'hsl(var(--ok))',
    '--ok-weak': 'hsl(var(--ok-weak))',
    '--flow': 'hsl(var(--flow))',
    '--flow-hi': 'hsl(var(--flow-hi))',
    '--flow-weak': 'hsl(var(--flow-weak))',
    '--aurora-hi': 'hsl(var(--aurora-hi))',
    '--aurora-weak': 'hsl(var(--aurora-weak))',
    '--warn': 'hsl(var(--warn))',
    '--warn-weak': 'hsl(var(--warn-weak))',
    '--err': 'hsl(var(--err))',
    '--fg-faint': 'hsl(var(--fg-faint))',
  } as const;

  /**
   * 一屏里同一档可能有**好几行**（`fast` 档就有 fast/exit/cur 三行，`proto` 角标七行都有），
   * 故一律 `many: 'all'`：断言吃的是全集，不是第一个（第五轮 M02 —— 上一版只读第一个，
   * 中间那行改坏了门照绿）。
   */
  const LAT_ASKS = (Object.keys(LAT_ROLE) as (keyof typeof LAT_ROLE)[]).map((cls) => ({
    select: `.mn-lat.${cls}`,
    props: ['color'] as const,
    many: 'all' as const,
  }));
  const PILL_ASKS = ['proto', 'exit', 'cur', 'warn'].map((v) => ({
    select: `.mn-pill.${v}`,
    props: ['color', 'background-color'] as const,
    many: 'all' as const,
  }));

  /** 两条主题腿各量一次 —— 断言若只在深色下成立，那它守的不是角色而是某个具体色。 */
  const paint = (theme: 'dark' | 'light') =>
    measure(
      {
        ctx: 'mobile',
        // 真挂载树：`#root > .m-shell > .m-scroll > .m-page > 屏`。屏根之上那几层曾经整段缺席，
        // 于是 `.m-page .mn-lat{ … }` 这条真回归在门底下全绿（第五轮 B01）。
        html: inMobileShell(paintedHtml(), 'nodes'),
        rootAttrs: { 'data-theme': theme },
        // 🔴 系统偏好**显式钉住**，且与这条腿的主题配对（第六轮 A3）。不钉它 = 由跑门那台机器的
        // 系统主题决定 —— 产品 CSS 里 `@media (prefers-color-scheme:dark){ :root:not([data-theme="light"]) … }`
        // 这一族在 `data-theme="dark"` 下照样命中，于是同一个 commit 换台机器就换个结论。
        media: { 'prefers-color-scheme': theme },
        viewport: { width: 390, height: 844 },
        probes: PROBES,
      },
      [
        ...LAT_ASKS,
        ...PILL_ASKS,
        // 祖先链本身也是一条判据：`.mn-lat` 到底住不住在 `.mn-row` 里，由真 DOM 答。
        { select: '.mn-row .mn-lat.fast', props: ['color'] as const, many: 'all' as const },
      ],
    );

  it('自检：五档与三颗角标都真的渲染出来了，且中性档的阈值两个方向都有牙', async () => {
    const m = await paint('dark');
    for (const cls of Object.keys(LAT_ROLE)) expect(m.count(`.mn-lat.${cls}`)).toBeGreaterThan(0);
    // `.mn-lat` 住在 `.mn-row` 里 —— R5-01 那条变异的前提，由真 DOM 答，不由评审断言。
    expect(m.count('.mn-row .mn-lat.fast'), '`.mn-lat` 不在 `.mn-row` 子树里了').toBeGreaterThan(0);
    // 五个角色探针两两不同 —— 下面每条对拍才有分辨力。
    const roles = ['--ok', '--flow-hi', '--warn', '--err', '--fg-faint'] as const;
    expect(new Set(roles.map((r) => m.probe(r))).size, '五个角色色里有重色 —— 五档的对拍没有分辨力').toBe(5);
    // 正向对照：中性阈值不是「谁都过得去」—— 四个语义档在两条主题腿上都远超它。
    for (const theme of ['dark', 'light'] as const) {
      const t = theme === 'dark' ? m : await paint('light');
      for (const cls of ['fast', 'mid', 'slow', 'dead'] as const)
        for (const v of t.getAll(`.mn-lat.${cls}`, 'color'))
          expect(
            spread(rgb(v)),
            `${theme} 下 .mn-lat.${cls} 的通道极差没超过中性阈值 ${NEUTRAL_SPREAD} —— 中性判据失去分辨力`,
          ).toBeGreaterThan(NEUTRAL_SPREAD);
    }
  }, 30_000);

  for (const theme of ['dark', 'light'] as const) {
    it(`延迟五档在 ${theme} 下逐档落到自己的角色令牌（Chrome 算出的 rgb 对拍探针）`, async () => {
      const m = await paint(theme);
      for (const [cls, token] of Object.entries(LAT_ROLE)) {
        const seen = m.getAll(`.mn-lat.${cls}`, 'color');
        expect(seen.length, `.mn-lat.${cls} 一个都没渲染 ⇒ 下面这条恒真`).toBeGreaterThan(0);
        for (const v of seen)
          expect(
            v,
            `.mn-lat.${cls} 在浏览器里算出来不是 ${token} —— ` +
              '这一档的规则被更高特异度的选择器压了（`.mn-row .mn-lat` 那一族），或者被条件档覆写了。' +
              '静态模型对这两类都看不见。',
          ).toBe(m.probe(token));
      }
    }, 30_000);

    it(`延迟五档在 ${theme} 下落在**用户看得见的色族**里（令牌的值被改写时这条红）`, async () => {
      const m = await paint(theme);
      for (const cls of Object.keys(LAT_ROLE) as (keyof typeof LAT_ROLE)[])
        for (const v of m.getAll(`.mn-lat.${cls}`, 'color'))
          expect(
            LAT_FAMILY[cls](rgb(v)),
            `.mn-lat.${cls} 渲染出来是 ${v} —— 不在这一档该有的色族里。` +
              '（R5-07 的形状：`:root{ --ok: 0 84% 60% }` 一句话就能让 fast 档变红，' +
              '而源码里 `hsl(var(--ok))` 一个字没变、对拍探针也跟着一起变。）',
          ).toBe(true);
    }, 30_000);
  }

  it('角标族逐颗落到自己的角色色，已选择不冒充连接成功', async () => {
    const m = await paint('dark');
    for (const [variant, fg, bg] of [
      ['proto', '--flow-hi', '--flow-weak'],
      ['exit', '--aurora-hi', '--aurora-weak'],
      ['cur', '--flow-hi', '--flow-weak'],
      ['warn', '--warn', '--warn-weak'],
    ] as const) {
      const fgs = m.getAll(`.mn-pill.${variant}`, 'color');
      expect(fgs.length, `.mn-pill.${variant} 一颗都没渲染 ⇒ 下面这条恒真`).toBeGreaterThan(0);
      for (const v of fgs) expect(v, `.mn-pill.${variant} 的字色不是 ${fg}`).toBe(m.probe(fg));
      for (const v of m.getAll(`.mn-pill.${variant}`, 'background-color'))
        expect(v, `.mn-pill.${variant} 的底色不是 ${bg}`).toBe(m.probe(bg));
    }
    for (const v of m.getAll('.mn-pill.cur', 'color')) expect(v).not.toBe(m.probe('--ok'));
  }, 30_000);

  /**
   * 角标**底色**的色族腿（第六轮 A7）。
   *
   * 上一版角标底色只有一条腿：`toBe(m.probe(bg))`。而探针曾与被判元素同住 `<body>` 底下 ⇒
   * 令牌被改在 `body`（或更外层）上时两边一起动，那条对拍恒真。实测
   * `body{ --ok-weak: 0 0% 100% }` ⇒ 全量全绿，而浏览器实算的「当前出口」角标背景已经是纯白。
   * 探针已经搬进 `<head>`（`body` 及以下的覆写动不了它），这里再补上**用户看得见的那一面**：
   * 底色与字色**主导通道相同**（同一个色族），且**不是中性**。纯白 / 纯灰当场红。
   */
  const dominant = (c: readonly [number, number, number]): 'r' | 'g' | 'b' => {
    const max = Math.max(...c);
    return c[0] === max ? 'r' : c[1] === max ? 'g' : 'b';
  };
  /** 轻底色仍须保留可见色相；新主题浅色 proto 弱底的极差是 9。 */
  const PILL_BG_SPREAD = 7;

  for (const theme of ['dark', 'light'] as const) {
    it(`角标底色在 ${theme} 下与字色**同族**且不是中性（令牌被就地改成白/灰时这条红）`, async () => {
      const m = await paint(theme);
      for (const variant of ['proto', 'exit', 'cur', 'warn'] as const) {
        const fgs = m.getAll(`.mn-pill.${variant}`, 'color');
        const bgs = m.getAll(`.mn-pill.${variant}`, 'background-color');
        expect(bgs.length, `.mn-pill.${variant} 一颗都没渲染 ⇒ 下面这两条恒真`).toBeGreaterThan(0);
        for (const [i, bg] of bgs.entries()) {
          const c = rgb(bg);
          expect(
            spread(c),
            `.mn-pill.${variant} 的底色是 ${bg} —— 通道极差没超过 ${PILL_BG_SPREAD}，` +
              '用户看到的是一块中性色块，读不出这是哪一档（`--*-weak` 被就地改成白/灰的形状）',
          ).toBeGreaterThan(PILL_BG_SPREAD);
          expect(
            dominant(c),
            `.mn-pill.${variant} 的底色 ${bg} 与字色 ${fgs[i] ?? fgs[0]} 不在同一个色族里`,
          ).toBe(dominant(rgb(fgs[i] ?? fgs[0])));
        }
      }
    }, 30_000);
  }
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑮ 无效节点那一格的**中间一跳**（批 18 接上写入方之后补的门）。
 *
 * 这条链路今天由三段判据分别守着，而**两段之间的缝正好是生产路径**：
 *  · 后端事件 → `store.invalidNodes`：`../app-wiring.test.tsx` ④b（行为，喂假 IPC 真跑）；
 *  · `row.invalidReason` → 常驻 `.mn-note`：本文件 ⑦ 那条「四种异常态各一条 `.mn-note`」（真渲染）；
 *  · **中间这一跳** —— 接线层把 `invalidIndex[server.id]` 翻成 `row.invalidReason` —— 此前没有门。
 *    把它删掉，上面两段照样全绿，而「这个节点已失效」从此永远不亮（那正是批 18 之前的现象，
 *    只不过当时断在第一段）。本组补的就是这一跳。
 *
 * 🔴 射程自曝：本组是**源码级**断言（接线层的 VM 构造住在 `useMemo` 里，node 环境下够不着）。
 * 它证明那条腿写在源码里，不证明它在真机上跑通 —— 后者归真机验收。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑮ 无效节点：接线层真的把剔除理由喂进了行 VM（两扇门之间的那条缝）', () => {
  const WIRING = strip(read(join(MOBILE, 'nodes', 'MobileNodesScreen.tsx')));

  it('`invalidReason` 由 `invalidIndex[server.id]` 过 `invalidNodeReasonText` 得来', () => {
    const at = WIRING.indexOf('invalidReason:');
    expect(at, '行 VM 上没有 `invalidReason` 这一格 —— 剔除理由喂不进呈现层').toBeGreaterThan(-1);
    const leg = WIRING.slice(at, at + 300);
    expect(leg, '没按节点 id 取剔除理由 ⇒ 每一行拿到的都是同一个（或恒 undefined）').toContain(
      'invalidIndex[server.id]',
    );
    // **必须过翻译腿**：直接把 token 喂给呈现层，用户看到的是 `detour-cascade` 这串标识符，
    // 五个语种一视同仁地看不懂 —— 那正是 `domain/invalid-node-reason.ts` 存在的理由。
    expect(leg, '剔除原因没过 `invalidNodeReasonText` ⇒ 机器 token 会被原样渲染给用户').toContain(
      'invalidNodeReasonText(',
    );
  });

  it('索引由 `invalidNodeIndex(invalidNodes)` 算出，且 `invalidNodes` 真的读的是 store', () => {
    expect(WIRING, '没有 `invalidNodeIndex` 这一跳').toContain('invalidNodeIndex(invalidNodes)');
    expect(
      WIRING,
      '`invalidNodes` 不是从 store 读的 —— 那条订阅写进去的表就没人取',
    ).toContain('useAppStore((s) => s.invalidNodes)');
  });
});

describe('mobile consequence badges with the real two-action row width', () => {
  for (const locale of ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa'] as const) {
    it(`${locale}: complete consequences fit the row and wrap within two lines`, async () => {
      const catalog = JSON.parse(read(join(SRC, 'i18n', 'locales', `${locale}.json`))) as Record<string, unknown>;
      const translated: NodesScreenViewProps['t'] = (key) => {
        const value = key.split('.').reduce<unknown>((part, child) => typeof part === 'object' && part !== null ? (part as Record<string, unknown>)[child] : undefined, catalog);
        return typeof value === 'string' ? value : key;
      };
      const html = render({ t: translated, rows: [row('help-status', {
        stagedOnly: true, speedTestable: false, meshRouteReport: meshReport,
      })] });
      expect(html).toContain(translated('mobileHelp.stagedNode'));
      expect(html).toContain(translated('mobileMeshRouteEvidence.summary.preview'));
      const m = await measure({ ctx: 'mobile', html: inMobileShell(html, 'nodes'), viewport: { width: 390, height: 844 } }, [
        { select: '.mn-row-main', props: ['width'] },
        { select: '.mn-row-actions', props: ['width'] },
        { select: '.mn-pill.warn', props: ['line-height', 'padding-top', 'padding-bottom', 'border-top-width', 'border-bottom-width'], many: 'all' },
      ]);
      const main = m.rect('.mn-row-main');
      expect(m.rect('.mn-row-actions').width).toBe(88);
      for (const [index, badge] of m.rectAll('.mn-pill.warn').entries()) {
        expect(badge.x).toBeGreaterThanOrEqual(main.x);
        expect(badge.x + badge.width).toBeLessThanOrEqual(main.x + main.width + .5);
        const sum = ['padding-top', 'padding-bottom', 'border-top-width', 'border-bottom-width']
          .reduce((total, prop) => total + parseFloat(m.getAll('.mn-pill.warn', prop)[index]), 0);
        expect(badge.height).toBeLessThanOrEqual(parseFloat(m.getAll('.mn-pill.warn', 'line-height')[index]) * 2 + sum + .5);
      }
    });
  }
});
