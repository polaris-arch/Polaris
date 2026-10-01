/**
 * 移动端「连接」屏的门。射程 = 本屏的**屏级契约**，与相邻的门不重叠：
 * `../mobile-entry.test.ts` 守入口链路（CSS 契约 A1 / 五个目的地 / 导航与路由同源），
 * `../nodes/nodes-screen.test.tsx` 守节点屏与跨屏的出口写入不变量，
 * `../../styles/css-feature-floor.test.ts` 守 CSS 特性下限。这里守的是内容契约本身。
 *
 * 权威判据：
 *  · `mobile-kit/component-specs/information-architecture.md` §2 前言 + §2.3（屏级契约本体）
 *  · `mobile-kit/data-contract.json` 的 `connections-detail` / `connections-closed` /
 *    `connections-aggregate` 三个 source（增量协议、有界环、连接数不是字节数）
 *  · `polaris-mobile-ia-adjudications-2026-09-04.md` 裁定 #5（日志空态给文案）与
 *    #14（写失败必有可见回显）—— 已定不重开
 *
 * ── 手段：node 环境 + `react-dom/server` 真渲染 ──────────────────────────────
 * 本仓刻意不装 jsdom / testing-library（先例见 `SubInfoBar.progress.test.tsx` 头注），
 * 故呈现层被切成纯组件 `ConnectionsView`：门喂它 props、拿回真 HTML，再对着 DOM 断言。
 * 「组件源码里出现过某个名字」不是同一件事 —— 那种断言挡不住「它渲染在一个永不为真的分支里」。
 *
 * ── 每一组都先有自检 ────────────────────────────────────────────────────────
 * 纯否定式判据（「不许出现 X」）会被「什么都没发生」骗过：扫描器塌了、正则敲错一个字母、
 * 渲染抛异常被吞，都会让它们一路绿灯。故每组先断言取材面非空、量级合理，再断言正面等式。
 */
import { afterAll, describe, it, expect } from 'vitest';
import {
  ALL,
  UNCONDITIONAL,
  contextOf,
  declaringSites,
  resolve as cascadeResolve,
  winners,
} from '@/styles/css-cascade.test-support';
import { renderToStaticMarkup } from 'react-dom/server';
import { closeOracle, measure } from '@/styles/css-oracle.test-support';
import { inMobileShell } from '@/styles/mount.test-support';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, resolve as resolvePath } from 'node:path';
import { fileURLToPath } from 'node:url';
import type {
  ClosedConnectionEntry,
  ConnectionEntry,
  ConnectionsClosedUpdate,
  ConnectionsDetailUpdate,
} from '@/contracts/types';
import { MAX_CLOSED_HISTORY } from '@/components/screens/connections/closed-history';
import { connectionRuleSubjects } from '@/components/screens/connections/connection-rule-subjects';
import { ConnectionsView, type ConnectionsViewProps } from './ConnectionsView';
import {
  ACTIVE_SORT_KEYS,
  CLOSED_SORT_KEYS,
  applyActiveFrame,
  applyClosedFrame,
  createActiveFeed,
  createClosedFeed,
  matchConnRow,
  resetActiveFeed,
  sortConnRows,
  type ConnRowVM,
} from './view-model';

/** …/ui/src/ */
const SRC = resolvePath(fileURLToPath(new URL('.', import.meta.url)), '..', '..');
const HERE = join(SRC, 'mobile', 'connections');
const DESKTOP_CONN = join(SRC, 'components', 'screens', 'connections');
const DESKTOP_LOGS = join(SRC, 'components', 'screens', 'logs');
const read = (p: string): string => readFileSync(p, 'utf8');
const rel = (p: string): string => relative(SRC, p).split('\\').join('/');

/**
 * 剥注释。**必须识别字符串**：本目录的头注里满是 `data-tip` 这类反面示例与引文，
 * 不剥就会把文档说明当成真实代码抓进取材面，门于是对着自己的注释报红。
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

/** 走目录树而不是写死清单：新加一个屏文件自动进面。 */
function walk(dir: string, ext: RegExp): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...walk(p, ext));
    else if (ext.test(name)) out.push(p);
  }
  return out;
}

const SCREEN_SRC = walk(HERE, /\.(?:ts|tsx)$/).filter((p) => !/\.test\.tsx?$/.test(p));
const SCREEN_TEXT = SCREEN_SRC.map((p) => strip(read(p))).join('\n');
const WIRING = strip(read(join(HERE, 'MobileConnectionsScreen.tsx')));
/**
 * CSS **剥掉注释**再进判据面：本文件的注释里逐字引用了别处的类名（比如外壳的 `.m-page`），
 * 不剥就会把注释里的名字当成「声明了 sticky 的类」，②「吸顶面只有一个入口」那条当场假红。
 */
const CSS = read(join(HERE, 'connections.css')).replace(/\/\*[\s\S]*?\*\//g, ' ');

/* ── HTML 祖先链 ──────────────────────────────────────────────────────────────
 * 「粘在 header 下」这件事的可执行判据是**祖先关系**，不是「源码里写了 sticky」：
 * 把搜索框挪出吸顶块，源码里那条 CSS 规则一字未动，而用户那边它已经跟着滚了。
 */
const VOID_TAGS = new Set([
  'area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta',
  'param', 'source', 'track', 'wbr',
]);

/** `html` 中 `index` 处仍然打开着的标签（由外向内）。 */
function openTagsAt(html: string, index: number): { name: string; text: string }[] {
  const stack: { name: string; text: string }[] = [];
  const re = /<(\/?)([a-zA-Z][a-zA-Z0-9-]*)((?:"[^"]*"|'[^']*'|[^>"'])*)>/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(html)) !== null) {
    if (m.index >= index) break;
    const [full, slash, name, attrs] = m;
    if (slash) {
      const at = stack.map((x) => x.name).lastIndexOf(name);
      if (at >= 0) stack.length = at;
    } else if (!VOID_TAGS.has(name) && !attrs.trimEnd().endsWith('/')) {
      stack.push({ name, text: full });
    }
  }
  return stack;
}

/**
 * `connections.css` 的吸顶面 —— 走共享解析器 `styles/css-cascade.test-support.ts`。
 *
 * 迁移前是「`matchAll` 扫全部块，声明面命中 `/position:\s*sticky/` 就把选择器里的**类名**收进 Set」。
 * 那是一个**存在性**判定，2026-09-05 实测两条都溜过去了：
 *  · 在文件末尾补 `@container mscreen (min-width: 37.5em){ .mc-top{ position: static } }` ——
 *    平板档整个吸顶块不再吸顶，而 `.mc-top` 早已因基础态那条进了 Set，静态覆写不会把它移出去；
 *  · 补 `#mc-alt-top{position:sticky;top:0}` 另开一个吸顶入口 —— 正则只采**类名**，
 *    穷举断言 `toEqual(['mc-top'])` 照旧成立。
 *
 * 现在按**选择器**建表（不是类名）、按**层叠胜出值**判（不是"出现过"），并且把条件块里的覆写
 * 单独列出来 —— 某一档把 sticky 取消掉是本门守的那件事在那一档不成立，必须自曝。
 * `-webkit-sticky` 这个合法旧拼法也认（旧正则不认）。
 */
const CONN_CTX = contextOf([
  { file: 'src/mobile/connections/connections.css', css: read(join(HERE, 'connections.css')) },
]);

/**
 * 单文件上下文的胜出值必须等于**整条移动端层叠链**上的胜出值。
 *
 * 2026-09-05 MAJ-4：`CONN_CTX` 只装 `connections.css`，而进包顺序里 `src/mobile/mobile.css`
 * 排在**最后**（各屏 CSS 先进包，外壳压屏）—— 它的同键覆写在单文件模型里根本不存在。
 * 实测：`mobile.css` 追加 `.mc-top{ position: static }` 之后真机上吸顶头不再吸顶，
 * 而本组每条祖先断言照旧绿、全量 rc=0。判据形态抄同批的
 * `components/layout/window-drag-region.test.ts:52-58`。
 */
function chainAgrees(rawSel: string, prop: string, single: string | null): string | null {
  const chain =
    cascadeResolve({ sel: rawSel, prop, ctx: 'mobile', where: UNCONDITIONAL }).winner?.value ?? null;
  if (chain === single) return null;
  return `${rawSel} 的 \`${prop}\`：connections.css 里是 \`${single}\`，整条移动端链上是 \`${chain}\``;
}
const IS_STICKY = /^(?:-webkit-)?sticky$/;

/** 声明过 `position` 的每个选择器（键 → 原文 + 无条件胜出值 + 条件块里的覆写）。 */
const POSITION_SITES = (() => {
  const bySel = new Map<
    string,
    { raw: string; conditional: { value: string; conds: string[]; line: number }[] }
  >();
  for (const d of declaringSites('position', { decls: CONN_CTX, where: ALL })) {
    for (const [i, key] of d.sels.entries()) {
      const e = bySel.get(key) ?? { raw: d.rawSels[i], conditional: [] };
      if (d.conds.length > 0) e.conditional.push({ value: d.value, conds: d.conds, line: d.line });
      bySel.set(key, e);
    }
  }
  return [...bySel].map(([key, e]) => ({
    key,
    raw: e.raw,
    // **显式**只看无条件声明：条件档单独列在 `conditional` 里，由下面两条判据各自表态
    //（解析器对「没给 where 而同键还有条件声明」失败关闭，见 BLK-2）。
    base: winners({ sel: e.raw, decls: CONN_CTX, where: UNCONDITIONAL }).won.get('position')?.value ?? null,
    conditional: e.conditional,
  }));
})();

/** 无条件层叠胜出值是 sticky 的那些选择器（原文，排序稳定）。 */
const STICKY_SELECTORS = POSITION_SITES.filter((p) => p.base !== null && IS_STICKY.test(p.base))
  .map((p) => p.raw)
  .sort();

/** 那些吸顶选择器里出现的类名（祖先链判定用）。 */
const STICKY_CLASSES = new Set(
  STICKY_SELECTORS.flatMap((sel) => [...sel.matchAll(/\.([a-zA-Z][\w-]*)/g)].map((m) => m[1])),
);

/** 某个锚点是否被一个「CSS 里声明为 sticky」的元素包住。 */
function hasStickyAncestor(html: string, needle: string): boolean {
  const at = html.indexOf(needle);
  if (at < 0) return false;
  return openTagsAt(html, at).some((tag) => {
    const cls = /class="([^"]*)"/.exec(tag.text)?.[1] ?? '';
    return cls.split(/\s+/).some((c) => STICKY_CLASSES.has(c));
  });
}

/* ── 渲染夹具 ───────────────────────────────────────────────────────────────── */

/** `t` 桩：把插值原样拼回 key 后面，这样「计数没传进文案」这种变异也抓得到。 */
const t: ConnectionsViewProps['t'] = (key, vars) =>
  vars ? `${key}(${Object.entries(vars).map(([k, v]) => `${k}=${String(v)}`).join(',')})` : key;

const win = (total: number, page = 0, size = 20) => ({
  page,
  pageCount: Math.max(1, Math.ceil(total / size)),
  start: page * size,
  end: Math.min(total, page * size + size),
});

const row = (id: string, over: Partial<ConnRowVM> = {}): ConnRowVM => ({
  id,
  host: `host-${id}.example`,
  dest: '198.51.100.7:443',
  destIP: '198.51.100.7',
  rule: 'GEOSITE,cn',
  ruleGroupKey: 'policy:proxied',
  chain: 'node-a',
  l4: 'TCP',
  l4Full: 'tcp · Tun',
  udp: false,
  procName: 'app',
  procFull: '/usr/bin/app',
  sourceIP: '10.0.0.2',
  totalBytes: 4096,
  upRate: 100,
  dnRate: 200,
  startAt: 1_000,
  endedAt: null,
  rateSequence: 7,
  chainKind: 'named',
  ...over,
});

const baseProps = (over: Partial<ConnectionsViewProps> = {}): ConnectionsViewProps => ({
  t,
  segment: 'overview',
  onSegment: () => {},
  search: '',
  onSearch: () => {},
  overflow: [],
  onOverflow: () => {},
  privacy: false,
  aggregateLoaded: true,
  topN: 10,
  onTopN: () => {},
  hosts: [{ label: 'a.example', count: 4, aurora: true }],
  outbounds: [{ label: 'node-a', count: 3 }],
  rows: [],
  listTotal: 0,
  listLoaded: true,
  listWindow: win(0),
  onListPage: () => {},
  clock: { at: 10_000, sequence: 7 },
  expandedId: null,
  onToggleRow: () => {},
  onRowMenu: () => {},
  onCloseRow: () => {},
  sort: null,
  paused: false,
  logRows: [],
  logTotal: 0,
  logWindow: win(0),
  onLogPage: () => {},
  logPrivacyNote: false,
  legacyNotice: null,
  coreLevelNotice: null,
  coreLevelHint: 'logs.coreLevelHint',
  follow: true,
  pendingCount: 0,
  onResumeFollow: () => {},
  rowErrors: {},
  sheet: null,
  onCloseSheet: () => {},
  ...over,
});

const render = (over: Partial<ConnectionsViewProps> = {}): string =>
  renderToStaticMarkup(<ConnectionsView {...baseProps(over)} />);

/* ── 增量协议夹具 ───────────────────────────────────────────────────────────── */

const entry = (id: string, over: Partial<ConnectionEntry> = {}): ConnectionEntry => ({
  id,
  chains: ['node-a'],
  rule: 'GEOSITE,cn',
  metadata: { host: `${id}.example`, network: 'tcp', destinationIP: '198.51.100.7' },
  upload: 100,
  download: 200,
  start: new Date(1_000).toISOString(),
  ...over,
});

const detail = (over: Partial<ConnectionsDetailUpdate>): ConnectionsDetailUpdate => ({
  reset: false,
  generation: 1,
  sequence: 1,
  connections: [],
  at: 1_000,
  ...over,
});

const closedEntry = (id: string, closedAt: number): ClosedConnectionEntry => ({
  entry: entry(id),
  // 后端给的是 UnixNano。
  closedAt: closedAt * 1_000_000,
});

const closedUpdate = (over: Partial<ConnectionsClosedUpdate>): ConnectionsClosedUpdate => ({
  reset: false,
  connections: [],
  at: 1_000,
  ...over,
});

// ─────────────────────────────────────────────────────────────────────────────

describe('⓪ 自检：判据的取材面是活的', () => {
  it('剥注释挡住注释里的反面示例，且不把字符串当注释', () => {
    expect(strip(`/* data-tip={x} */\nconst a = 1;`)).not.toContain('data-tip');
    expect(strip(`const g = ['**/x/**'];\nconst b = 2;`)).toContain('**/x/**');
  });

  it('目录走查扫到了屏的源码（扫 0 个文件会让下面每条恒绿）', () => {
    expect(SCREEN_SRC.map(rel)).toContain('mobile/connections/ConnectionsView.tsx');
    expect(SCREEN_SRC.map(rel)).toContain('mobile/connections/MobileConnectionsScreen.tsx');
    expect(SCREEN_SRC.map(rel)).toContain('mobile/connections/view-model.ts');
    expect(SCREEN_TEXT.length).toBeGreaterThan(10_000);
    expect(WIRING.length).toBeGreaterThan(5_000);
  });

  it('屏真的渲染得出来（渲染抛异常被吞会让 DOM 类断言全部对着空串恒绿）', () => {
    const html = render();
    expect(html.length).toBeGreaterThan(800);
    expect(html).toContain('data-conn-scope="mobile-connections"');
  });

  it('祖先链解析器在合成样本上正反两向都成立（它是 ② 那组的全部依据）', () => {
    const sample = '<div class="a"><section class="b"><input id="x"/></section></div>ZZ<span>Y</span>';
    expect(openTagsAt(sample, sample.indexOf('id="x"')).map((n) => n.name)).toEqual([
      'div',
      'section',
    ]);
    // 闭合之后就不该再算祖先 —— 少了这一条，「把搜索框移出吸顶块」的变异会被判成没移。
    expect(openTagsAt(sample, sample.indexOf('ZZ')).map((n) => n.name)).toEqual([]);
    // 自闭合标签不许留在栈里（留了的话它后面的一切都会多一层假祖先）。
    expect(openTagsAt(sample, sample.indexOf('Y')).map((n) => n.name)).toEqual(['span']);
  });

  it('CSS 里确实解析出了 sticky 类（解析不出来会让 ② 那组恒绿）', () => {
    expect([...STICKY_CLASSES]).toContain('mc-top');
  });
});

describe('① IA §2.3 内容序：四个分段，默认概览，每段的块按重要性序出现', () => {
  it('四个分段一个不少，且默认段是概览（对齐桌面 `ConnectionsScreen.tsx:249`）', () => {
    const html = render();
    for (const key of [
      'mobileConnections.seg.overview',
      'connections.activeTab',
      'connections.closedTab',
      'mobileConnections.seg.logs',
    ]) {
      expect(html, `缺分段：${key}`).toContain(key);
    }
    // 默认段渲染的是概览的两张卡，不是列表。
    expect(html).toContain('data-segment="overview"');
    expect(html).not.toContain('data-segment="active"');
  });

  it('概览：top hosts 卡（含 N 选择器）在出站分布卡**之前**', () => {
    const html = render();
    const hosts = html.indexOf('mobileConnections.overview.hostsTitle');
    const outs = html.indexOf('mobileConnections.overview.outboundsTitle');
    expect(hosts).toBeGreaterThanOrEqual(0);
    expect(outs).toBeGreaterThan(hosts);
    // N 选择器只在域名卡上；出站卡是同一个 N 的只读回显。
    expect(html).toContain('connections.topCount');
    expect(html).toContain('connections.topBadge(n=10)');
  });

  it('活动 / 已结束：搜索框 → 列表 → 分页器', () => {
    const rows = Array.from({ length: 25 }, (_, i) => row(`c${i}`));
    const html = render({
      segment: 'active',
      rows: rows.slice(0, 20),
      listTotal: 25,
      listWindow: win(25),
    });
    const at = ['data-conn-search', 'mc-list', 'data-pager'].map((x) => html.indexOf(x));
    expect(at.every((i) => i >= 0), `缺块：${at.join(',')}`).toBe(true);
    expect([...at].sort((x, y) => x - y)).toEqual(at);
  });

  it('日志：搜索 → 日志视图 → 页脚（实时指示 + 结果计数 + 分页器）', () => {
    const logRows = Array.from({ length: 20 }, (_, i) => ({
      key: `l${i}`,
      ts: '00:00:00',
      level: 'info',
      message: `line ${i}`,
    }));
    const html = render({ segment: 'logs', logRows, logTotal: 44, logWindow: win(44) });
    const at = ['data-conn-search', 'mc-logview', 'mc-logfoot', 'data-pager'].map((x) =>
      html.indexOf(x),
    );
    expect(at.every((i) => i >= 0), `缺块：${at.join(',')}`).toBe(true);
    expect([...at].sort((x, y) => x - y)).toEqual(at);
    expect(html).toContain('logs.liveStream');
    expect(html).toContain('mobileConnections.logs.lines(n=44)');
  });

  it('级别与来源筛选进 sheet，不在页面上占一行（IA §2.3「in a sheet」）', () => {
    const plain = render({ segment: 'logs' });
    expect(plain, '级别筛选出现在了页面本体上').not.toContain('logs.currentLevel');
    // 正面：它在面板里确实渲染得出来（不是「哪儿都没有」）。
    const sheeted = render({
      segment: 'logs',
      sheet: {
        title: 'mobileConnections.logs.filter',
        items: [{ id: 'level-info', label: 'logs.currentLevel' }],
      },
    });
    expect(sheeted).toContain('logs.currentLevel');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ② IA §2.3「Sticky under the header」—— 本文件里最重的一条。
 *
 * 分段条是作用域指示；搜索框是**持续作用于实时列表**的条件。把后者滚走，等于每改一次
 * 检索条件都要先滚回顶部 —— 而那条路上列表还在自己动，滚回去时已经不是刚才那一屏了。
 *
 * 判据按**祖先关系**写，不按「源码里有没有 sticky 那行 CSS」写：把搜索框挪出吸顶块，
 * CSS 一字未动，用户那边它已经跟着滚了。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('② 分段条与搜索框粘在 header 下（IA §2.3）', () => {
  for (const segment of ['active', 'closed', 'logs'] as const) {
    it(`${segment}：搜索框的祖先链里有一个 CSS 声明为 sticky 的元素`, () => {
      const html = render({ segment, rows: segment === 'logs' ? [] : [row('c1')] });
      expect(html, '搜索框根本没渲染 ⇒ 下面那条恒假，先修夹具').toContain('data-conn-search');
      expect(
        hasStickyAncestor(html, 'data-conn-search'),
        '搜索框不在吸顶块里 —— 边读实时列表边改检索条件是常态，' +
          '把它滚走等于每次改条件都要滚回顶部，而那时列表已经不是刚才那一屏',
      ).toBe(true);
    });
  }

  it('分段条同样在吸顶块里（它是作用域指示，滚走了下面的列表就读不出是哪一段）', () => {
    const html = render({ segment: 'active' });
    expect(html).toContain('mc-seg');
    expect(hasStickyAncestor(html, 'class="mc-seg"')).toBe(true);
  });

  it('反向对照：概览段**没有**搜索框（证明上面几条不是恒真）', () => {
    const html = render({ segment: 'overview' });
    expect(html).not.toContain('data-conn-search');
    expect(html, '反向对照本身要有信息量：这一轮仍然渲染出了吸顶块').toContain('mc-top');
  });

  it('吸顶面只有一个入口（多一个 sticky 容器，上面的祖先断言就可能被别的块喂饱）', () => {
    // 按**选择器**穷举，不是按类名：`#mc-alt-top{position:sticky}` 这种另开的入口，
    // 只采类名的旧写法一个字都不会说。
    expect(STICKY_SELECTORS).toEqual(['.mc-top']);
    expect([...STICKY_CLASSES].sort()).toEqual(['mc-top']);
  });

  it('单文件读数与整条移动端层叠链一致（后进包的 `mobile.css` 没在盖它）', () => {
    const drift = POSITION_SITES.map((p) => chainAgrees(p.raw, 'position', p.base)).filter(
      (x): x is string => x !== null,
    );
    expect(
      drift,
      `这些选择器的 \`position\` 在整条链上被后进包的文件盖掉了 —— ` +
        `本组的祖先断言读的是一个真机上不成立的值：\n${drift.join('\n')}`,
    ).toEqual([]);
  });

  it('没有哪个断点档**新开**一个吸顶入口（上面那条穷举只看无条件档）', () => {
    // 2026-09-05 MAJ-9：`@container mscreen (min-width: 37.5em){ .mc-alt-top{ position: sticky } }`
    // 这种「只在某一档才吸顶」的入口，`base` 是 null ⇒ 进不了 `STICKY_SELECTORS`，
    // 而「吸顶面只有一个入口」那条穷举一个字都不会说。两个方向要各有一条。
    const extra = POSITION_SITES.filter(
      (p) =>
        (p.base === null || !IS_STICKY.test(p.base)) &&
        p.conditional.some((c) => IS_STICKY.test(c.value)),
    ).flatMap((p) =>
      p.conditional
        .filter((c) => IS_STICKY.test(c.value))
        .map((c) => `${p.raw} 在 ${c.conds.join(' « ')} 里成了吸顶入口（:${c.line}）`),
    );
    expect(
      extra,
      `这些档位上多出了别的吸顶容器 —— 上面几条祖先断言可能被别的块喂饱：\n${extra.join('\n')}`,
    ).toEqual([]);
  });

  it('没有哪个断点档把吸顶取消掉（`@container` 里一句 `position:static` 就够）', () => {
    const cancelled = POSITION_SITES.filter((p) => p.base !== null && IS_STICKY.test(p.base)).flatMap(
      (p) =>
        p.conditional
          .filter((c) => !IS_STICKY.test(c.value))
          .map((c) => `${p.raw} 在 ${c.conds.join(' « ')} 里被改成 \`${c.value}\`（:${c.line}）`),
    );
    expect(
      cancelled,
      `这些档位上吸顶块不再吸顶 —— 上面几条祖先断言在那一档不成立：\n${cancelled.join('\n')}`,
    ).toEqual([]);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ③ IA 裁定 #5：日志空态**给文案**。
 *
 * 桌面刻意不渲染任何占位，理由写在 `LogsScreen.tsx` 头注 :38-39：逐字复现原型。
 * 那是**保真约束**不是产品原则 —— 移动端没有那份原型，这条约束在这里不存在；
 * 而移动端有一个桌面没有的失败形态：固定头与固定底导航之间一片空白，读作「屏幕坏了」。
 *
 * 本条是本批**唯一**一处有意偏离桌面既有产品行为的裁定，不得扩大到别的空态上 ——
 * 故下面第三条反过来钉住：列表空态用的仍是桌面那几句既有文案，没有另发明一套。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('③ 日志空态有文案（IA 裁定 #5）', () => {
  it('零行时渲染出空态块，且有标题与说明两句', () => {
    const html = render({ segment: 'logs', logRows: [], logTotal: 0 });
    expect(html, '日志视图本身没渲染 ⇒ 下面几条无意义').toContain('mc-logview');
    expect(
      html,
      '日志空态什么都没画 —— 固定头与固定底导航之间一片空白读作「屏幕坏了」，' +
        '不读作「暂时没有日志」（IA 裁定 #5）',
    ).toContain('data-log-empty');
    expect(html).toContain('mobileConnections.logs.emptyTitle');
    expect(html).toContain('mobileConnections.logs.emptyBody');
  });

  it('反向对照：有日志行时不画空态（证明上一条不是恒真）', () => {
    const html = render({
      segment: 'logs',
      logRows: [{ key: 'l1', ts: '00:00:01', level: 'info', message: 'hello' }],
      logTotal: 1,
    });
    expect(html).not.toContain('data-log-empty');
    expect(html).toContain('hello');
  });

  it('这条偏离**不许扩大**：列表空态仍用桌面既有文案，没有另发明一套', () => {
    const none = render({ segment: 'active', rows: [], listLoaded: true });
    expect(none).toContain('connections.noActive');
    const closed = render({ segment: 'closed', rows: [], listLoaded: true });
    expect(closed).toContain('connections.noClosed');
    const loading = render({ segment: 'active', rows: [], listLoaded: false });
    expect(loading).toContain('connections.loading');
    const filtered = render({ segment: 'active', rows: [], listTotal: 0, search: 'zz' });
    expect(filtered).toContain('connections.noMatch');
    // 四句必须是四句 —— 「没有活动连接」「还没读到」「搜不到」是三个不同的诊断。
    expect(new Set([none, closed, loading, filtered]).size).toBe(4);
  });
});

describe('④ IA §2.3：四个破坏性/控制动作进 header overflow，不摆成一行工具栏', () => {
  const items = [
    { id: 'pause', label: 'connections.pause' },
    { id: 'close-all', label: 'connections.closeAll', danger: true },
  ];

  it('overflow 为空时按钮不渲染（一颗点开什么都没有的 `⋯` 比没有更坏）', () => {
    expect(render({ segment: 'overview', overflow: [] })).not.toContain(
      'mobileConnections.more',
    );
  });

  it('overflow 非空时按钮在页头里，且动作只在面板里出现，不在页面本体上', () => {
    const html = render({ segment: 'active', overflow: items });
    expect(html).toContain('mobileConnections.more');
    expect(html, '破坏性动作被摆到了页面上 —— compact 放不下四个按钮的一行工具栏').not.toContain(
      'connections.closeAll',
    );
    const opened = render({
      segment: 'active',
      overflow: items,
      sheet: { title: 'mobileConnections.more', items },
    });
    expect(opened).toContain('connections.closeAll');
    expect(opened).toContain('mc-sheet-item danger');
  });

  it('武装态落成 `.confirming` 类 —— 这不是样式，是与 `confirm-twice` 的跨文件契约', () => {
    const opened = render({
      segment: 'active',
      overflow: items,
      sheet: {
        title: 'mobileConnections.more',
        items: [{ id: 'close-all', label: 'connections.confirm', danger: true, confirming: true }],
      },
    });
    expect(
      opened,
      '武装中的条目没有 `.confirming` 类：`confirm-twice` 的 capture 阶段 pointerdown ' +
        '会在第二次点击落下时先复位，文案于是在「全部关闭 ↔ 确认」之间无限翻转，永远关不掉',
    ).toContain('confirming');
    // 反向对照：不武装时不带这个类。
    expect(
      render({
        segment: 'active',
        sheet: { title: 'x', items: [{ id: 'close-all', label: 'connections.closeAll' }] },
      }),
    ).not.toContain('confirming');
  });

  it('接线侧真的用了共用实现，且四个确认键都在（本地自造确认位由全仓门另行禁止）', () => {
    expect(WIRING).toContain("from '@/lib/confirm-twice'");
    expect(WIRING).toMatch(/useConfirmTwice\(\)/);
    for (const key of [
      'CLOSE_ALL_KEY',
      'CLOSE_FILTERED_KEY',
      'CLEAR_CLOSED_KEY',
      'LOGS_CLEAR_KEY',
    ]) {
      expect(WIRING, `${key} 没有经 confirmTwice 武装`).toMatch(
        new RegExp(`confirmTwice\\(${key}`),
      );
      expect(WIRING, `${key} 的武装态没有传给条目 ⇒ 按钮永远不翻红`).toMatch(
        new RegExp(`armed === ${key}`),
      );
    }
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑤ 增量协议 / 有界环 / 计数器缺席 —— 纯逻辑单测。
 *
 * 这三件事写错的表现全是「列表看起来有数据但不对」：静态门看不出来，真机也不报错。
 * 判据只能是喂帧、看行。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑤ 活动连接的增量协议（data-contract#connections-detail）', () => {
  it('自检：正常的 reset 基线能建起行（建不起来会让下面每条否定恒真）', () => {
    const feed = createActiveFeed();
    const frame = applyActiveFrame(
      feed,
      detail({ reset: true, generation: 1, sequence: 1, connections: [entry('a'), entry('b')] }),
      false,
    );
    expect(frame).not.toBeNull();
    expect(frame?.rows.map((r) => r.id)).toEqual(['a', 'b']);
  });

  it('没有 reset 基线之前的孤立增量不许污染空索引', () => {
    const feed = createActiveFeed();
    const frame = applyActiveFrame(
      feed,
      detail({ reset: false, generation: 3, sequence: 9, connections: [entry('ghost')] }),
      false,
    );
    expect(frame, '拒帧必须回 null，调用方才知道「保留上一批」而不是「清空」').toBeNull();
    expect(feed.index.size).toBe(0);
  });

  it('generation 跳变而没带 reset ⇒ 拒收（否则重连后两个数据集会并成一份）', () => {
    const feed = createActiveFeed();
    applyActiveFrame(feed, detail({ reset: true, generation: 1, sequence: 1, connections: [entry('a')] }), false);
    const merged = applyActiveFrame(
      feed,
      detail({ reset: false, generation: 2, sequence: 1, connections: [entry('b')] }),
      false,
    );
    expect(merged).toBeNull();
    expect([...feed.index.keys()], '新代的增量被并进了旧代的索引').toEqual(['a']);
  });

  it('新代的 reset 才换基线：旧成员整批消失，不是叠加', () => {
    const feed = createActiveFeed();
    applyActiveFrame(feed, detail({ reset: true, generation: 1, sequence: 1, connections: [entry('a')] }), false);
    const frame = applyActiveFrame(
      feed,
      detail({ reset: true, generation: 2, sequence: 1, connections: [entry('b')] }),
      false,
    );
    expect(frame?.rows.map((r) => r.id)).toEqual(['b']);
  });

  it('旧代与重复序列一律拒收（乱序帧不许把计数回退）', () => {
    const feed = createActiveFeed();
    applyActiveFrame(feed, detail({ reset: true, generation: 5, sequence: 4, connections: [entry('a')] }), false);
    expect(
      applyActiveFrame(feed, detail({ generation: 4, sequence: 99, connections: [entry('z')] }), false),
    ).toBeNull();
    expect(
      applyActiveFrame(feed, detail({ generation: 5, sequence: 4, connections: [entry('z')] }), false),
    ).toBeNull();
    expect([...feed.index.keys()]).toEqual(['a']);
  });

  it('reset 必须**连速率基线一起清** —— 否则重连后第一帧拿旧账算速率', () => {
    const feed = createActiveFeed();
    applyActiveFrame(
      feed,
      detail({ reset: true, generation: 1, sequence: 1, at: 1_000, connections: [entry('a', { upload: 0, download: 0 })] }),
      false,
    );
    applyActiveFrame(
      feed,
      detail({ generation: 1, sequence: 2, at: 2_000, connections: [entry('a', { upload: 1_000, download: 2_000 })] }),
      false,
    );
    expect(feed.prev.get('a')?.dn, '自检：常态帧确实在记账').toBe(2_000);
    /*
     * 新代 reset：同一个 id 带着计数回来。
     *
     * 🔴 这里**必须给比旧账更大的计数**，不能给 0（主会话 2026-09-05 修）。
     * 原版给的是 `upload: 0, download: 0`，而速率算式是 `Math.max(0, (now - prev) / dt)` ——
     * 旧账没清时算出的是负值，被 `Math.max(0, …)` 夹成 **0**，与「旧账已清 ⇒ 无 prev ⇒ 0」
     * 在断言上**完全无法分辨**。实测：把 `feed.prev.clear()` 从 reset 分支里删掉，本组 64 条全绿。
     * 换成更大的计数后，旧账没清会算出 4000，与 0 可分辨，这条断言才真的在守它命名的那件事。
     */
    const after = applyActiveFrame(
      feed,
      detail({
        reset: true,
        generation: 2,
        sequence: 1,
        at: 3_000,
        connections: [entry('a', { upload: 5_000, download: 6_000 })],
      }),
      false,
    );
    expect(after?.rows[0]?.dnRate, 'reset 后仍拿旧账算速率').toBe(0);
    expect(after?.rows[0]?.upRate).toBe(0);
  });

  it('`resetActiveFeed` 把 generation 一并回 null（不回 ⇒ 重订后第一帧被当成同代重复拒掉）', () => {
    const feed = createActiveFeed();
    applyActiveFrame(feed, detail({ reset: true, generation: 7, sequence: 3, connections: [entry('a')] }), false);
    resetActiveFeed(feed);
    expect(feed.sync.generation).toBeNull();
    const back = applyActiveFrame(
      feed,
      detail({ reset: true, generation: 7, sequence: 1, connections: [entry('a')] }),
      false,
    );
    expect(back, '重订后的第一帧被拒 ⇒ 列表永远停在空的').not.toBeNull();
  });

  it('计数器缺席 ≠ 0（data-contract trap）：totalBytes 为 null，排序仍按 0 参与', () => {
    const feed = createActiveFeed();
    const frame = applyActiveFrame(
      feed,
      detail({
        reset: true,
        generation: 1,
        sequence: 1,
        connections: [entry('none', { upload: undefined, download: undefined }), entry('some')],
      }),
      false,
    );
    const byId = new Map(frame?.rows.map((r) => [r.id, r]));
    expect(byId.get('none')?.totalBytes).toBeNull();
    expect(byId.get('some')?.totalBytes).toBe(300);
    // 排序按 0 参与 ⇒ 缺席那条排在前面，而不是被丢掉或排到末尾。
    const sorted = sortConnRows(frame?.rows ?? [], { key: 'total', dir: 1 }, { at: 0, sequence: 1 });
    expect(sorted.map((r) => r.id)).toEqual(['none', 'some']);
  });

  it('隐私态在**投影层**摘掉 sourceIP 与进程路径（渲染层拿不到就画不出来）', () => {
    const feed = createActiveFeed();
    const frame = applyActiveFrame(
      feed,
      detail({
        reset: true,
        generation: 1,
        sequence: 1,
        connections: [
          entry('a', {
            metadata: { host: 'a.example', sourceIP: '10.0.0.9', processPath: '/usr/bin/secret' },
          }),
        ],
      }),
      true,
    );
    expect(frame?.rows[0]?.sourceIP).toBeNull();
    expect(frame?.rows[0]?.procFull).toBe('');
    expect(frame?.rows[0]?.procName).toBe('');
  });
});

describe('⑤b 已结束历史是**有界环**（data-contract#connections-closed）', () => {
  it('自检：常态增量按结束时间降序并入', () => {
    const feed = createClosedFeed();
    const rows = applyClosedFrame(
      feed,
      closedUpdate({ reset: true, connections: [closedEntry('a', 1_000), closedEntry('b', 2_000)] }),
      false,
    );
    expect(rows.map((r) => r.id)).toEqual(['b', 'a']);
  });

  it('到上限即逐出最旧的，索引不留被淘汰项（否则内存与列表都会无界增长）', () => {
    const feed = createClosedFeed();
    const many = Array.from({ length: MAX_CLOSED_HISTORY + 5 }, (_, i) =>
      closedEntry(`c${i}`, i + 1),
    );
    const rows = applyClosedFrame(feed, closedUpdate({ reset: true, connections: many }), false);
    expect(rows.length).toBe(MAX_CLOSED_HISTORY);
    expect(feed.index.size).toBe(MAX_CLOSED_HISTORY);
    // 最旧的五条（closedAt 最小）被逐出，最新的那条还在。
    expect(rows.map((r) => r.id)).toContain(`c${MAX_CLOSED_HISTORY + 4}`);
    for (let i = 0; i < 5; i += 1) {
      expect(feed.index.has(`c${i}`), `第 ${i} 条最旧的没被逐出`).toBe(false);
    }
  });

  it('reset 帧取代全部旧历史（不是并进去）', () => {
    const feed = createClosedFeed();
    applyClosedFrame(feed, closedUpdate({ reset: true, connections: [closedEntry('old', 1)] }), false);
    const rows = applyClosedFrame(
      feed,
      closedUpdate({ reset: true, connections: [closedEntry('new', 2)] }),
      false,
    );
    expect(rows.map((r) => r.id)).toEqual(['new']);
  });
});

describe('⑤c 检索面与九个排序键', () => {
  it('列表显示可读规则，点开后才显示内核原始命中条件', () => {
    const item = row('named', {
      rule: 'rule_set=[local-rs-a local-rs-b]',
      ruleName: '流媒体解锁',
      ruleGroupKey: 'named:流媒体解锁',
    });
    const collapsed = render({ segment: 'active', rows: [item], listTotal: 1, listWindow: win(1) });
    expect(collapsed).toContain('mc-row-rule">流媒体解锁');
    expect(collapsed).not.toContain(item.rule);
    const expanded = render({ segment: 'active', rows: [item], listTotal: 1, listWindow: win(1), expandedId: 'named' });
    expect(expanded).toContain(item.rule);
  });

  it('检索面覆盖域名 / 目标 / 规则 / 链路 / 进程 / L4（与桌面同一条）', () => {
    const r = row('a', { host: 'h.example', dest: 'd', rule: 'RU', chain: 'CH', procName: 'p', procFull: 'pf', l4Full: 'udp' });
    for (const q of ['h.example', 'd', 'ru', 'ch', 'pf', 'udp']) {
      expect(matchConnRow(r, q.toLowerCase()), `检索面漏了 ${q}`).toBe(true);
    }
    expect(matchConnRow(r, 'nothing-here')).toBe(false);
    expect(matchConnRow(row('named', { ruleName: '流媒体解锁' }), '流媒体')).toBe(true);
  });

  it('九个键一个不少（少一个就是新造一处两端不一致，IA §3.1 #22）', () => {
    expect(ACTIVE_SORT_KEYS.length).toBe(9);
    expect(CLOSED_SORT_KEYS.length).toBe(9);
    // rate 只在活动、ended 只在已结束 —— 与桌面的列集一致。
    expect(ACTIVE_SORT_KEYS).toContain('rate');
    expect(ACTIVE_SORT_KEYS).not.toContain('ended');
    expect(CLOSED_SORT_KEYS).toContain('ended');
    expect(CLOSED_SORT_KEYS).not.toContain('rate');
  });
});

describe('⑥ 概览两张卡量的是**连接数**，不是字节数（data-contract#connections-aggregate）', () => {
  it('两张卡的标题与常驻说明都写明了单位', () => {
    const html = render({ segment: 'overview' });
    expect(html).toContain('mobileConnections.overview.hostsTitle');
    expect(html).toContain('mobileConnections.overview.outboundsTitle');
    expect(
      html,
      '没有那句单位说明 ⇒ 用户会拿它与首页那两张按字节排名的卡互相对账，然后认定其中一个坏了',
    ).toContain('mobileConnections.overview.unitNote');
  });

  it('两张卡都不经字节格式化器（画出 `4 KB` 就是把连接数说成了流量）', () => {
    const html = render({
      segment: 'overview',
      hosts: [{ label: 'a.example', count: 4096, aurora: true }],
      outbounds: [{ label: 'node-a', count: 4096 }],
    });
    expect(html).toContain('4096');
    expect(html).not.toMatch(/4(\.\d+)?\s?(KB|MB|KiB)/);
  });

  it('接线侧把 sentinel「其它」换成了本地化文案，且域名卡整条剔除它', () => {
    expect(WIRING).toContain('TOPOLOGY_OTHERS_KEY');
    expect(WIRING, 'sentinel 会被原样画到屏幕上（一串控制字符）').toContain("'home.others'");
    expect(WIRING).toMatch(/filter\(\(h\) => h\.name !== TOPOLOGY_OTHERS_KEY\)/);
  });

  it('「阻断」走动作标签轴（`--err`），不是流量表达轴（`--warn`）', () => {
    /*
     * 本屏的 `.mc-row-chain.block` 已登记进 `styles/style-invariants.test.ts` 的花名册，
     * 但那道门的 CSS 取材面只有桌面四个样式文件 ⇒ **它管不到这份 CSS 的取值**。
     * 取值由这里钉：与桌面连接表同一轴、同一 token（「这条连接的出站是阻断」是动作标签，
     * 不是「流量到此被丢弃」那条 --warn 的流量表达）。
     */
    const blockRule = /\.mc-row-chain\.block\s*\{([^}]*)\}/.exec(CSS);
    expect(blockRule, '`.mc-row-chain.block` 的规则不见了 —— 判据面塌了').not.toBeNull();
    expect(blockRule?.[1]).toMatch(/hsl\(var\(--err\)\)/);
    expect(blockRule?.[1], '混进了第二档严重度 —— 阻断又漂成两种说法').not.toMatch(/--warn/);
    // 渲染腿：block 档真的挂得上那个类，且文案走的是登记在花名册里的那个 key。
    const html = render({
      segment: 'active',
      rows: [row('c1', { chain: 'block', chainKind: 'block' })],
    });
    expect(html).toContain('mc-row-chain block');
    expect(html).toContain('home.routingBlock');
  });

  it('隐私态：域名卡换成隐私文案（域名同属敏感数据）', () => {
    const html = render({ segment: 'overview', privacy: true });
    expect(html).toContain('connections.privacyHidden');
    expect(html).not.toContain('a.example');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑦ IA §4.12：桌面 data-tip 的每一条解释都有移动端落点（计数，不是印象）。
 * ═════════════════════════════════════════════════════════════════════════ */

/**
 * 桌面这两个屏的 `data-tip` 里出现过、而移动端**不以同名 key 复用**的那些，逐条登记落点。
 * 不在此表且不在移动端源码里出现 ⇒ 红。
 *
 * ⚠️ 这张表**只许放真正承重的条目**。给一条移动端其实已经复用了的 key 也写上处置，
 * 表面无害，实则是给门留了一个后门：将来那处复用被删掉时，本表会替它继续答「有落点」，
 * 而用户那边的解释已经没了。
 */
const TIP_DISPOSITION: Record<string, string> = {
  'logs.openDirTip':
    '「日志目录」整颗在 Android 上没有对应物（日志目录 = `activity.dataDir`，应用私有内部存储，' +
    '没有任何 DocumentsProvider 暴露它），故它的解释也随之不适用 —— 处置码与证据锚登记在 ' +
    '`absence-register.test-support.ts` 的 `LogsScreen.tsx|k:logs.openDir+logs.openDirTip` 一条；' +
    '用户「拿到日志」这件事由本屏的两条导出腿承载，那一条另有自己的说明。',
};

describe('⑦ 桌面 data-tip 的落点（IA §4.12）', () => {
  /** 桌面连接屏 + 日志屏里，真实 `data-tip={…}` / `tip={…}` 属性中出现的字面 i18n key。 */
  const tipKeys = (() => {
    const keys = new Set<string>();
    for (const file of [join(DESKTOP_CONN, 'ConnectionsScreen.tsx'), join(DESKTOP_LOGS, 'LogsScreen.tsx')]) {
      const body = strip(read(file));
      for (const attr of ['data-tip={', 'tip={']) {
        let i = body.indexOf(attr);
        while (i !== -1) {
          let j = i + attr.length;
          let depth = 1;
          while (j < body.length && depth > 0) {
            if (body[j] === '{') depth += 1;
            else if (body[j] === '}') depth -= 1;
            j += 1;
          }
          for (const m of body.slice(i, j).matchAll(/t\('([\w.]+)'/g)) keys.add(m[1]);
          i = body.indexOf(attr, j);
        }
      }
    }
    return [...keys].sort();
  })();

  /**
   * 间接载体：桌面把三句核内级别的解释先算进一个变量再挂上 `data-tip`，
   * 上面那个按属性取的扫描器看不见它们。逐条登记 + 自检它们确实还在桌面源码里。
   */
  const INDIRECT_TIP_KEYS = [
    'logs.coreLevelHint',
    'logs.coreLevelDriftUnsaved',
    'logs.coreLevelDriftRestart',
  ];

  it('自检：桌面的 data-tip 取材面非空且有量级（扫 0 条会让下面那条恒绿）', () => {
    expect(tipKeys.length, `只扫到 ${tipKeys.length} 条 data-tip key —— 取材面塌了`).toBeGreaterThan(
      10,
    );
    expect(tipKeys).toContain('connections.closeAllTitle');
    expect(tipKeys).toContain('logs.redactTip');
    const logsSrc = read(join(DESKTOP_LOGS, 'LogsScreen.tsx'));
    for (const key of INDIRECT_TIP_KEYS) {
      expect(logsSrc, `间接登记表里的 ${key} 在桌面日志屏里找不到了`).toContain(key);
    }
  });

  it('逐条有落点：要么移动端复用同一句 key，要么在处置表里登记', () => {
    const orphan = [...tipKeys, ...INDIRECT_TIP_KEYS].filter(
      (k) => !SCREEN_TEXT.includes(`'${k}'`) && TIP_DISPOSITION[k] === undefined,
    );
    expect(
      orphan,
      '这些解释在桌面只由 hover 承载，移动端既没复用也没登记落点 —— ' +
        '一个够不着含义的角标比没有角标更坏：它会被读成装饰，用户于是断定没出问题',
    ).toEqual([]);
  });

  it('处置表不留僵尸条目（登记了一条桌面上已经不存在的 tip）', () => {
    for (const key of Object.keys(TIP_DISPOSITION)) {
      expect(tipKeys, `处置表里的 ${key} 在桌面上已经没有了 —— 该把它删掉`).toContain(key);
    }
  });

  it('移动端源码里一次 `data-tip` 都没有（触屏没有 hover，挂上去等于没写）', () => {
    expect(SCREEN_TEXT).not.toContain('data-tip');
    // 正面：解释确实落到了常驻通道上（面板条目的第二行 / 展开区）。
    const opened = render({
      sheet: {
        title: 'x',
        items: [{ id: 'a', label: 'connections.closeAll', description: 'connections.closeAllTitle' }],
      },
    });
    expect(opened).toContain('mc-sheet-item-sub');
    expect(opened).toContain('connections.closeAllTitle');
  });

  it('行动作面板的三个规则对象都取得到（喂进拼了端口的串会静默少一条「复制 IP」）', () => {
    const subjects = connectionRuleSubjects({
      id: 'c1',
      chains: ['node-a'],
      rule: 'r',
      metadata: {
        host: 'a.example',
        destinationIP: '198.51.100.7',
        processPath: '/usr/bin/app',
      },
    });
    expect(subjects.map((s) => s.kind)).toEqual(['domain', 'ip', 'process']);
    // 反向对照：把显示串（带端口）喂进去，`ip` 那条当场消失 —— 这就是要挡的那个形态。
    const withPort = connectionRuleSubjects({
      id: 'c1',
      chains: ['node-a'],
      rule: 'r',
      metadata: { host: 'a.example', destinationIP: '198.51.100.7:443' },
    });
    expect(withPort.map((s) => s.kind)).toEqual(['domain']);
    // 接线侧确实喂的是原值那一格，不是显示串。
    expect(WIRING).toContain('destinationIP: row.destIP');
  });

  it('行展开区是那七处截断全文的落点（桌面靠 hover 看全文，触屏没有）', () => {
    const html = render({ segment: 'active', rows: [row('c1')], expandedId: 'c1' });
    expect(html).toContain('data-conn-detail="c1"');
    for (const key of [
      'connections.colHost',
      'connections.colDest',
      'connections.colRule',
      'connections.colChain',
      'connections.colType',
      'connections.colTime',
      'connections.colProcess',
    ]) {
      expect(html, `展开区缺 ${key}`).toContain(key);
    }
    // 反向对照：不展开就没有这一块（证明上面不是恒真）。
    expect(render({ segment: 'active', rows: [row('c1')] })).not.toContain('data-conn-detail');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑧ IA 裁定 #14：写失败必有可见回显。
 *
 * 移动外壳既无 `DialogHost` 也无 toast 宿主 ⇒ 一次写失败今天的默认表现是
 * 「状态自己弹回去，一句话都没有」（`contracts/action-failure-visibility.test.ts` 守的正是
 * 这个形态，W10/W14 真机首曝过）。本屏的处置是行内错误 + **唯一写出口** `runWrite`。
 *
 * 判据刻意**不只钉「有 catch」**：有 catch 而 catch 里什么都不做，正是要抓的那一种。
 * 而且这里的失败有两条腿 —— 抛异常，以及**回 `ok:false` 的正常应答**（后端把「核没跑 /
 * gRPC 连不上 / 内核拒绝」装在里面）。只接住前者，破坏性动作照样静默。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑧ 写操作失败必有可见回显（IA 裁定 #14）', () => {
  /** `runWrite(` 每一次调用的括号平衡区间。 */
  const runWriteSpans = (() => {
    const spans: [number, number][] = [];
    for (const m of WIRING.matchAll(/\brunWrite\s*\(/g)) {
      let i = m.index + m[0].length;
      let depth = 1;
      while (i < WIRING.length && depth > 0) {
        if (WIRING[i] === '(') depth += 1;
        else if (WIRING[i] === ')') depth -= 1;
        i += 1;
      }
      spans.push([m.index, i]);
    }
    return spans;
  })();

  const inRunWrite = (index: number): boolean =>
    runWriteSpans.some(([a, b]) => index > a && index < b);

  /**
   * **读腿白名单**：订阅、水合、状态回读。它们不是「用户按下了一个按钮」，失败没有
   * 可归因的控件，故不占用户的行内通道（留 console 证据）。
   * 白名单之外的每一个 `api.*.*(` 都必须在 `runWrite` 里 —— 新增一处写而忘了包，本条转红。
   */
  const READ_CALLS = new Set([
    'api.stats.onConnectionsDetail',
    'api.stats.onConnectionsClosed',
    'api.stats.onConnectionsAggregate',
    'api.stats.subscribe',
    'api.stats.unsubscribe',
    'api.logs.onReceivedBatchReady',
    'api.logs.get',
    'api.logs.unsubscribe',
    'api.logs.search',
    'api.logs.diagnosticState',
    'api.logs.runtimeLevel',
    // 旧日志只读探测（批 13）：进日志段问一次「有没有那个文件」，没有用户在等回执。
    // 归档 / 删除那两条**不在**这里，它们罩在 `runWrite` 里。
    'api.logs.legacyInfo',
    'api.proxy.onLifecycle',
  ]);

  /**
   * **容忍换行的点号**：`api.logs\n  .search(…)` 与 `api.logs.search(…)` 是同一次调用，
   * 而只认后者的扫描器可以被一次重排静默绕过 —— 那是「判据被格式化打败」的经典形态。
   * 匹配到的文本归一化后再比对。
   */
  const API_CALL_RE = /\bapi\s*\.\s*[a-zA-Z]+\s*\.\s*[a-zA-Z]+(?=\s*\()/g;
  const apiCalls = [...WIRING.matchAll(API_CALL_RE)].map((m) => ({
    text: m[0].replace(/\s+/g, ''),
    index: m.index,
  }));
  const apiCallNames = new Set(apiCalls.map((c) => c.text));
  const otherWrites = [...WIRING.matchAll(/\bnavigator\.clipboard\.[a-zA-Z]+\s*\(|\bsaveConfig\s*\(/g)].map(
    (m) => ({ text: m[0].trim(), index: m.index }),
  );

  it('自检：本屏真的有一批调用，且 `runWrite` 真的被调过（扫 0 处会让下面几条恒绿）', () => {
    expect(apiCalls.length, `只扫到 ${apiCalls.length} 处 api 调用 —— 判据面塌了`).toBeGreaterThan(
      10,
    );
    expect(runWriteSpans.length, '一次 `runWrite(` 调用都没扫到').toBeGreaterThan(5);
    expect(otherWrites.length, '剪贴板与 saveConfig 一处都没扫到').toBeGreaterThan(2);
  });

  it('每一处写调用都在 `runWrite(` 的参数区间内（新增一处绕过唯一出口的写 ⇒ 红）', () => {
    const escaped = [
      ...apiCalls.filter((c) => !READ_CALLS.has(c.text) && !inRunWrite(c.index)),
      ...otherWrites.filter((c) => !inRunWrite(c.index)),
    ];
    expect(
      escaped.map((x) => x.text),
      '这些写调用不在 `runWrite` 里 —— 它们失败时用户看不到任何东西：' +
        '状态弹回原值，一句话都没有（IA 裁定 #14 要抓的正是这个形态）',
    ).toEqual([]);
  });

  it('读腿白名单不留僵尸（登记了一个已经没人调的读）', () => {
    for (const name of READ_CALLS) {
      expect([...apiCallNames], `白名单里的 ${name} 已经没人调了 —— 该把它删掉`).toContain(name);
    }
  });

  it('扫描器自检：换行写法的点号照样抓得到（否则一次重排就能绕过上面每一条）', () => {
    const sample = 'void api.logs\n  .search(q);\nawait api.connections.close(id);';
    expect([...sample.matchAll(API_CALL_RE)].map((m) => m[0].replace(/\s+/g, ''))).toEqual([
      'api.logs.search',
      'api.connections.close',
    ]);
  });

  it('四个破坏性动作确实各自经过 `runWrite`（不是「有 runWrite 但没人用」）', () => {
    for (const call of [
      'api.connections.close(',
      'api.connections.closeAll(',
      'api.stats.clearClosed(',
      'api.logs.clear(',
    ]) {
      const at = WIRING.indexOf(call);
      expect(at, `${call} 不见了 —— 判据面塌了`).toBeGreaterThan(-1);
      expect(inRunWrite(at), `${call} 没有经 runWrite`).toBe(true);
    }
  });

  it('`runWrite` 的两条失败腿都写进了错误通道 —— 不是空 catch、也不是只接住异常', () => {
    const at = WIRING.indexOf('const runWrite');
    expect(at, '`runWrite` 不见了 —— 判据面塌了（改名了？）').toBeGreaterThan(-1);
    const body = WIRING.slice(at, at + 1_200);
    const catchAt = body.indexOf('catch');
    expect(catchAt, '`runWrite` 里没有 catch').toBeGreaterThan(-1);
    expect(
      body.slice(catchAt),
      'catch 里没有把失败送进 `setWriteErrors` —— 空 catch 与没有 catch 等价',
    ).toContain('setWriteErrors');
    const okBranch = body.slice(body.indexOf('if (!ok)'), catchAt);
    expect(
      okBranch,
      '`ok:false` 那条腿没有回显 —— 后端把「核没跑 / 内核拒绝」装在正常应答里，' +
        '只 catch 是接不住的，破坏性动作照样静默',
    ).toContain('setWriteErrors');
  });

  it('屏级错误档真的渲染得出来（通道存在但画不出来 = 门在但没牙）', () => {
    const html = render({ segment: 'active', writeError: 'connections.closeFailed' });
    expect(html).toContain('data-write-error');
    expect(html).toContain('connections.closeFailed');
    // 它必须落在吸顶块里 —— 用户的手指刚才就在那颗 `⋯` 上，回显不该在滚动区里等着被发现。
    expect(hasStickyAncestor(html, 'data-write-error')).toBe(true);
  });

  it('行级错误档落在**那一行**里，不是屏顶', () => {
    const html = render({
      segment: 'active',
      rows: [row('c1'), row('c2')],
      rowErrors: { c2: 'connections.closeFailed' },
    });
    const rowAt = html.indexOf('data-conn-row="c2"');
    const errAt = html.indexOf('data-write-error');
    expect(rowAt).toBeGreaterThan(-1);
    expect(errAt).toBeGreaterThan(rowAt);
    expect(openTagsAt(html, errAt).some((tag) => tag.text.includes('data-conn-row="c2"'))).toBe(
      true,
    );
  });

  it('面板里按下的失败留在面板里（面板是模态，回显在它背后等于没有回显）', () => {
    const html = render({
      sheet: { title: 'x', items: [{ id: 'a', label: 'y' }], error: 'connections.closeFailed' },
    });
    const errAt = html.indexOf('data-write-error');
    expect(errAt).toBeGreaterThan(-1);
    expect(openTagsAt(html, errAt).some((tag) => tag.text.includes('mc-sheet'))).toBe(true);
  });

  it('反向对照：没有错误时一处都不渲染（证明上面几条不是恒真）', () => {
    const html = render({ segment: 'active', rows: [row('c1')] });
    expect(html).not.toContain('data-write-error');
    expect(html, '反向对照要有信息量：这一轮确实渲染出了行').toContain('data-conn-row="c1"');
  });
});

describe('⑨ 分页保留、列表恒单列、暂停是退订', () => {
  it('分页器在两段列表与日志上都在（IA §2.3：不换成无限滚动）', () => {
    expect(render({ segment: 'active', rows: [row('a')], listTotal: 50, listWindow: win(50) })).toContain(
      'data-pager',
    );
    expect(render({ segment: 'logs', logTotal: 50, logWindow: win(50) })).toContain('data-pager');
    // 反向对照：一页装得下时不渲染分页器。
    expect(render({ segment: 'active', rows: [row('a')], listTotal: 1, listWindow: win(1) })).not.toContain(
      'data-pager',
    );
  });

  it('expanded 下行内多出进程这一格（IA §2.3 Breakpoints），且由容器查询而非 JS 决定', () => {
    const html = render({ segment: 'active', rows: [row('c1', { procName: 'chrome' })] });
    expect(html, '进程那一格根本没渲染 ⇒ 容器查询打开它时也没东西可显示').toContain('mc-row-proc');
    expect(html).toContain('chrome');
    // 可见性归 CSS：默认不占位，expanded 断点里现身。
    expect(CSS).toMatch(/\.mc-row-proc\s*\{[^}]*display:\s*none/);
    const expanded = CSS.slice(CSS.indexOf('min-width: 52.5em'));
    expect(expanded, 'expanded 断点里没有打开进程格 —— 那一格于是永远看不见').toMatch(
      /\.mc-row-proc\s*\{[^}]*display:\s*inline/,
    );
  });

  it('实时列表恒单列：CSS 里没有任何多列/网格把它铺开', () => {
    const listRules = [...CSS.matchAll(/([^{}]*\.mc-list[^{}]*)\{([^{}]*)\}/g)].map((m) => m[2]);
    expect(listRules.length, '`.mc-list` 的规则一条都没解析出来 —— 判据面塌了').toBeGreaterThan(0);
    for (const body of listRules) {
      expect(body, '实时列表被铺成了多列 —— 行在被实时增删时会在读者眼下重排').not.toMatch(
        /column-count|grid-template-columns/,
      );
    }
  });

  it('暂停在接线上是**退订**（不是只冻结渲染），且屏上有常驻说明', () => {
    expect(
      WIRING,
      '暂停没有参与订阅腿的门控 ⇒ 它只是冻了画面，后端整条链路还在跑',
    ).toMatch(/if \(paused \|\| segment !== 'active'\) return;/);
    const html = render({ segment: 'active', paused: true });
    expect(html).toContain('mobileConnections.pausedNote');
    expect(render({ segment: 'active', paused: false })).not.toContain(
      'mobileConnections.pausedNote',
    );
    /*
     * 另两条腿（与桌面 `topic-subscription-wiring.test.ts` 的 T3 同形，但那道门按文件名读桌面屏，
     * 覆盖不到这里）：cleanup 必须 `dispose` 而不是 `setWanted(false)` —— 后者监听还在，
     * 在途帧仍会落表，「暂停」于是只暂停了一半；且 `paused` 必须真在依赖里，否则不触发重建。
     */
    const at = WIRING.indexOf('onConnectionsDetail');
    expect(at, 'detail 端口不见了 —— 判据面塌了').toBeGreaterThan(-1);
    const effect = WIRING.slice(WIRING.lastIndexOf('useEffect(', at), at + 700);
    expect(effect, 'cleanup 必须 dispose（同步摘监听）').toMatch(
      /return\s*\(\)\s*=>\s*sub\.dispose\(\);/,
    );
    expect(effect, '`paused` 不在依赖里 ⇒ 暂停不触发重建，订阅照跑').toMatch(
      /\}\s*,\s*\[[^\]]*\bpaused\b[^\]]*\]\s*\)/,
    );
  });

  it('暂停**不清空**列表：清账那条 effect 的守卫只看段，不看 paused', () => {
    const clearAt = WIRING.indexOf('resetActiveFeed(activeFeedRef.current)');
    expect(clearAt, '清账调用不见了 —— 判据面塌了').toBeGreaterThan(-1);
    const guard = WIRING.slice(WIRING.lastIndexOf('useEffect', clearAt), clearAt);
    expect(guard.length, '取不到那条 effect 的守卫 —— 判据面塌了').toBeGreaterThan(10);
    expect(guard, '自检：守卫确实按段门控').toContain("segment === 'active'");
    expect(
      guard,
      '`paused` 进了清账守卫 ⇒ 一按暂停列表就空了。暂停的语义是「停止更新」不是「清屏」：' +
        '用户按下暂停正是为了把此刻这一屏留住看清楚',
    ).not.toContain('paused');
  });

  it('浮动「回到底部」只在跟随被打断后出现', () => {
    const broken = render({ segment: 'logs', follow: false, pendingCount: 12 });
    expect(broken).toContain('data-log-jump');
    expect(broken).toContain('logs.scrollToBottom');
    expect(broken, '缓冲了多少行这条信息不能丢').toContain('12');
    expect(render({ segment: 'logs', follow: true })).not.toContain('data-log-jump');
  });
});

/* ═══════════ ⑩ 分段控件的轨道层 ═══════════ */

/**
 * 取出某个选择器那一条规则的声明面。找不到返回 `null` —— 调用点必须把 `null` 当红，
 * 不许当「没这条规则所以通过」。（`CSS` 进来时注释已剥。）
 */
/**
 * 同一个选择器**全部**规则体的拼接（`null` = 一条都没有）。
 *
 * 上一版是「找到第一条即 `return`」，断言的于是是「某处写过这些声明」，不是「层叠之后看得见的
 * 是这些」：在文件后面再写一条同名规则把值盖回去，门读到的仍是靠前那条 ⇒ 反面断言
 * （`.not.toMatch(...)`）会被绕过去。收全部之后，反面断言扫的是整个声明面。
 *
 * 顺手剥掉块注释：不剥的话本文件头注里的反面示例会被当成真规则抓进取材面。
 *
 * **射程边界**：拼接判的是「有没有人写过」，判不了「谁胜出」。同名选择器的顺序之争由
 * `styles/cascade-dead-rules.test.ts` 全仓看住。
 */
function declsOf(selector: string): string | null {
  const body = CSS.replace(/\/\*[\s\S]*?\*\//g, '');
  const hits: string[] = [];
  for (const m of body.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (m[1].trim().replace(/\s+/g, ' ') === selector) hits.push(m[2]);
  }
  return hits.length > 0 ? hits.join('\n') : null;
}

describe('⑩ 概览 TOP N 是 `segmented-control`，不是三颗独立按钮（IA §3.1 #7）', () => {
  /*
   * 这一格是 touch-adapted：「Hit area to `layout.touchTargetMin`.
   * **Border, radius and selected fill unchanged.**」—— 只准放大命中面。
   * 没有轨道那一层时，5/10/15 读作三颗按钮里有一颗换了描边色；深色主题下轨道与选中块的
   * 明度差本来就小，靠边框色分离几乎看不出当前档是哪一个。
   */
  it('轨道层存在：有边框、有 `--surface-2` 底、有圆角', () => {
    const track = declsOf('.mc-choice');
    expect(track, '`.mc-choice` 这条规则不见了 —— 判据面塌了').not.toBeNull();
    expect(track, '分段控件没有轨道：缺 border ⇒ 它读作三颗独立按钮').toMatch(/\bborder\s*:/);
    expect(track).toMatch(/background\s*:\s*hsl\(var\(--surface-2\)\)/);
    expect(track).toMatch(/border-radius\s*:/);
  });

  it('选中态是 `--surface` 浮块 + `--flow-hi` 文字，且只在轨道里成立', () => {
    const on = declsOf(".mc-choice .mc-btn[aria-pressed='true']");
    expect(on, '轨道内的选中态规则不见了 —— 判据面塌了').not.toBeNull();
    expect(on, '选中块没有浮起的 `--surface` 底').toMatch(
      /background\s*:\s*hsl\(var\(--surface\)\)/,
    );
    expect(on).toMatch(/color\s*:\s*hsl\(var\(--flow-hi\)\)/);
    // 反面：不许再出现不带轨道限定的裸选择器（它会波及任何一颗带按下态的 `.mc-btn`）。
    expect(declsOf(".mc-btn[aria-pressed='true']"), '裸 `.mc-btn[aria-pressed]` 又回来了').toBeNull();
    /*
     * 反面之二：选中块不许退回 `flow-weak` 铺底 —— 那一档在**桌面**已经被当面否决过
     * （`styles/index.css`：「accent 铺在底色上，整块就是一片蓝」），深色下轨道与选中块的
     * 明度差本来就小，铺一层 accent 等于把「选中」读成「整条轨道亮了」。
     *
     * 这条**必须扫整个声明面**：在文件后面再补一条同选择器的 `@container` 规则把它复活，
     * 是 `declsOf` 上一版（命中首条即返回）漏掉的形态 —— 门读到的仍是靠前那条，绿。
     */
    expect(on, '选中块又回到了被桌面否决过的 `flow-weak` 铺底').not.toMatch(/--flow-weak/);
  });

  it('接线：概览那三颗档位真的渲染在 `.mc-choice` 轨道里', () => {
    const html = render({ segment: 'overview' });
    const track = /<div class="mc-choice"[^>]*>([\s\S]*?)<\/div>\s*<\/div>/.exec(html);
    expect(track, '概览段没有渲染出 `.mc-choice` —— 判据面塌了').not.toBeNull();
    const pressed = [...(track?.[1] ?? '').matchAll(/aria-pressed="(true|false)"/g)];
    expect(pressed.length, '轨道里的档位数不对').toBe(3);
    expect(
      pressed.filter((m) => m[1] === 'true').length,
      '轨道里没有恰好一个选中档 —— 分段控件的语义就是「一条轨道里选中一格」',
    ).toBe(1);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑪ **浏览器裁判**：吸顶这件事由 Chrome 对真 DOM 算。
 *
 * ② 组读的是「CSS 里无条件胜出值是 sticky 的那些选择器」，再拿类名去对 DOM 的祖先链。
 * 那条链上有两处只有浏览器能答：
 *  · **计算值**：某一档写一句 `position: static`、或从更宽/更窄的选择器压一句下来，
 *    源码里 `.mc-top{position:sticky}` 一个字没变，用户那边它已经跟着滚了；
 *  · **祖先关系**：类名比对是「这个 class 出现在某个祖先的 class 列表里」的近似，
 *    真祖先链只有 DOM 知道。
 * 本组直接问浏览器：从搜索框往上数，computed `position` 是 `sticky` 的祖先都是谁。
 * ② 组保留 —— 它现在守的是「CSS 里只有一个吸顶入口」这件**源码面**的事。
 * ═══════════════════════════════════════════════════════════════════════════ */
describe('⑪ 浏览器裁判：搜索框与分段条真的吸在屏头下（真 DOM + 整条移动端 CSS 链）', () => {
  // 收尾走 CDP 的 `Browser.close`，让 Chrome 自己退（全程不 kill 进程）。
  // 超时给到 30s：全量并行跑时机器争用，默认的 10s hookTimeout 会在收尾这一步假红。
  afterAll(closeOracle, 30_000);

  const STICKY = { prop: 'position', value: 'sticky' } as const;
  const paint = (over: Partial<ConnectionsViewProps>, asks: Parameters<typeof measure>[1]) =>
    measure(
      {
        ctx: 'mobile',
        // 真挂载树（第五轮 B01）：屏根之上还有 `#root > .m-shell > .m-scroll > .m-page`。
        html: inMobileShell(render(over), 'connections'),
        rootAttrs: { 'data-theme': 'dark' },
        viewport: { width: 390, height: 844 },
      },
      asks,
    );

  for (const segment of ['active', 'closed', 'logs'] as const) {
    it(`${segment}：搜索框的祖先链里恰好有一个 computed 吸顶元素，且就是 \`.mc-top\``, async () => {
      const m = await paint({ segment, rows: segment === 'logs' ? [] : [row('c1')] }, [
        { select: '[data-conn-search]', props: ['position'], ancestorsWhere: STICKY },
      ]);
      expect(m.count('[data-conn-search]'), '搜索框根本没渲染 ⇒ 下面那条恒假').toBeGreaterThan(0);
      expect(
        m.ancestors('[data-conn-search]'),
        '搜索框的祖先链上没有一个**算出来**是吸顶的元素 —— 边读实时列表边改检索条件是常态，' +
          '把它滚走等于每次改条件都要滚回顶部；' +
          '多出一个则「祖先链里有吸顶元素」这句话可能被别的块喂饱',
      ).toEqual(['div.mc-top']);
    }, 30_000);
  }

  it('分段条同样吸在屏头下（它是作用域指示，滚走了下面的列表就读不出是哪一段）', async () => {
    const m = await paint({ segment: 'active' }, [
      { select: '.mc-seg', props: ['position'], ancestorsWhere: STICKY },
    ]);
    expect(m.ancestors('.mc-seg')).toEqual(['div.mc-top']);
  }, 30_000);

  it('反向对照：`.mc-top` 自己算出来就是 sticky，且概览段没有搜索框（上面几条不是恒真）', async () => {
    const m = await paint({ segment: 'overview' }, [{ select: '.mc-top', props: ['position'] }]);
    expect(m.get('.mc-top', 'position'), '`.mc-top` 算出来不是吸顶 —— 整组的前提垮了').toBe('sticky');
    // 概览段没有搜索框 ⇒ 上面三条不是「随便问谁都吸顶」。
    // 这一条只能对 markup 断言：裁判对着一个命中不到的选择器是**抛**，不是返回空。
    expect(render({ segment: 'overview' }), '概览段也渲染了搜索框').not.toContain('data-conn-search');
  }, 30_000);
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑫ 批 13：把桌面这一屏最后三格接线钉成可执行事实。
 *
 * 三格各自的失败形态不同，故判据也分三种：
 *  · **可折叠分组**（桌面 `Csel` 的组头）—— 折叠是**视图态**，只能靠真渲染判：源码里出现
 *    `buildCselRows` 证明不了「折起来的那一组真的没画出来」。
 *  · **旧日志提示 + 两颗动作** —— 条件式在场（`legacyInfo().exists`）。反向对照是必须的：
 *    一条「有旧日志时画提示」的断言，在「恒画提示」的实现下同样绿。
 *  · **「加入规则」不再是死行** —— 接线侧的正面断言：那一行有 `onSelect`、且不带 `disabled`。
 *    纯否定式（「源码里没有 `disabled: true`」）会被「整行删掉」骗过。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑫ 批 13：加规则 / 可折叠分组 / 旧日志三格', () => {
  const grouped = (over: Partial<ConnectionsViewProps['sheet'] & object> = {}) => ({
    title: 'rules.pickTitle',
    items: [] as const,
    groups: [
      { label: 'mobileConnections.rules.canAppend', items: [{ id: 'ok-1', label: 'rule-alpha' }] },
      {
        id: 'blocked',
        label: 'mobileConnections.rules.cannotAppend',
        items: [
          { id: 'no-1', label: 'rule-beta', disabled: true },
          { id: 'no-2', label: 'rule-gamma', disabled: true },
        ],
      },
    ],
    ...over,
  });

  it('不带 id 的组恒展开、带 id 的组默认折起（主路径不许被折进去）', () => {
    const html = render({ segment: 'active', sheet: grouped() as ConnectionsViewProps['sheet'] });
    // 两个组头都在（折叠的那组也必须看得见，否则用户不知道还有东西）。
    expect(html).toContain('mobileConnections.rules.canAppend');
    expect(html).toContain('mobileConnections.rules.cannotAppend');
    // 恒展开那组的条目画出来了；折起来那组的条目一条都没画。
    expect(html).toContain('rule-alpha');
    expect(html, '折起来的那一组把条目也画出来了 —— 那就不是折叠').not.toContain('rule-beta');
    expect(html).not.toContain('rule-gamma');
    // 折叠态下唯一能看出「这组有多少条」的信号。
    expect(html, '折叠态下看不出这组有几条').toContain('class="mc-sheet-grp-c">2<');
    // 可折叠组头是按钮 + aria-expanded；不可折叠那个是 presentation，不占焦点链。
    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain('data-sheet-group="blocked"');
    expect(html).toContain('role="presentation"');
  });

  it('反向对照：`openGroups` 点名那一组 ⇒ 它的条目真的画出来（上一条不是「分组恒不画」）', () => {
    const html = render({
      segment: 'active',
      sheet: grouped({ openGroups: new Set(['blocked']) }) as ConnectionsViewProps['sheet'],
    });
    expect(html).toContain('rule-beta');
    expect(html).toContain('rule-gamma');
    expect(html).toContain('aria-expanded="true"');
  });

  it('折叠判据取自共用的 `buildCselRows`，不是本地又写一份', () => {
    const view = strip(read(join(HERE, 'ConnectionsView.tsx')));
    expect(view, '折叠行序另起了一份实现 —— 两端迟早分叉，且分叉了没有任何东西会红').toContain(
      "from '@/components/dialogs/csel-logic'",
    );
    expect(view).toContain('buildCselRows(');
    // 展开后要把新露出的那一段滚进视区（`.mc-sheet` 是 max-height + overflow-y:auto 的滚动容器）。
    expect(view, '展开不露出 ⇒ 组头在底部时用户读作「点了没反应」').toContain('revealSiblingGroup');
  });

  it('旧日志提示：`exists` 为真才画，且两种语气各自可达', () => {
    const warn = render({
      segment: 'logs',
      legacyNotice: { tone: 'warn', text: 'logs.legacyTitle logs.legacyBody' },
    });
    expect(warn).toContain('data-log-legacy');
    expect(warn).toContain('mc-notice warn');
    expect(warn).toContain('logs.legacyBody');
    const ok = render({ segment: 'logs', legacyNotice: { tone: 'ok', text: 'logs.deleteLegacyDone' } });
    expect(ok, '做完之后提示块凭空消失 —— 用户不知道刚才那一下成没成').toContain('mc-notice ok');
    expect(ok).toContain('logs.deleteLegacyDone');
  });

  it('反向对照：没有旧日志时整块不画（上一条不是「恒画」）', () => {
    expect(render({ segment: 'logs', legacyNotice: null })).not.toContain('data-log-legacy');
  });

  it('接线侧：探测在场、两颗动作随 `exists` 在场、且都经 `runWrite`', () => {
    /* 容忍换行的点号：`api.logs\n  .legacyInfo()` 与 `api.logs.legacyInfo()` 是同一次调用，
       只认后者的断言会被一次重排静默绕过（同门 ⑧ 那条扫描器自检的理由）。 */
    expect(WIRING, '旧日志探测腿不见了 ⇒ 那块提示与两颗动作恒不在场').toMatch(
      /\bapi\s*\.\s*logs\s*\.\s*legacyInfo\s*\(/,
    );
    // 两颗动作挂在 `legacyLog?.exists === true` 那一支上：没有那个文件时两端都一行不画。
    expect(WIRING).toContain('legacyLog?.exists === true');
    for (const call of ['api.logs.archiveLegacy(', 'api.logs.deleteLegacy(']) {
      const at = WIRING.indexOf(call);
      expect(at, `${call} 不见了 —— 判据面塌了`).toBeGreaterThan(-1);
    }
    /* 删除那颗的二次确认走表单宿主的 `confirm` 层，**不是** `window.confirm`：后者被
       dialog 插件的 init 覆写成 `plugin:dialog|confirm`，漏授 ACL 时整条腿抛 rejection，
       用户看到的是「操作失败」而不是一个确认框。 */
    expect(WIRING).toContain("kind: 'confirm'");
    expect(WIRING, '二次确认走了会被插件覆写的 `window.confirm`').not.toMatch(
      /\bwindow\s*\.\s*confirm\s*\(/,
    );
    expect(WIRING, '裸 `confirm(` 与 `window.confirm(` 是同一个全局，插件覆写的也是它').not.toMatch(
      /(?<![.\w$])confirm\s*\(/,
    );
  });

  it('「加入规则」那一行是真入口，不是在场置灰位', () => {
    const at = WIRING.indexOf("id: 'add-rule',");
    expect(at, "`id: 'add-rule'` 不见了 —— 判据面塌了").toBeGreaterThan(-1);
    const body = WIRING.slice(at, at + 400);
    expect(body, '「加入规则」又变回了写死禁用').not.toContain('disabled: true');
    expect(body, '那一行没有 onSelect ⇒ 点开什么都不发生').toContain('onSelect:');
    // 两颗动作各自兑现：新建 → 表单层（带预设）；加入已有 → 选择器 → 追加腿。
    expect(WIRING).toContain("openMobileForm({ kind: 'rule', preset:");
    expect(WIRING).toContain("await api.rules.update(next, 'route');");
    // 判据整层复用，不在本屏重写一份。
    expect(WIRING).toContain("from '@/components/dialogs/rule-append'");
    expect(WIRING, '那句「表单层还没有」的理由已经不成立，却还在渲染').not.toContain(
      'mobileRules.formUnavailable',
    );
  });
});
