/**
 * 移动端「首页」屏的门。射程 = 本屏的**屏级契约**，与相邻各道门不重叠：
 * `../mobile-entry.test.ts` 守入口链路（契约 A1 的 CSS 集合 / 五个目的地 / 导航与路由同源），
 * `../../styles/css-feature-floor.test.ts` 守 CSS 特性下限，
 * `../nodes/nodes-screen.test.tsx` ② A 腿守「全树上每颗写 exit_node 的控件都带标记 + 文件已登记」，
 * `../../lib/config-write-wiring.test.ts` 守 config 写入口登记。
 *
 * 权威判据：
 *  · `mobile-kit/component-specs/home-screen.md`（屏级契约本体：八张卡的序、三档内容一致）
 *  · 八张卡各自的 component-spec + `mobile-kit/data-contract.json`
 *  · `polaris-mobile-ia-adjudications-2026-09-04.md` #3（TsExitWarning 跨屏义务）与 #14（写失败可见）
 *
 * ── 手段：node 环境 + `react-dom/server` 真渲染 ──────────────────────────────
 * 本仓刻意不装 jsdom / testing-library，故呈现层被切成纯组件 `HomeScreenView`：门喂它 props、
 * 拿回真 HTML，再对着 DOM 断言。「源码里出现过这个名字」不是同一件事 —— 那种断言挡不住
 * 「它渲染在一个永不为真的分支里」。
 *
 * ── 每一组负面判据都先有正面对照 ────────────────────────────────────────────
 * 纯否定式判据（「CSS 里不许出现 X」）会被「什么都没发生」骗过：扫描器塌了、正则敲错一个字母、
 * 取材面读空，都会让它们一路绿灯。故每一条否定断言旁边都配一个**合成的正例**，
 * 证明同一个扫描器对着真违规确实报得出来。
 *
 * ── 判据的自污染 ────────────────────────────────────────────────────────────
 * 本文件里逐字写着被禁的形态（`display: none`、`order:` 之类）。取材面若把本文件也扫进去，
 * 门会对着自己的禁令列表报红；而「让它变绿」最自然的动作就是把取材面缩窄，最后缩成恒绿。
 * 故取材面**显式排除测试文件**，并由 ⓪ 组正面断言「排除生效 + 取材面仍然是活的」。
 */
import { afterAll, describe, it, expect } from 'vitest';
import { closeOracle, measure } from '@/styles/css-oracle.test-support';
import { inMobileShell } from '@/styles/mount.test-support';
import { renderToStaticMarkup } from 'react-dom/server';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import type { ServerConfig } from '@/contracts/types';
import { HomeScreenView, RuleHitsSheet, RULE_ACTION_KEY, RULE_ACTION_ORDER } from './HomeScreenView';
import {
  HOME_CARD_ORDER,
  WRITE_CONTROLS,
  type AppendTargetVM,
  type HomeScreenViewProps,
  type RuleAppendTarget,
} from './view-model';
import { createRunWrite, writeFailureText } from './write-errors';
import { UNLOCK_SERVICES, unlockBadgeSrc } from './unlock-services';
import { ENABLED_SERVICE_IDS } from '@/contracts/unlock-detection';
import {
  EMPTY_TRAFFIC_BUFFER,
  pushTotals,
  scaleMax,
  splinePath,
  TRAFFIC_WINDOW,
} from './traffic-buffer';
import {
  applyDetail,
  compositionOf,
  filterByHostQuery,
  hostTopOf,
  outboundOf,
  ruleHitsOf,
  totalBytesOf,
} from './aggregate';

/** …/ui/src/mobile/home/ */
const HERE = fileURLToPath(new URL('.', import.meta.url));
const read = (p: string): string => readFileSync(p, 'utf8');

/**
 * 剥 CSS 的注释与字符串。**两者一遍扫完** —— 分两遍会互相误吞（注释里含引号、字符串里含 `/*`）。
 * 本文件的头注里满是反面示例，不剥就会把说明文字当成真实声明抓进取材面。
 */
function stripCss(src: string): string {
  let out = '';
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === '/' && src[i + 1] === '*') {
      const end = src.indexOf('*/', i + 2);
      i = end < 0 ? src.length : end + 2;
      out += ' ';
      continue;
    }
    if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < src.length && src[j] !== c) j += src[j] === '\\' ? 2 : 1;
      i = j + 1;
      out += '""';
      continue;
    }
    out += c;
    i += 1;
  }
  return out;
}

/** 剥 TS/TSX 的注释与字符串，口径同上。 */
function stripTs(src: string): string {
  let out = '';
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === '/' && src[i + 1] === '*') {
      const end = src.indexOf('*/', i + 2);
      i = end < 0 ? src.length : end + 2;
      out += ' ';
      continue;
    }
    if (c === '/' && src[i + 1] === '/') {
      const end = src.indexOf('\n', i);
      i = end < 0 ? src.length : end;
      out += ' ';
      continue;
    }
    if (c === '"' || c === "'" || c === '`') {
      let j = i + 1;
      while (j < src.length && src[j] !== c) j += src[j] === '\\' ? 2 : 1;
      i = j + 1;
      out += '""';
      continue;
    }
    out += c;
    i += 1;
  }
  return out;
}

/** `@container` 每一块的**花括号平衡**体。缩进匹配靠不住，块里还有嵌套规则。 */
function containerBlocks(css: string): string[] {
  const out: string[] = [];
  for (const m of css.matchAll(/@container[^{]*\{/g)) {
    let i = m.index + m[0].length;
    let depth = 1;
    while (i < css.length && depth > 0) {
      if (css[i] === '{') depth += 1;
      else if (css[i] === '}') depth -= 1;
      i += 1;
    }
    out.push(css.slice(m.index, i));
  }
  return out;
}

/** 声明位上出现的属性名（`prop:` 只有跟在 `{` 或 `;` 后面才是一条声明，别把选择器里的冒号数进来）。 */
const declaredProps = (css: string, names: readonly string[]): string[] =>
  names.filter((n) => new RegExp(`(?:^|[;{])\\s*${n}\\s*:`, 'm').test(css));

const HOME_CSS_RAW = read(join(HERE, 'home.css'));
const HOME_CSS = stripCss(HOME_CSS_RAW);


/** 本屏的产品源码面（**排除测试文件**，见文件头注「判据的自污染」）。 */
const HOME_SRC = readdirSync(HERE)
  .filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f))
  .map((f) => join(HERE, f));
const HOME_TEXT = HOME_SRC.map((p) => stripTs(read(p))).join('\n');
const WIRING_TEXT = stripTs(read(join(HERE, 'MobileHomeScreen.tsx')));

/**
 * 只剥注释、**留字符串**。`stripTs` 把字符串也抹成空 —— 对「源码里不许出现这个标识符」那类判据
 * 正合适（注释里提一句不会误伤），但有几条判据要看的恰恰是字符串字面量本身
 * （`'tun'` / `runWrite('routing', …)`），在 `HOME_TEXT` 上它们全是 `""`，断言会恒假。
 * `[^:]` 前瞻避免把 `https://` 当行注释切掉（口径与 `store/system-proxy-live-wiring.test.ts` 同）。
 */
const stripTsComments = (src: string): string =>
  src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1');
const WIRING_RAW = stripTsComments(read(join(HERE, 'MobileHomeScreen.tsx')));

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

const t = ((key: string) => key) as HomeScreenViewProps['t'];

const server = (id: string): ServerConfig =>
  ({
    id,
    name: `node-${id}`,
    protocol: 'vless',
    address: '198.51.100.7',
    port: 443,
  }) as ServerConfig;

const baseProps = (over: Partial<HomeScreenViewProps> = {}): HomeScreenViewProps => ({
  t,
  writeErrors: {},
  core: 'running',
  connState: 'connected',
  node: {
    kind: 'node',
    name: 'HK03',
    protocolLabel: 'VLESS',
    flagSrc: null,
    latencyMs: 42,
    latencyStale: false,
  },
  onToggleConnect: () => {},
  onNetworkCheck: () => {},
  latencyCheck: {
    busyKind: null, error: null, feedback: null, blocked: null,
    blockedStatus: 'mobileHome.notApplicable', allUnavailable: null,
    onRunCurrent: () => {}, onRunAll: () => {},
  },
  tsExitWarning: 'none',
  onTsExitAction: () => {},
  pickerOpen: false,
  onOpenPicker: () => {},
  onClosePicker: () => {},
  pickerQuery: '',
  onPickerQuery: () => {},
  pickRows: [
    {
      server: server('a'),
      isCurrent: false,
      stagedOnly: false,
      latencyMs: 30,
      latencyStale: false,
      protocolLabel: 'VLESS',
    },
    {
      server: server('b'),
      isCurrent: true,
      stagedOnly: true,
      latencyMs: 42,
      latencyStale: false,
      protocolLabel: 'VLESS',
    },
  ],
  onUseAsExit: () => {},
  onPickSentinel: () => {},
  /* 默认可选（`routing !== 'direct'`）。直连模式那一档由 ⑬ 组自己覆写 —— 夹具给 null
     才能让「置灰 + 原因」那条断言的对照有意义。 */
  blockDisabledReason: null,
  /* 默认**有**节点（空态两颗入口的反向对照靠它）。 */
  noServers: false,
  onAddServer: () => {},
  onAddSubscription: () => {},
  routing: 'smart',
  /* 正向语义（默认）。反向那一档由 ⑫ 组自己覆写 —— 夹具给 false 才能让那组的对照有意义。 */
  reverseRouting: false,
  exitRegion: { kind: 'flag', code: 'hk' },
  exitFlagSrc: null,
  exitIsDirect: false,
  exitProbing: false,


  onSetRouting: () => {},
  exitIp: null,
  unlock: UNLOCK_SERVICES.map((s) => ({
    id: s.id,
    name: s.name,
    badgeSrc: null,
    result: { status: 'ok' as const },
  })),
  unlockCheckedLabel: 'mobileHome.checkedJustNow',
  unlockRunning: false,
  unlockDetailId: null,
  onOpenUnlockDetail: () => {},
  onCloseUnlockDetail: () => {},
  samples: Array.from({ length: 12 }, (_, i) => ({ up: i * 100, down: i * 300 })),
  composition: [
    { key: 'proxy', bytes: 800 },
    { key: 'Direct', bytes: 200 },
  ],
  ruleHits: [
    { key: 'named:国内直连', rule: '国内直连', kind: 'named', policy: 'direct', count: 3 },
    { key: 'policy:proxied', rule: 'proxied', kind: 'policy', policy: 'proxied', count: 5 },
  ],
  hosts: [
    { key: 'example.com', bytes: 700 },
    { key: 'cdn.example.net', bytes: 300 },
  ],
  hostsMasked: false,
  /* 主机筛选默认**没筛**（空串）—— 「有筛选词 / 无筛选词 / 无匹配」三档的反向对照靠它。
     `hosts` 是接线层筛过再排名的结果，故夹具里两者各自给值、互不推导。 */
  hostQuery: '',
  onHostQuery: () => {},
  /* 规则面板默认**没开**（`ruleSubject: null`）—— 反向对照靠它。开着的那几档由 ⑬ 组覆写。 */
  ruleSubject: null,
  ruleSubjectView: 'menu',
  onOpenRuleSubject: () => {},
  onCloseRuleSubject: () => {},
  onRuleSubjectView: () => {},
  onQuickRule: () => {},
  newRuleAction: 'proxy',
  onNewRuleAction: () => {},
  newRuleRemarks: '',
  onNewRuleRemarks: () => {},
  ruleRemarksHint: 'home.ruleRemarks',
  onCreateRule: () => {},
  appendQuery: '',
  onAppendQuery: () => {},
  appendTargets: [],
  onAppendToRule: () => {},
  windowBytes: 1000,
  windowConnections: 8,
  kernelConnections: 9,
  ...over,
});

/** 规则面板开着的那一档（域名对象，没有被任何规则覆盖）。 */
const SUBJECT_VM: HomeScreenViewProps['ruleSubject'] = {
  subject: { kind: 'domain', type: 'domain', value: 'example.com' },
  coveringName: null,
};

/** 「合并进已有规则」的一行候选（可追加，非影子，规则启用中）。 */
const appendRow = (over: Partial<RuleAppendTarget> = {}): AppendTargetVM => ({
  target: {
    ruleId: 'r1',
    ruleIndex: 0,
    remarks: '流媒体解锁',
    enabled: true,
    ruleType: 'domain',
    ruleValues: ['netflix.com'],
    condIndex: 0,
    type: 'domain',
    values: ['netflix.com'],
    block: null,
    search: ['流媒体解锁'],
    ...over,
  },
  shadowed: false,
});

const render = (over: Partial<HomeScreenViewProps> = {}): string =>
  renderToStaticMarkup(<HomeScreenView {...baseProps(over)} />);

/** DOM 里 `data-home-card="…"` 的出现序。 */
const cardOrder = (html: string): string[] =>
  [...html.matchAll(/data-home-card="([^"]+)"/g)].map((m) => m[1]);

/** 把渲染结果切成「每张卡各自的片段」（卡是兄弟节点，两个标记之间的内容归前一张）。 */
function cardSegments(html: string): Map<string, string> {
  const out = new Map<string, string>();
  const marks = [...html.matchAll(/data-home-card="([^"]+)"/g)];
  marks.forEach((m, i) => {
    const end = i + 1 < marks.length ? marks[i + 1].index : html.length;
    out.set(m[1], html.slice(m.index, end));
  });
  return out;
}

// ─────────────────────────────────────────────────────────────────────────────

describe('⓪ 自检：判据的取材面是活的', () => {
  it('剥离器挡住注释里的反面示例，且不把字符串当注释', () => {
    expect(stripCss('/* .x{display:none} */\n.y{color:red}')).not.toContain('display');
    expect(stripCss('.y{content:"/*"}color:red}')).toContain('color');
    expect(stripTs("// order: -1\nconst a = 1;")).not.toContain('order');
    expect(stripTs("const g = 'display: none';\nconst b = 2;")).not.toContain('display');
  });

  it('源码 / CSS 取材面非空，且**不含**本测试文件（否则门会对着自己的禁令列表报红）', () => {
    expect(HOME_SRC.map((p) => p.split('/').pop())).toContain('HomeScreenView.tsx');
    expect(HOME_SRC.some((p) => p.endsWith('.test.tsx'))).toBe(false);
    expect(HOME_TEXT.length).toBeGreaterThan(5000);
    expect(HOME_CSS.length).toBeGreaterThan(3000);
  });

  it('屏真的渲染得出来（渲染抛异常被吞会让 DOM 类断言全部对着空串恒绿）', () => {
    const html = render();
    expect(html.length).toBeGreaterThan(2000);
    expect(html).toContain('data-exit-scope="mobile-home"');
  });

  it('CSS 里真的有断点块（扫到 0 块会让 ② 组那两条 `@container` 断言恒绿）', () => {
    expect(containerBlocks(HOME_CSS).length).toBeGreaterThanOrEqual(2);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ① `home-screen.md`「Composition」：八张卡的 DOM 序 = 重要性序，**任何断点都不许重排**。
 *
 * 两条正交的破坏路径，各守一条腿：
 *   A 渲染腿：DOM 序与权威数组逐项相等 —— 挡「在 JSX 里把两张卡换个位置」；
 *   B CSS 腿：本屏 CSS 里不出现任何重排原语 —— 挡「在某个断点块里给某张卡加一条 `order`」。
 * 只守 A 会被「DOM 序没动、但 CSS 把它挪到别处」骗过，而那恰恰是**只在某一档发生**的重排。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('① DOM 序 = 重要性序，不许被断点重排', () => {
  it('A 腿：渲染出的卡序逐项等于 `HOME_CARD_ORDER`（换位置 ⇒ 红）', () => {
    expect(cardOrder(render())).toEqual([...HOME_CARD_ORDER]);
  });

  it('A 腿：八张卡各出现且**只出现一次**', () => {
    const seen = cardOrder(render());
    expect(seen.length).toBe(HOME_CARD_ORDER.length);
    expect(new Set(seen).size).toBe(HOME_CARD_ORDER.length);
  });

  it('A 腿：卡序与状态无关（换一组 props 仍是同一个序）', () => {
    expect(
      cardOrder(
        render({
          core: 'stopped',
          connState: 'disconnected',
          node: { kind: 'direct' },
          composition: [],
          ruleHits: [],
          hosts: [],
          samples: [],
          pickerOpen: true,

        }),
      ),
    ).toEqual([...HOME_CARD_ORDER]);
  });

  it('B 腿：CSS 不用 order / 反向排布篡改 DOM 阅读顺序', () => {
    const props = declaredProps(HOME_CSS, ['order', 'direction']);
    expect(
      props,
      '本屏 CSS 出现了能让视觉顺序和 DOM 阅读顺序相反的属性',
    ).toEqual([]);
    expect(/:\s*(?:row|column|wrap)-reverse/.test(HOME_CSS), '出现了反向排布').toBe(false);
  });

  it('B 腿的正面对照：同一个扫描器对着真违规确实报得出来', () => {
    const mutant = '@container mscreen (min-width: 37.5em) { .h-card { order: -1; } }';
    expect(declaredProps(stripCss(mutant), ['order'])).toEqual(['order']);
    expect(/:\s*(?:row|column|wrap)-reverse/.test('.a{flex-direction:column-reverse}')).toBe(true);
  });

  it('相关出口/检测与统计内容成组，宽屏容器重排不复制卡片', () => {
    expect(render()).toContain('h-connection-panel');
    expect(render()).toContain('h-statistics-panel');
    expect(HOME_CARD_ORDER).toContain('rule-hits');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ② `home-screen.md`「Content parity」：三档断点渲染**同样的八项**。
 *
 * 没有宽屏专属卡，也没有 compact 专属卡。内容体量固定，宽屏以受控列宽和
 * 两列组织卡片，动态字号仍由用户设置控制。
 *
 * 「只在 medium 以上渲染一张卡」有两条实现路径，各守一条腿：
 *   A JS 腿：本屏源码里**一个断点标识符都没有**（没有断点 prop、不量容器、不 `matchMedia`）；
 *   B CSS 腿：可见性属性既不出现在任何 `@container` 块里，也不以「默认藏起来」的形态出现在别处。
 * 两条腿合起来把那条路的两半各堵一次；只堵一半等于没堵。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('② 三档断点内容完全一致（Content parity）', () => {
  it('A 腿：本屏 TS/TSX 里没有任何断点/量尺标识符（响应式全部归 CSS 容器查询）', () => {
    const banned = [
      'matchMedia',
      'innerWidth',
      'clientWidth',
      'getBoundingClientRect',
      'breakpoint',
      'compact',
      'expanded',
    ];
    const nonAria = HOME_TEXT.replace(/\baria-expanded\b/g, '');
    const hit = banned.filter((b) => nonAria.includes(b));
    expect(
      hit,
      '本屏源码里出现了断点/尺寸标识符 —— 一旦 JS 能看见断点，「只在某一档渲染的卡」就有了写法',
    ).toEqual([]);
  });

  it('B 腿：任何 `@container` 块里都没有可见性属性（断点条件的显示/隐藏 ⇒ 红）', () => {
    const offenders = containerBlocks(HOME_CSS).flatMap((b) =>
      declaredProps(b, ['display', 'visibility', 'content-visibility']).map((p) => p),
    );
    expect(
      offenders,
      '某个断点块里改了可见性 —— 那正是「宽屏专属卡 / compact 专属卡」的实现方式',
    ).toEqual([]);
  });

  it('B 腿：全文件都没有「默认藏起来」的声明（那是上一条的另一半）', () => {
    expect(/display\s*:\s*none/.test(HOME_CSS), '出现了 display:none').toBe(false);
    expect(/visibility\s*:\s*hidden/.test(HOME_CSS), '出现了 visibility:hidden').toBe(false);
    expect(/content-visibility\s*:/.test(HOME_CSS), '出现了 content-visibility').toBe(false);
  });

  it('B 腿的正面对照：同一批扫描器对着真违规确实报得出来', () => {
    const mutant = stripCss(
      '.h-wide{display:none}\n@container mscreen (min-width: 37.5em){ .h-wide{display:block} }',
    );
    expect(/display\s*:\s*none/.test(mutant)).toBe(true);
    expect(containerBlocks(mutant).flatMap((b) => declaredProps(b, ['display']))).toEqual(['display']);
  });

  it('正面等式：`HOME_CARD_ORDER` 就是 spec 那张表的八项，一项不多一项不少', () => {
    expect([...HOME_CARD_ORDER]).toEqual([
      'node-status-card',
      'mode-chips',
      'unlock-detection',
      'rule-hits',
      'traffic-chart',
      'traffic-composition',
      'host-traffic-top',
    ]);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ③ IA 裁定 #3 —— 首页的节点卡能切出口 ⇒ 首页必须有 `TsExitWarning`，且它紧贴那颗控件。
 *
 * 「全树上每颗写出口的控件都带 `data-exit-write`、且文件已登记」由
 * `../nodes/nodes-screen.test.tsx` ② A 腿守（那条腿扫整棵 `ui/src/mobile`）。
 * 这里守的是**本屏渲染面**：标记与警示同处一个 scope。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('③ TsExitWarning 的同屏义务（IA 裁定 #3）', () => {
  it('自检：本屏真的渲染得出写出口的控件（扫不到 ⇒ 下面两条恒绿，那才是真危险）', () => {
    const html = render({ pickerOpen: true });
    expect((html.match(/data-exit-write=/g) ?? []).length).toBeGreaterThan(0);
  });

  it('警示态下，每个 `data-exit-write` 都与一条 `data-ts-exit-warning` 同处一个 scope', () => {
    const html = render({ pickerOpen: true, tsExitWarning: 'no-exit-device' });
    const scopes = html.split('data-exit-scope=').slice(1);
    expect(scopes.length, '渲染出的 DOM 里没有 exit scope —— 判据面塌了').toBe(1);
    for (const scope of scopes) {
      const marks = (scope.match(/data-exit-write=/g) ?? []).length;
      expect(marks, '这个 scope 里一颗出口控件都没有 —— 夹具没渲染出选择器？').toBeGreaterThan(0);
      expect(
        scope,
        `一个含 ${marks} 颗出口控件的 scope 里没有 TsExitWarning —— ` +
          'TS 当出口未配 exit_node 时公网会静默走直连而无任何提示（safety）',
      ).toContain('data-ts-exit-warning');
    }
  });

  it('出口 scope 就是节点卡本身（警示落在别的卡上等于没紧贴控件）', () => {
    const seg = cardSegments(render({ pickerOpen: true, tsExitWarning: 'needs-auth' })).get(
      'node-status-card',
    );
    expect(seg).toBeDefined();
    expect(seg).toContain('data-exit-scope="mobile-home"');
    expect(seg).toContain('data-exit-write=');
    expect(seg).toContain('data-ts-exit-warning="needs-auth"');
  });

  it('四档各自渲染得出（少一档 = 那一档的用户什么也看不到）', () => {
    for (const w of [
      'needs-auth',
      'no-exit-device',
      'exit-device-offline',
      'exit-device-not-advertised',
    ] as const) {
      const html = render({ tsExitWarning: w });
      expect(html, `${w} 档没渲染出警示`).toContain(`data-ts-exit-warning="${w}"`);
      expect(html, `${w} 档没渲染出文案`).toContain('home.tsExit');
    }
  });

  it('反向对照：`none` 档不渲染警示（证明上面几条不是恒真）', () => {
    const html = render({ pickerOpen: true, tsExitWarning: 'none' });
    expect(html).not.toContain('data-ts-exit-warning');
    expect(html, '反向对照本身要有信息量：这一轮仍然渲染出了出口控件').toContain('data-exit-write');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ④ IA 裁定 #14 —— 写操作失败必有**可见**回显，且贴着动手的那颗控件。
 *
 * 移动端没有 toast / 弹窗宿主，本批也不许造 ⇒ 失败的默认表现是「控件弹回原位，一句话都没有」。
 * 判据**不许只钉「有 catch」**（裁定原文点名）：下面第一条直接拿一个必然 reject 的 op 驱动
 * 纯工厂，断言错误真的落进了表 —— 空 catch 会被它当场抓住。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('④ 写失败必有可见回显（IA 裁定 #14）', () => {
  it('纯工厂：失败真的把一条错误写进了表（空 catch ⇒ 红）', async () => {
    let errors: Record<string, string | undefined> = {};
    const run = createRunWrite((f) => {
      errors = f(errors) as Record<string, string | undefined>;
    }, t as never);
    await run('connect', () => Promise.reject(new Error('boom-because-x')));
    expect(Object.keys(errors)).toEqual(['connect']);
    expect(errors.connect).toContain('mobileHome.actionFailedWithReason');
  });

  it('纯工厂：成功要**清掉**上一次的失败（否则修好的错误会永久挂在控件下面）', async () => {
    let errors: Record<string, string | undefined> = { connect: 'stale' };
    const run = createRunWrite((f) => {
      errors = f(errors) as Record<string, string | undefined>;
    }, t as never);
    await run('connect', () => Promise.resolve());
    expect(errors.connect).toBeUndefined();
  });

  it('取不到人话时只说「操作失败」，**不编一个原因**', () => {
    expect(writeFailureText(t as never, new Error(''))).toBe('mobileHome.actionFailed');
    expect(writeFailureText(t as never, new Error('why\nstack…'))).toBe(
      'mobileHome.actionFailedWithReason',
    );
  });

  it('每一颗登记的控件都渲染得出来，且它的错误与它**同处一张卡**', () => {
    for (const id of WRITE_CONTROLS) {
      const html = render({
        writeErrors: { [id]: `boom-${id}` },
        // 三个弹层里的控件也要能被渲染到（少开一个，那一层里的控件就算「没渲染出来」）。
        pickerOpen: true,

        ruleSubject: SUBJECT_VM,
      });
      expect(html, `控件 ${id} 没渲染出来`).toContain(`data-write-control="${id}"`);
      expect(html, `控件 ${id} 的错误没渲染出来`).toContain(`data-write-error="${id}"`);
      if (id === 'connect') {
        const header = html.slice(html.indexOf('<header class="h-header"'), html.indexOf('</header>') + 9);
        expect(header).toContain('data-write-control="connect"');
        expect(header).toContain('data-write-error="connect"');
        continue;
      }
      const owner = [...cardSegments(html)].find(([, seg]) =>
        seg.includes(`data-write-control="${id}"`),
      );
      expect(owner, `找不到承载 ${id} 的卡`).toBeDefined();
      expect(
        owner?.[1],
        `${id} 的行内错误没有落在它自己那张卡里 —— 用户看不见「刚才那一下失败了」`,
      ).toContain(`data-write-error="${id}"`);
      expect(owner?.[1]).toContain(`boom-${id}`);
    }
  });

  it('弹层必须带背幕（没有它，「点外面关掉」会穿透到底下那颗连接开关上）', () => {
    expect(render({ pickerOpen: true })).toContain('h-scrim');
    expect(render({ ruleSubject: SUBJECT_VM })).toContain('h-scrim');
    // 反向对照：不开弹层时不渲染背幕（否则整屏被一层不可见的按钮盖住）。
    expect(render()).not.toContain('h-scrim');
  });

  it('反向对照：没有错误时一个 `data-write-error` 都不渲染（证明上一条不是恒真）', () => {
    expect(
      render({ pickerOpen: true, ruleSubject: SUBJECT_VM }),
    ).not.toContain('data-write-error');
  });

  it('接线层：每一处写调用都在 `runWrite(` 的参数区间内（新增一处绕过唯一出口的写 ⇒ 红）', () => {
    /** 「这一处会写盘 / 会调后端 / 会改运行态」的记号。新增一类写操作也要登记进来。 */
    const WRITE_CALLS = [
      /\bapi\.[a-zA-Z]+\.[a-zA-Z]+\s*\(/g,
      /\bunlockApi\.[a-zA-Z]+\s*\(/g,
      /\bswitchServer\s*\(/g,
      /\bstartProxy\s*\(/g,
      /\bstopProxy\s*\(/g,
      /(?<![.\w$])update\s*\(\s*\{/g,
    ];
    const spans: [number, number][] = [];
    for (const m of WIRING_TEXT.matchAll(/\brunWrite\s*\(/g)) {
      let i = m.index + m[0].length;
      let depth = 1;
      while (i < WIRING_TEXT.length && depth > 0) {
        if (WIRING_TEXT[i] === '(') depth += 1;
        else if (WIRING_TEXT[i] === ')') depth -= 1;
        i += 1;
      }
      spans.push([m.index, i]);
    }
    /**
     * **不是写**的 `api.*` 方法，逐个登记。
     *
     * 裁定 #14 管的是「用户动了一下、它失败了、而用户什么也看不到」。事件监听与 topic 订阅
     * 生命周期两者都不由用户发起，也没有会弹回原值的控件 —— 它们失败的表现是「卡里没有数据」，
     * 而那正是各卡的空态在说的话。按名字登记而不是整片放行 `api.stats.*`：
     * 那个域里的 `clearClosed` 是真的写，整片放行会给它开一条绕过唯一出口的路。
     */
    const NOT_A_WRITE = ['subscribe', 'unsubscribe', 'onStatsUpdated', 'onConnectionsDetail'];
    const all = WRITE_CALLS.flatMap((re) =>
      [...WIRING_TEXT.matchAll(re)].map((m) => ({ text: m[0], index: m.index })),
    );
    // 自检：登记表不是僵尸（登记了一个已经没人调的方法 = 白名单在腐烂）。
    for (const n of NOT_A_WRITE) {
      expect(all.map((x) => x.text).join(' '), `${n} 已经没人调了 —— 该把它从登记表里删掉`).toContain(
        `.${n}(`,
      );
    }
    const sites = all.filter((x) => !NOT_A_WRITE.some((n) => x.text.includes(`.${n}(`)));
    expect(sites.length, `只扫到 ${sites.length} 处写调用 —— 判据面塌了`).toBeGreaterThan(4);
    expect(spans.length, '一次 `runWrite(` 调用都没扫到').toBeGreaterThan(3);
    const escaped = sites.filter((s) => !spans.some(([a, b]) => s.index > a && s.index < b));
    expect(
      escaped.map((x) => x.text),
      '这些写调用不在 `runWrite` 里 —— 它们失败时用户看不到任何东西：' +
        '状态弹回原值，一句话都没有（裁定 #14 要抓的正是这个形态）',
    ).toEqual([]);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑤ 采样与聚合的纯逻辑（`traffic-chart.md` / 三张分析卡 + `data-contract.json` 的 traps）
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑤ 流量采样：按秒差分、计数器归零重启缓冲', () => {
  it('第一帧只记基线，不出点（没有可差分的对象）', () => {
    const b = pushTotals(EMPTY_TRAFFIC_BUFFER, 1000, 2000, 0);
    expect(b.samples).toEqual([]);
    expect(b.baseline).not.toBeNull();
  });

  it('满一秒才出一个点，速率 = 增量 ÷ 实测 Δt', () => {
    let b = pushTotals(EMPTY_TRAFFIC_BUFFER, 0, 0, 0);
    b = pushTotals(b, 100, 300, 500); // 不足一秒 ⇒ 原样返回
    expect(b.samples).toEqual([]);
    b = pushTotals(b, 200, 600, 2000); // Δt=2s
    expect(b.samples).toEqual([{ up: 100, down: 300 }]);
  });

  it('计数器变小 = 新的一条核生命线 ⇒ **丢掉那个差分并清空缓冲**，不画负值也不画尖峰', () => {
    let b = pushTotals(EMPTY_TRAFFIC_BUFFER, 0, 0, 0);
    b = pushTotals(b, 5000, 5000, 1000);
    expect(b.samples.length).toBe(1);
    b = pushTotals(b, 10, 10, 2000); // 停核重启，计数器归零
    expect(b.samples, '把两条生命线的速率画进了同一根曲线').toEqual([]);
    expect(b.baseline).toEqual({ up: 10, down: 10, at: 2000 });
  });

  it('窗口封顶在 60 点（最新在尾）', () => {
    let b = pushTotals(EMPTY_TRAFFIC_BUFFER, 0, 0, 0);
    for (let i = 1; i <= TRAFFIC_WINDOW + 10; i += 1) b = pushTotals(b, i, i, i * 1000);
    expect(b.samples.length).toBe(TRAFFIC_WINDOW);
  });

  it('全零窗口的刻度上限不为 0（否则整条 path 变 NaN，曲线会**整条消失**）', () => {
    expect(scaleMax([])).toBeGreaterThan(0);
    expect(scaleMax([{ up: 0, down: 0 }])).toBeGreaterThan(0);
  });

  it('平滑后夹取：控制点不许把曲线推到 0 以下或刻度以上', () => {
    const d = splinePath([0, 1000, 0, 1000, 0], 300, 100, 1000);
    const ys = [...d.matchAll(/[-\d.]+\s+([-\d.]+)/g)].map((m) => Number(m[1]));
    expect(ys.length).toBeGreaterThan(4);
    expect(Math.min(...ys)).toBeGreaterThanOrEqual(0);
    expect(Math.max(...ys)).toBeLessThanOrEqual(100);
  });

  it('少于两个点不成曲线 ⇒ 空 path（调用方据此渲染「无样本」而不是画一个孤点）', () => {
    expect(splinePath([], 300, 100, 10)).toBe('');
    expect(splinePath([5], 300, 100, 10)).toBe('');
  });

  it('无样本时**不编一个 0**（那是「还没测到」，不是「测到了 0」）', () => {
    for (const samples of [[], [{ up: 0, down: 0 }]]) {
      const html = render({ samples });
      expect(html).toContain('mobileHome.noSamples');
      expect(html).not.toContain('class="h-ymax"');
      expect(html).not.toContain('class="h-ymin"');
      expect(html).not.toContain('class="h-xaxis"');
    }
    const sampled = render({ samples: [{ up: 0, down: 0 }, { up: 1, down: 10 }] });
    expect(sampled).not.toContain('mobileHome.noSamples');
    expect(sampled).toContain('class="h-ymax"');
    expect(sampled).toContain('class="h-xaxis"');
  });
});

describe('⑥ 活动连接聚合：按字节、口径与后端同源', () => {
  const entry = (over: Record<string, unknown>) =>
    ({ id: 'x', chains: [], rule: 'final', ...over }) as never;

  it('outbound 取 `chains[0]`（后端 `aggregator.rs` 的口径），空链回落 Direct', () => {
    expect(outboundOf(entry({ chains: ['proxy-a', 'group-b'] }))).toBe('proxy-a');
    expect(outboundOf(entry({ chains: [] }))).toBe('Direct');
    expect(outboundOf(entry({ chains: [''] }))).toBe('Direct');
  });

  it('按**字节**聚合，不是按条数（`ConnectionAggOutbound.count` 数的是连接）', () => {
    const es = [
      entry({ id: '1', chains: ['a'], upload: 10, download: 90 }),
      entry({ id: '2', chains: ['b'], upload: 1, download: 1 }),
      entry({ id: '3', chains: ['b'], upload: 1, download: 1 }),
      entry({ id: '4', chains: ['b'], upload: 1, download: 1 }),
    ];
    // 条数上 b 是 a 的三倍，字节上 a 远大于 b —— 排序必须按字节。
    expect(compositionOf(es).map((s) => s.key)).toEqual(['a', 'b']);
    expect(totalBytesOf(es)).toBe(106);
  });

  it('缺席的 upload/download 按 0 参与排名（但呈现层不得把它印成一个测得的 0）', () => {
    expect(totalBytesOf([entry({ id: '1', chains: ['a'] })])).toBe(0);
  });

  it('host 名回落链：host > destinationIP > rule；Top N 截断且排序稳定', () => {
    const es = [
      entry({ id: '1', metadata: { host: 'a.com' }, download: 5 }),
      entry({ id: '2', metadata: { destinationIP: '1.2.3.4' }, download: 5 }),
      entry({ id: '3', rule: 'final', download: 5 }),
    ];
    expect(hostTopOf(es, 3).map((h) => h.key).sort()).toEqual(['1.2.3.4', 'a.com', 'final']);
    expect(hostTopOf(es, 2).length).toBe(2);
  });

  it('原生统计的 null 元数据回落到 IP / 规则，等字节排序和搜索均可用', () => {
    const es = [
      entry({ id: 'domain', metadata: { host: 'example.com' }, download: 0 }),
      entry({ id: 'ip', metadata: { host: null, destinationIP: '1.1.1.1' }, download: 0 }),
      entry({ id: 'rule', metadata: { host: null, destinationIP: null }, rule: 'final', download: 0 }),
      entry({ id: 'empty', metadata: { host: '', destinationIP: '' }, rule: 'final', download: 0 }),
    ];
    expect(hostTopOf(es).map((h) => h.key)).toEqual(['1.1.1.1', 'example.com', 'final']);
    expect(filterByHostQuery(es, '1.1').map((e) => e.id)).toEqual(['ip']);
    expect(filterByHostQuery(es, 'FINAL').map((e) => e.id)).toEqual(['rule', 'empty']);
  });

  /*
   * 主机筛选（批 18 接上的那一维）。判据是后端 `project_connections_topology_iter` 那段 filter
   * 的 1:1 转写，故这一组逐条对拍的是**那段 Rust 的行为**，不是我们自己新发明的口径：
   * `trim().to_lowercase()`、空串放行全部、host **或** outbound 任一含子串。
   */
  it('主机筛选：host 或 outbound 任一命中；大小写不敏感、首尾空白不算', () => {
    const es = [
      entry({ id: '1', metadata: { host: 'Cdn.Example.COM' }, chains: ['proxy-a'], download: 9 }),
      entry({ id: '2', metadata: { host: 'other.net' }, chains: ['HK-Relay'], download: 8 }),
      entry({ id: '3', metadata: { host: 'nothing.org' }, chains: ['direct'], download: 7 }),
    ];
    // host 侧命中，且两端的大小写都不参与判定。
    expect(filterByHostQuery(es, 'EXAMPLE').map((e) => e.id)).toEqual(['1']);
    // outbound 侧命中 —— 少了这一半，按出口节点找流量整条不可达（桌面那个框的占位符
    // 逐字写着「域名 / IP / 出口节点」）。
    expect(filterByHostQuery(es, 'hk-relay').map((e) => e.id)).toEqual(['2']);
    // 首尾空白不算（触屏长按粘贴很容易带一个尾空格，按字面匹配会一条都搜不到）。
    expect(filterByHostQuery(es, '  example  ').map((e) => e.id)).toEqual(['1']);
    // 空串 / 纯空白 = 不筛，整集原样放行（不是「一条都不匹配」）。
    expect(filterByHostQuery(es, '').map((e) => e.id)).toEqual(['1', '2', '3']);
    expect(filterByHostQuery(es, '   ').map((e) => e.id)).toEqual(['1', '2', '3']);
    // 反向对照：确实筛得掉（否则上面几条可能只是「恒不过滤」的假绿）。
    expect(filterByHostQuery(es, 'zzz')).toEqual([]);
  });

  it('🔴 筛选发生在**排名之前** —— 否则只能在已经看得见的那几行里筛，等于没筛', () => {
    const es = [
      entry({ id: '1', metadata: { host: 'big-a.com' }, download: 1000 }),
      entry({ id: '2', metadata: { host: 'big-b.com' }, download: 900 }),
      entry({ id: '3', metadata: { host: 'big-c.com' }, download: 800 }),
      entry({ id: '4', metadata: { host: 'big-d.com' }, download: 700 }),
      entry({ id: '5', metadata: { host: 'big-e.com' }, download: 600 }),
      // 第 6 名：按字节永远进不了 Top 5，唯一够得着它的路就是先筛后排名。
      entry({ id: '6', metadata: { host: 'tiny.example.org' }, download: 1 }),
    ];
    expect(hostTopOf(es).map((h) => h.key), '基线：它本来就不在 Top 5 里').not.toContain(
      'tiny.example.org',
    );
    expect(hostTopOf(filterByHostQuery(es, 'tiny')).map((h) => h.key)).toEqual([
      'tiny.example.org',
    ]);
    // 接线层真的是这个顺序，不是在渲染出来的 5 行上二次过滤。
    expect(
      WIRING_RAW,
      '接线层没有「先筛后排名」这条腿 —— 判据在这里，呈现层只负责画',
    ).toContain('hostTopOf(filterByHostQuery(entries, hostQuery))');
  });

  it('规则命中：同名多条件并组，策略取最多者；同字业务类别仍隔离', () => {
    const es = [
      entry({ id: '1', rule: 'domain=a.com', ruleName: '直连', chains: ['direct'] }),
      entry({ id: '2', rule: 'ip_cidr=1.2.3.4/32', ruleName: '直连', chains: ['proxy'] }),
      entry({ id: '3', rule: 'rule_set=x', ruleName: '直连', chains: ['proxy'] }),
      entry({ id: '4', rule: 'domain=b.com', chains: ['direct'] }),
    ];
    const hits = ruleHitsOf(es);
    expect(hits.reduce((sum, hit) => sum + hit.count, 0)).toBe(es.length);
    expect(hits).toEqual([
      { key: 'named:直连', rule: '直连', kind: 'named', policy: 'proxied', count: 3 },
      { key: 'policy:direct', rule: 'direct', kind: 'policy', policy: 'direct', count: 1 },
    ]);
  });

  it('增量流：`generation` 换代 ⇒ **整表换**，不许把两个数据集合并', () => {
    const g1 = applyDetail(
      { generation: -1, byId: new Map() },
      { reset: true, generation: 1, sequence: 1, connections: [entry({ id: 'old' })], at: 0 },
    );
    expect([...g1.byId.keys()]).toEqual(['old']);
    const g2 = applyDetail(g1, {
      reset: false,
      generation: 2,
      sequence: 1,
      connections: [entry({ id: 'new' })],
      at: 1,
    });
    expect([...g2.byId.keys()], '忽略 generation 会在一次重连之后混两份数据集').toEqual(['new']);
  });

  it('常态帧的 counters 就地合并，不整条替换（否则会把 chains/rule 抹掉）', () => {
    const t0 = applyDetail(
      { generation: 1, byId: new Map() },
      { reset: true, generation: 1, sequence: 1, connections: [entry({ id: 'a', chains: ['p'] })], at: 0 },
    );
    const t1 = applyDetail(t0, {
      reset: false,
      generation: 1,
      sequence: 2,
      connections: [],
      counters: [{ id: 'a', upload: 7, download: 8 }],
      at: 1,
    });
    expect(t1.byId.get('a')?.chains).toEqual(['p']);
    expect(totalBytesOf([...t1.byId.values()])).toBe(15);
  });

  it('源码面：三张分析卡一处都没用那两个 `count` 字段（它们数的是连接不是字节）', () => {
    for (const banned of ['ConnectionAggOutbound', 'ConnectionAggHost', 'onConnectionsAggregate']) {
      expect(HOME_TEXT, `用了 ${banned} —— 它是连接条数，不是字节数`).not.toContain(banned);
    }
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑦ 各卡的「不许把不知道的事说成知道」
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑦ 无源不画：陈旧 / 哨兵 / 隐私 / 空态', () => {
  it('延迟陈旧**渲染成陈旧**，不许印成一个数', () => {
    const html = render({
      node: { kind: 'node', name: 'HK03', protocolLabel: 'VLESS', flagSrc: null, latencyMs: 42, latencyStale: true },
    });
    expect(html).toContain('nodes.mobileLatencyStale');
    expect(html).not.toContain('42 ms');
  });

  it('哨兵选择不是节点：直连 / 阻断各有如实标签，且不画协议与旗', () => {
    const direct = render({ node: { kind: 'direct' } });
    expect(direct).toContain('home.routingDirect');
    expect(direct).not.toContain('h-flag');
    const block = render({ node: { kind: 'block' } });
    expect(block).toContain('home.routingBlock');
  });

  it('隐私模式**遮蔽域名而不是藏起整张卡**（流量形状仍可见）', () => {
    const html = render({ hostsMasked: true });
    expect(html).toContain('data-home-card="host-traffic-top"');
    expect(html).toContain('connections.privacyHidden');
    expect(html).not.toContain('example.com');
    expect(html, '条还在 ⇒ 形状仍然可见').toContain('h-hostbar');
  });

  it('出口国家未探到时显示「待定」，不回落一个猜的国家', () => {
    const html = render({ exitRegion: { kind: 'none' }, exitFlagSrc: null });
    expect(html).toContain('home.unlockStatus.idle');
  });

  it('出口地区的文本分支过了 `localizeRegion`（否则境外直连出口会把裸 `US` 呈现给用户）', () => {
    expect(
      /localizeRegion\(\s*exitRegionRaw\.region/.test(WIRING_TEXT),
      '`resolveExitRegion` 的 text 分支刻意不本地化（好让它 node 直测），调用方必须自己补这一道',
    ).toBe(true);
  });

  it('三张分析卡共用同一个窗口标签，且卡头的总数就是各部分之和', () => {
    const html = render();
    expect((html.match(/connections\.active/g) ?? []).length).toBeGreaterThanOrEqual(2);
    const sum = baseProps().composition.reduce((a, s) => a + s.bytes, 0);
    expect(sum, '卡头印的总数不是各段之和').toBe(baseProps().windowBytes);
  });

  /** 渲染出的某颗控件的**开标签**（用来判 `disabled`）。 */
  const openTagOf = (html: string, control: string): string => {
    const at = html.indexOf(`data-write-control="${control}"`);
    expect(at, `${control} 没渲染`).toBeGreaterThan(-1);
    return html.slice(html.lastIndexOf('<', at), html.indexOf('>', at) + 1);
  };

  it('起核/停核在飞时连接按钮不可点（否则会在已有起核腿之上再叠一次 start）', () => {
    for (const core of ['starting', 'stopping'] as const) {
      expect(openTagOf(render({ core }), 'connect'), `${core} 档按钮仍可点`).toContain('disabled');
    }
  });

  it('反向对照：稳态下它是可点的（证明上一条不是恒真）', () => {
    for (const core of ['running', 'stopped'] as const) {
      expect(openTagOf(render({ core }), 'connect')).not.toContain('disabled');
    }
  });

  it('检测在飞时两颗检测入口都不可点（重入 = 排一次多余的检测）', () => {
    const html = render({ unlockRunning: true });
    expect(openTagOf(html, 'network-check')).toContain('disabled');
    const idle = render({ unlockRunning: false });
    expect(openTagOf(idle, 'network-check')).not.toContain('disabled');
  });

  it('核心状态带**文字**，不是只有一个彩色圆点', () => {
    const html = render({ core: 'error' });
    expect(html.slice(html.indexOf('<header'), html.indexOf('</header>'))).toContain('home.statusProxyUnavailable');
  });
});

describe('⑧ 解锁服务表：登记表与上线集对得上（桌面改了上线集而这边没跟 ⇒ 红）', () => {
  it('id 集合恰等于 `ENABLED_SERVICE_IDS`（七项，grok 不在其中）', () => {
    expect(UNLOCK_SERVICES.map((s) => s.id).sort()).toEqual([...ENABLED_SERVICE_IDS].sort());
    expect(UNLOCK_SERVICES.map((s) => s.id)).not.toContain('grok');
    for (const service of UNLOCK_SERVICES) {
      expect(unlockBadgeSrc(service.id), `${service.id} 的图标缺失会留下只有圆点的空按钮`).not.toBeNull();
    }
  });

  it('保留桌面的组序（AI 三个在前，流媒体四个在后）', () => {
    expect(UNLOCK_SERVICES.map((s) => s.group)).toEqual([
      'ai',
      'ai',
      'ai',
      'stream',
      'stream',
      'stream',
      'stream',
    ]);
  });

  it('七态各有一档状态点，且 `blocked` 与 `restricted` 不共用一档', () => {
    for (const st of ['idle', 'checking', 'ok', 'partial', 'blocked', 'restricted', 'timeout'] as const) {
      const html = render({
        unlock: [{ id: 'chatgpt', name: 'ChatGPT', badgeSrc: null, result: { status: st } }],
      });
      expect(html, `${st} 档没渲染`).toContain(`h-stdot st-${st}`);
      expect(html).toContain(`aria-label="ChatGPT: home.unlockStatus.${st}"`);
      expect(html).toContain(`aria-busy="${st === 'checking'}"`);
    }
    const blocked = /\.h-stdot\.st-blocked\s*\{[^}]*\}/.exec(HOME_CSS)?.[0] ?? '';
    const restricted = /\.h-stdot\.st-restricted[^{]*\{[^}]*\}/.exec(HOME_CSS)?.[0] ?? '';
    expect(blocked.length, 'blocked 没有自己的样式 ⇒ 判据面塌了').toBeGreaterThan(0);
    expect(restricted.length).toBeGreaterThan(0);
    expect(
      blocked.replace(/^[^{]*/, ''),
      'blocked 与 restricted 被合成了同一个颜色 —— 那会抹掉检测引擎产出的唯一可行动差异',
    ).not.toBe(restricted.replace(/^[^{]*/, ''));
  });

  it('徽章底色两个主题都是白的（不是 surface token）—— 多色/带黑描边的标记才不会消失', () => {
    expect(/\.h-badge\s*\{[^}]*background:\s*#fff/.test(HOME_CSS)).toBe(true);
  });

  it('失败态去饱和（颜色之外的第二条通道），而 `partial` **不**去饱和（它是真实结果不是失败）', () => {
    const rule = /((?:\.h-badge\.st-[a-z]+,?\s*)+)\{[^}]*grayscale\(1\)/.exec(HOME_CSS)?.[1] ?? '';
    expect(rule).toContain('st-idle');
    expect(rule).toContain('st-timeout');
    expect(rule).toContain('st-blocked');
    expect(rule).not.toContain('st-partial');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑨ **接管方式不是移动端的一个选择**（`polaris-mobile-ia-adjudications-2026-09-04.md` §3 的
 *   能力缺席口径；先例是 `../settings/TunPage.tsx` 的 `strict_route`）。
 *
 * # 为什么是移除，不是置灰
 *
 * 三档接管里另外两档（`systemProxy` / `manual`）全靠 `mixed-in` 本地代理入站，而
 * `crates/config-engine/src/builder/inbounds.rs` 在 `platform == "android"` 时**根本不发**它 ⇒
 * 在 Android 上选中它们产出的配置里一个用户流量入站都没有，而且不报错。配置默认值又恰好是
 * `systemProxy`。**一个承诺了某件事却什么都不做的控件，比没有这个控件更坏** —— 用户会照着它行动。
 *
 * # 三条腿，缺一条这道门就有缝
 *
 *  A **呈现层**：那颗芯片与它那张弹层一处都不渲染（含四条接管文案）。
 *  B **接线层**：本屏不再从 `config.proxyModeType` 取接管方式 —— 光把 UI 删掉、接线还在读那个字段，
 *    就是「机制有门、接线没门」：盘上默认值是 `systemProxy`，连接三态会走进一条移动端不存在的
 *    降级分支。故接线那一半单独有一条判据。
 *  C **正面等式**：只写「不许出现 X」会被「什么都没发生」骗过 —— 整张卡被误删、或者分流策略跟着
 *    一起没了，同样满足 A。故这一组同时钉住**分流策略三档仍在、仍可切、仍写进配置**，
 *    以及芯片行**恰好两颗**（一颗可切、一颗读出），不是一个空槽也不是一颗孤零零的芯片。
 *
 * # 回放
 *
 * 下面每条否定判据都配一份**合成的旧形态**（`TAKEOVER_ERA_MARKUP` / `TAKEOVER_ERA_WIRING`，
 * 逐字重建移除前的产物），证明同一个扫描器对着「把选择器加回去」确实报得出来、且点得出名字。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑨ 接管方式：整颗缺席，而分流策略照旧可切', () => {
  /** 四条接管文案键。呈现层用的 `t` 夹具原样回键名，故它们在 markup 里就是这四个字符串。 */
  const TAKEOVER_COPY = [
    'home.takeoverMethod',
    'home.takeoverSystemProxy',
    'home.takeoverTun',
    'home.takeoverManual',
  ] as const;

  /** 扫描器 A：这段 markup 里出现了哪几条接管文案。真判据与回放对照共用同一份实现。 */
  const takeoverCopyIn = (html: string): string[] => TAKEOVER_COPY.filter((k) => html.includes(k));

  /** 扫描器 B：「从配置里读接管方式」的形态（`config.proxyModeType` / `config?.proxyModeType`）。 */
  const configTakeoverReads = (src: string): string[] =>
    [...src.matchAll(/config\s*\??\.\s*proxyModeType/g)].map((m) => m[0]);

  /** 扫描器 C：`mode-chips` 卡里每颗芯片的修饰类（`sel` = 可切，`readout` = 读出）。 */
  const chipsOf = (html: string): string[] =>
    [
      ...(cardSegments(html).get('mode-chips') ?? '').matchAll(/class="h-chip(?: ([a-z-]+))?"/g),
    ].map((m) => m[1] ?? '');

  /** 弹层全开的一屏（接管方式若还在，它的芯片与选项都会在这一份里出现）。 */
  const openHtml = render({ pickerOpen: true });

  /**
   * 回放：**移除前**那一版的产物形态，逐字重建（三颗芯片 + 接管弹层 + 三个接管选项）。
   * 真组件已经没有这段了，故对照只能是合成的；它存在的唯一理由是证明上面三个扫描器不是死的。
   */
  const TAKEOVER_ERA_MARKUP =
    '<section class="h-card" data-home-card="mode-chips">' +
    '<div class="h-chips">' +
    '<button type="button" class="h-chip sel" data-write-control="takeover">' +
    '<span class="h-chiplabel copy">home.takeoverTun</span></button>' +
    '<button type="button" class="h-chip sel" data-write-control="routing">' +
    '<span class="h-chiplabel copy">home.routingSmart</span></button>' +
    '<button type="button" class="h-chip readout" data-write-control="exit-probe"></button>' +
    '</div>' +
    '<div class="h-sheet" role="dialog" aria-label="home.takeoverMethod">' +
    '<ul class="h-optlist"><li><button class="h-opt">home.takeoverSystemProxy</button></li>' +
    '<li><button class="h-opt cur">home.takeoverTun</button></li>' +
    '<li><button class="h-opt">home.takeoverManual</button></li></ul></div>' +
    '</section>' +
    '<section class="h-card" data-home-card="unlock-detection"></section>';

  /** 回放的接线那一半：移除前接线层逐字长这样。 */
  const TAKEOVER_ERA_WIRING = "const takeover = (config?.proxyModeType ?? 'systemProxy');";

  it('自检：三个扫描器对着**加回去**的旧形态确实报得出来，且点得出名字', () => {
    expect(
      takeoverCopyIn(TAKEOVER_ERA_MARKUP).sort(),
      '接管文案扫描器对着旧形态一条都没报 —— 下面那条否定判据是死的',
    ).toEqual([...TAKEOVER_COPY].sort());
    expect(
      configTakeoverReads(TAKEOVER_ERA_WIRING),
      '「从配置读接管方式」扫描器对着旧形态没报 —— 接线那条腿是死的',
    ).toEqual(['config?.proxyModeType']);
    expect(chipsOf(TAKEOVER_ERA_MARKUP), '旧形态是三颗芯片，扫描器却没数出来').toEqual([
      'sel',
      'sel',
      'readout',
    ]);
  });

  /**
   * 桌面 `components/Icons.tsx` 里某个字形组件的源码段。切不出来就**抛**（空串会让下面恒绿）。
   *
   * 出处比对读的是**桌面源码本身**，不是本文件里抄一份 path —— 抄一份就与出处脱钩了，
   * 桌面那枚改了形状而移动端没跟，比对照样绿（`GLYPH_PATHS` 那张表是更早的写法，留着不动）。
   */


  /**
   * 模式芯片的**前导字形**（UNBLOCK-K1b-18）。
   *
   * `mode-chips.md`「Anatomy」写的是「每颗芯片：前导字形 + 短标签」，原型 `.chip` 里那颗 `#i-smart`
   * = `icon-registry.json` 的 `mode.smart-routing` → `components/Icons.tsx#NavAppPolicyIcon`。
   * 它此前被 `config-sync.test.tsx` 的 `routingChipOf` 正则挡在门外整整一轮（那条正则要求标签
   * 紧跟按钮开标签），2026-09-05 那条正则改成「先切整颗芯片再找标签」后补上。
   *
   * 宽度模型同批跟上：`styles/text-fit.test.ts` 的 `CHIP_SLOT.mode.has.icon` 与
   * `chipSiblingClasses()` 的兄弟集合对拍；标签可用宽 138 → 112，ru 的 `Глобальный`
   * 因此进了 `CHIP_OVER_WIDTH` 裁切登记表（如实登记，见那张表的头注）。
   */


  it('A 呈现层：四条接管文案一处都不渲染（芯片、弹层标题、三个选项）', () => {
    expect(
      takeoverCopyIn(openHtml),
      '首页又出现了接管方式 —— Android 上那三档里只有 TUN 存在，另外两档产出的配置没有任何用户流量入站',
    ).toEqual([]);
    // 源码面同扫一遍：文案表被留在文件里（哪怕暂时没渲染）就是下一次「顺手接回去」的种子。
    expect(HOME_TEXT).not.toContain('TAKEOVER_KEY');
    expect(HOME_TEXT).not.toContain('onSetTakeover');
    expect(WRITE_CONTROLS as readonly string[]).not.toContain('takeover');
  });

  it('B 接线层：本屏不再从 `config.proxyModeType` 取接管方式（删了 UI 却还在读 = 门有缝）', () => {
    expect(
      configTakeoverReads(HOME_TEXT),
      '本屏又在从配置读接管方式：盘上默认值是 `systemProxy`，而移动端已经没有控件能改它 —— ' +
        '连接三态会走进「系统代理未生效」这条 Android 上根本不存在的降级分支',
    ).toEqual([]);
    // 正面对照：接管方式仍然被**当成平台事实**喂进那条共用的判定腿（不是干脆不传了）。
    expect(WIRING_RAW).toMatch(/MOBILE_TAKEOVER\s*:\s*ProxyModeType\s*=\s*'tun'/);
    expect(WIRING_TEXT).toMatch(/deriveTakeoverConnState\(\{[^}]*proxyModeType:\s*MOBILE_TAKEOVER/s);
  });





  it('C 正面等式：分流策略**仍写进配置**，且这条写腿仍在唯一写出口里（机制 + 接线成对）', () => {
    // 接线：那颗选项按钮真的把值送进了 `onSetRouting`（呈现层这一半）。
    expect(HOME_TEXT).toContain('props.onSetRouting(v)');
    // 接线：`onSetRouting` 真的写 `proxyMode`，且整条写调用在 `runWrite('routing', …)` 的参数区间里
    //（④ 那条通扫是集合级的；这里点名钉住分流策略这一条，否则它被删掉只会让那条通扫「少一处」）。
    expect(
      WIRING_RAW,
      '分流策略的写腿不见了 —— 这一屏就只剩两颗印着字的芯片，点了什么也不会发生',
    ).toMatch(/runWrite\(\s*'routing'\s*,\s*\(\)\s*=>\s*update\(\{\s*proxyMode:\s*v\s*\}\)\s*\)/);
    // 反面：不许有任何一处写回 `proxyModeType`（那正是被移除的那条腿）。
    expect(WIRING_TEXT).not.toMatch(/update\(\{\s*proxyModeType/);
  });


});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑨ 动作行 / 连接态徽标的**字形层**：真的画在生产渲染路径上。
 *
 * 这一层原本整个缺席 —— 三颗动作按钮只有一行字，连接态徽标是一颗空心圆。真机上的读感是
 * 「首页的启动按钮不明显」：三个等宽的灰字，没有任何一颗看起来像主操作。
 *
 * ── 为什么这一组必须是**渲染腿**，不是源码腿 ──────────────────────────────────
 * 本项目反复撞的是「机制有门、接线没门」：字形组件写好了、CSS 也写好了，但 JSX 里没挂，
 * 或者挂在一个永不为真的分支里。`HOME_TEXT.includes('ActionGlyph')` 对这两种都恒绿。
 * 故下面每一条都吃 `renderToStaticMarkup` 的真 HTML，并且**每条都配一个反向对照**
 * （反态下那一层确实不在），证明断言不是「什么都没发生也算过」。
 *
 * ── 取材面自检 ──────────────────────────────────────────────────────────────
 * `actsOf()` 切不出动作行时**抛**，不返回空串：切法一失效，下面每条 `toContain` 都会对着
 * 空串恒绿，而那正是这一层第一次消失时门没说话的原因。
 *
 * ── 字形本身也钉住 ──────────────────────────────────────────────────────────
 * `icons/icon-registry.json` 的 policy 是 `reuse-current-client-only` / `allowRedesign:false`：
 * 下面把三枚动作字形与那枚对勾的 path data **逐字**钉在原型 `<symbol id="i-…">` 上。
 * 有人「顺手换一套更好看的图标」时这一组会红，而不是等真机截图对比才发现。
 * （本文件不在 `HOME_TEXT` 取材面里，见头注「判据的自污染」⇒ 在这里写字面量不会污染别的门。）
 * ═════════════════════════════════════════════════════════════════════════ */

/** 新视觉的结构结果：动作与状态不再绑在旧三按钮行/诊断行里。 */
describe('⑨–⑫ 出口控制、检测与规则统计的当前呈现契约', () => {
  afterAll(closeOracle, 30_000);

  const headerOf = (html: string): string => html.slice(html.indexOf('<header'), html.indexOf('</header>') + 9);

  it('连接是页头独立动作，运行态文字同行且忙态不可重入', () => {
    const running = headerOf(render({ core: 'running', connState: 'connected' }));
    expect(running).toContain('data-write-control="connect"');
    expect(running).toContain('mobileHome.disconnect');
    expect(running).toContain('home.statusConnected');
    expect(running).toContain('class="h-connect-action is-disconnect"');
    for (const core of ['starting', 'stopping'] as const) {
      const busy = headerOf(render({ core }));
      expect(busy).toContain('aria-busy="true"');
      expect(busy).toContain('disabled');
    }
  });

  it('三种路由在同一互斥组，选中状态与动作标签均可读', () => {
    const html = render({ routing: 'global' });
    const route = html.slice(html.indexOf('class="h-routing"'), html.indexOf('</div>', html.indexOf('class="h-routing"')));
    expect((route.match(/class="h-route-option/g) ?? [])).toHaveLength(3);
    expect((route.match(/aria-pressed="true"/g) ?? [])).toHaveLength(1);
    for (const key of ['home.routingSmart', 'home.routingGlobal', 'home.routingDirect']) expect(route).toContain(key);
  });

  it('七项能力只显示图标与圆点；名称和状态留在可访问名称及点击详情', () => {
    const html = cardSegments(render()).get('unlock-detection') ?? '';
    expect((html.match(/class="h-unlock"/g) ?? [])).toHaveLength(7);
    expect((html.match(/class="h-badge /g) ?? [])).toHaveLength(7);
    expect(html).not.toContain('class="h-unlockname"');
    expect((html.match(/class="h-stdot /g) ?? [])).toHaveLength(7);
    expect(html).not.toContain('class="h-unlockstatus"');
    for (const svc of baseProps().unlock) {
      expect(html).toContain(`aria-label="${svc.name}: home.unlockStatus.${svc.result.status}"`);
    }
    expect(html).toContain('data-write-control="network-check"');
    expect(html).toContain('mobileHome.exitIp');
  });

  it('当前与全部测速各有明确动作，错误/不可测原因留在节点控制区', () => {
    const html = render({ latencyCheck: {
      busyKind: 'all', error: 'request failed', feedback: null,
      blocked: 'connect first', blockedStatus: 'idle', allUnavailable: null,
      onRunCurrent: () => {}, onRunAll: () => {},
    } });
    const panel = html.slice(html.indexOf('class="h-control-panel"'), html.indexOf('data-home-card="unlock-detection"'));
    expect(panel).toContain('mobileHome.currentSpeedTest');
    expect(panel).toContain('mobileHome.allSpeedTestBusy');
    expect(panel).toContain('connect first');
    expect(panel).toContain('role="alert"');
  });

  it('非空卡头提供全部入口，空态不提供失效入口；完整列表不截断聚合项', () => {
    expect(render()).toContain('mobileHome.ruleHitsViewAll');
    expect(render({ ruleHits: [] })).not.toContain('mobileHome.ruleHitsViewAll');
    const hits = Array.from({ length: 35 }, (_, index) => ({
      key: `named:rule-${index}`, rule: `rule-${index}`, kind: 'named' as const,
      policy: index === 0 ? 'blocked' as const : 'proxied' as const, count: index + 1,
    }));
    const html = renderToStaticMarkup(<RuleHitsSheet ruleHits={hits} windowConnections={630} t={t}
      onClose={() => {}} onOpenRuleHit={() => {}} />);
    expect((html.match(/class="h-ringcell"/g) ?? []).length).toBe(35);
    expect(html).toContain('aria-label="rule-34 · 35"');
    expect(html).toContain('connections.active');
    expect(html).toContain('class="h-ring p-block"');
    expect(html).toContain('<span class="h-ringpolicy">home.routingBlock</span>');
    expect(html).not.toContain('h-rings"');
    const empty = renderToStaticMarkup(<RuleHitsSheet ruleHits={[]} windowConnections={0} t={t} onClose={() => {}} />);
    expect(empty).toContain('role="status">connections.noActive');
  });

  it('规则卡只显可读名称，条件留在下钻明细', () => {
    const html = cardSegments(render({
      ruleHits: [{ key: 'named:流媒体解锁', rule: '流媒体解锁', kind: 'named', policy: 'proxied', count: 5 }],
      onOpenRuleHit: () => {},
    })).get('rule-hits') ?? '';
    expect(html).toContain('<span class="h-ringname">流媒体解锁</span>');
    expect(html).toContain('class="h-ringval">5</text>');
    expect(html).toContain('aria-label="流媒体解锁 · 5"');
    expect(html).not.toContain('disabled=""');
  });

  it('浏览器实算：主连接动作与次动作分层，状态和检测可随字号自然换行', async () => {
    const m = await measure({
      ctx: 'mobile', html: inMobileShell(render({ core: 'stopped', connState: 'disconnected' }), 'home'),
      rootAttrs: { 'data-theme': 'light' }, viewport: { width: 320, height: 740 }, probes: {},
    }, [
      { select: '.h-connect-action', props: ['background-color', 'min-height'] },
      { select: '.h-connect-action', pseudo: '::before', props: ['background-color'] },
      { select: '.h-control-panel .h-act', props: ['background-color'] },
    ]);
    expect(m.count('.h-connect-action')).toBe(1);
    expect(m.count('.h-control-panel .h-act')).toBeGreaterThan(0);
    expect(m.get('.h-connect-action', 'background-color')).toBe('rgba(0, 0, 0, 0)');
    expect(m.get('.h-connect-action', 'background-color', '::before')).not.toBe(
      m.get('.h-control-panel .h-act', 'background-color'),
    );
  }, 30_000);
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑬ 批 12 接上的四条能力，各自的**正面事实断言**。
 *
 * 为什么必须有这一组：`wiring-completeness.test.ts` 的头注把自己的天花板写得很清楚 ——
 * 它守得住「文案还在」，守不住「文案删了但功能也没接」。把登记表里一条 `absent` 改成 `ported`
 * 再配一个能对上的锚，那道门就会变绿，**而锚只证明那段文本在**。这一组断言的是那四条能力
 * 今天确实渲染得出、且接在真腿上；少了它，销账与删文案在 CI 里长得一模一样。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑬ 哨兵出口 / 空态入口 / 主机行规则面板：能力真的在', () => {
  /* ── 哨兵出口（`NodeMenu.tsx|k:home.routingDirect` / `routingBlock` / `block[MiCheck]`）── */

  it('选择面里有直连 / 阻断两行，且它们不参与搜索过滤（搜不到节点时仍然要能退回直连）', () => {
    const html = render({ pickerOpen: true, pickerQuery: '不存在的节点', pickRows: [] });
    expect(html).toContain('home.routingDirect');
    expect(html).toContain('home.routingBlock');
    // 反向对照：这一轮确实没有任何节点行（否则「两行在」可能是节点行冒充的）。
    expect(html).toContain('mobileHome.noMatchingNode');
    expect(html).toContain('data-exit-write="home-sentinel-direct"');
    expect(html).toContain('data-exit-write="home-sentinel-block"');
  });

  it('选中态（`MiCheck` 的移动端等价）：哪一行是当前出口由 `node.kind` 说了算', () => {
    const sentinelRow = (html: string, which: string): string => {
      const at = html.indexOf(`data-exit-write="home-sentinel-${which}"`);
      expect(at, `${which} 那一行没渲染出来`).toBeGreaterThan(-1);
      return html.slice(html.lastIndexOf('<button', at), at + 120);
    };
    const direct = render({ pickerOpen: true, node: { kind: 'direct' } });
    expect(sentinelRow(direct, 'direct')).toContain('h-pickrow cur');
    expect(sentinelRow(direct, 'block')).not.toContain('cur');
    const block = render({ pickerOpen: true, node: { kind: 'block' } });
    expect(sentinelRow(block, 'block')).toContain('cur');
    expect(sentinelRow(block, 'direct')).not.toContain('cur');
    // 反向对照：选的是真节点时两行都不带选中态（证明上面两条不是恒真）。
    const node = render({ pickerOpen: true });
    expect(sentinelRow(node, 'direct')).not.toContain('cur');
    expect(sentinelRow(node, 'block')).not.toContain('cur');
  });

  it('直连模式下「阻断」在场置灰 + 写出原因，不留一颗按下去什么也不发生的选项', () => {
    const html = render({ pickerOpen: true, blockDisabledReason: 'home.blockExitUnavailableInDirect' });
    const at = html.indexOf('data-exit-write="home-sentinel-block"');
    const tag = html.slice(html.lastIndexOf('<button', at), at + 260);
    expect(tag, '「阻断」没被禁用').toContain('disabled');
    expect(tag, '置灰了却没说为什么 —— 用户只会反复去点它').toContain(
      'home.blockExitUnavailableInDirect',
    );
    // 反向对照：可选那一档既不禁用、也不带那句原因。
    const open = render({ pickerOpen: true });
    const openTag = open.slice(
      open.lastIndexOf('<button', open.indexOf('data-exit-write="home-sentinel-block"')),
      open.indexOf('data-exit-write="home-sentinel-block"') + 260,
    );
    expect(openTag).not.toContain('disabled');
    expect(openTag).not.toContain('home.blockExitUnavailableInDirect');
  });

  it('接线层：直连与阻断都走权威切换收据，沿用各自名称反馈', () => {
    expect(WIRING_RAW, '没有 onPickSentinel 这条腿').toContain('onPickSentinel');
    expect(WIRING_RAW).toContain('DIRECT_SERVER_ID');
    expect(WIRING_RAW).toContain('BLOCK_SERVER_ID');
    const at = WIRING_RAW.indexOf('const onPickSentinel');
    const body = WIRING_RAW.slice(at, WIRING_RAW.indexOf('const onSetRouting', at));
    expect(body).toContain("if (kind === 'block' && blockDisabledReason !== null) return");
    expect(body).toContain('if (id === selectedServerId)');
    expect(body).toContain('switchServer(id)');
    expect(body).toContain("kind === 'direct' ? 'home.routingDirect' : 'home.routingBlock'");
    expect(body).not.toContain('update({ selectedServerId: id })');
  });

  /* ── 空态两颗直达入口（`HomeScreen.tsx|k:home.addServer` / `addSubscription`）── */

  it('空态给出路：两颗直达入口在，且落到移动端自己的表单宿主', () => {
    const html = render({ noServers: true, node: { kind: 'none' } });
    expect(html).toContain('home.addServer');
    expect(html).toContain('home.addSubscription');
    // 两颗必须在**节点卡**里（空态的出路要贴着「还没选节点」那句话，不能飘到别的卡上）。
    const card = cardSegments(html).get('node-status-card') ?? '';
    expect(card, '两颗入口没落在节点卡上').toContain('home.addServer');
    expect(card).toContain('home.addSubscription');
    // 接线层：开的是 `mobile/forms` 的两支，不是桌面弹窗。
    expect(WIRING_RAW).toContain("openMobileForm({ kind: 'node' })");
    expect(WIRING_RAW).toContain("openMobileForm({ kind: 'sub' })");
  });

  it('反向对照：有节点（或选了哨兵）时不画那两颗 —— 否则它们就是常驻噪音', () => {
    expect(render({ noServers: false })).not.toContain('home.addServer');
    // 判据本体在接线层：哨兵那半句不能省，否则选了直连的人也会看到「去加节点」。
    expect(WIRING_RAW).toContain('servers.length === 0 && !isSentinelSelection(selectedServerId)');
  });

  /* ── 主机行的规则面板（拓扑图那一族六条）── */

  it('判得出对象的主机行是可点的，判不出的不是（不留点下去没反应的触控目标）', () => {
    const html = render({
      hosts: [
        { key: 'example.com', bytes: 700 }, // 域名
        { key: '198.51.100.7', bytes: 300 }, // IP
        // `hostNameOf` 的第三级回落是**规则名**。带空格的规则备注判不出对象 ⇒ 那一行不可点。
        // ⚠️ 如实记在这里：单标签的规则名（`final`）会被 `DOMAIN_RE` 判成域名、于是可点 ——
        // 那与桌面拓扑图**逐字同一个判据**（两边都调 `ruleSubjectForValue`），
        // 不是本屏新引入的偏差；给它加一条 domain 规则无害，只是不命中。
        { key: '内网 直连', bytes: 100 },
      ],
    });
    expect(html.match(/class="h-hostrow act"/g)?.length, '可点的行数不对').toBe(2);
    expect(html, '判不出对象那一行不该是按钮').toContain('class="h-hostrow"');
  });

  it('隐私模式下整列都不可点 —— 否则「点开就看见被遮蔽的那个名字」，遮蔽当场作废', () => {
    const html = render({ hostsMasked: true });
    expect(html).not.toContain('h-hostrow act');
    expect(html, '遮蔽本身要还在（这一条不是靠「整张卡没渲染」蒙过去的）').toContain(
      'connections.privacyHidden',
    );
  });

  it('面板菜单四颗动作齐全，与桌面右键菜单一一对应', () => {
    const html = render({ ruleSubject: SUBJECT_VM });
    for (const key of ['home.ruleProxy', 'home.ruleDirect', 'rules.addNew', 'rules.addExisting']) {
      expect(html, `菜单里少了 ${key}`).toContain(key);
    }
    // 两颗快速规则是**写**控件，必须带写标记（否则它们的失败没有可归因的落点）。
    expect(html.match(/data-write-control="rule-add"/g)?.length).toBeGreaterThanOrEqual(2);
    // 反向对照：面板没开时这几颗一个都不在。
    // ⚠️ **不拿 `rules.addNew` 做对照**：主机行的无障碍名复用了同一条键（`<host> · 新建规则`），
    // 它在面板没开时也在 DOM 里 —— 拿它对照会把「行的 aria-label」误报成「菜单渲染了」。
    const closed = render();
    for (const key of ['rules.addExisting', 'data-write-control="rule-add"', 'h-optlist']) {
      expect(closed, `面板没开却渲染了 ${key}`).not.toContain(key);
    }
  });

  it('已被规则覆盖时如实说出来，但**不禁用**任何一颗动作（客户端判断只是启发式）', () => {
    const html = render({
      ruleSubject: { subject: SUBJECT_VM.subject, coveringName: '流媒体解锁' },
    });
    expect(html).toContain('rules.subjectAlreadyInRule');
    expect(html).toContain('流媒体解锁');
    const menu = html.slice(html.indexOf('rules.subjectAlreadyInRule'));
    expect(menu.slice(0, menu.indexOf('</div>')), '覆盖提示把动作禁掉了').not.toContain('disabled');
  });

  it('「新建规则」视图：三档动作 + 备注占位符就是真正会落盘的那一句', () => {
    const html = render({
      ruleSubject: SUBJECT_VM,
      ruleSubjectView: 'new',
      ruleRemarksHint: 'REMARKS-HINT',
    });
    for (const action of RULE_ACTION_ORDER) {
      expect(html, `少了 ${action} 这一档`).toContain(RULE_ACTION_KEY[action]);
    }
    expect(html, '三档不是单选（读屏说不出「三选一，当前第几个」）').toContain('role="radiogroup"');
    expect(html).toContain('aria-checked="true"');
    expect(html, '占位符与落盘值不同源 —— 灰着一句、存进去的是另一句').toContain(
      'placeholder="REMARKS-HINT"',
    );
    expect(html, '提交键不见了').toContain('h-sheetsubmit');
    // 「阻断」那一档常驻红（动作标签轴），不等选中或悬停才显出危险度。
    expect(html).toContain('h-opt blk');
  });

  it('「合并进已有规则」视图：候选、置灰三档各自的出路、影子提示、清空键', () => {
    const rows: AppendTargetVM[] = [
      appendRow(),
      appendRow({ ruleId: 'r2', remarks: '已包含', block: 'contains' }),
      appendRow({ ruleId: 'r3', remarks: 'AND 规则', block: 'andMode' }),
      appendRow({ ruleId: 'r4', remarks: '值不适用', block: 'valueUnfit' }),
      appendRow({ ruleId: 'r5', remarks: '已禁用', enabled: false }),
      { ...appendRow({ ruleId: 'r6', remarks: '被前面挡住' }), shadowed: true },
    ];
    const html = render({
      ruleSubject: SUBJECT_VM,
      ruleSubjectView: 'pick',
      appendTargets: rows,
      appendQuery: 'net',
    });
    // 三档置灰**各自带出路**，不是一句笼统的「不可追加」。
    expect(html).toContain('rules.pickWhyAnd');
    expect(html).toContain('rules.pickWhyUnfit');
    expect(html).toContain('rules.subjectAlreadyInRule');
    expect(html).toContain('rules.pickDisabledTag');
    expect(html, '「前面可能先命中」没提示').toContain('rules.pickShadowTag');
    // 影子那一行**不禁用** —— 用户完全可以就是想把值加进后面那条。
    const shadow = html.slice(html.lastIndexOf('<button', html.indexOf('被前面挡住')), html.indexOf('被前面挡住'));
    expect(shadow, '影子行被禁用了 —— 那份判断只是启发式，权威匹配在内核').not.toContain('disabled');
    // 清空键：有搜索词才出现。
    // ⚠️ 它**不抵** `ConnectionTopology.tsx|k:home.clear` 那笔账（那一条如实留在债务侧）——
    // 桌面那颗清的是拓扑图主机筛选框，移动端没有那个对象；这一颗服务的是规则选择器，
    // 只是恰好同名。这条断言守的是这一颗本身在不在，不证明另一颗接上了。
    expect(html).toContain("aria-label=\"home.clear\"");
    expect(render({ ruleSubject: SUBJECT_VM, ruleSubjectView: 'pick', appendTargets: rows })).not.toContain(
      "aria-label=\"home.clear\"",
    );
    // 空态两句分得开：没搜过 vs 搜了没命中。
    expect(render({ ruleSubject: SUBJECT_VM, ruleSubjectView: 'pick' })).toContain('rules.pickEmpty');
    expect(
      render({ ruleSubject: SUBJECT_VM, ruleSubjectView: 'pick', appendQuery: 'zzz' }),
    ).toContain('rules.pickNoMatch');
  });

  it('接线层：两条规则写腿都过暂存闸门，且判据与桌面同一条（不在移动端另开一个 if）', () => {
    for (const leg of ['addSubjectRule', 'onAppendToRule']) {
      const at = WIRING_RAW.indexOf(`const ${leg}`);
      expect(at, `${leg} 这条腿不见了`).toBeGreaterThan(-1);
      const body = WIRING_RAW.slice(at, at + 1800);
      expect(body, `${leg} 没过暂存闸门 —— 同一次编辑在两个客户端上会落成两种形状`).toContain(
        "editRoute('trafficRules', stagingEnabled)",
      );
      expect(body, `${leg} 没有暂存那一半的 stage() 调用`).toContain('stage({');
    }
    // 追加腿的失败必须如实报，不许把「目标漂移」静默当成功。
    const append = WIRING_RAW.slice(WIRING_RAW.indexOf('const onAppendToRule'));
    expect(append.slice(0, 1800)).toContain("throw new Error(t('rules.appendFail'))");
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑭ 批 18 接上的**主机筛选**（桌面 `ConnectionTopology.tsx|k:home.clear` 那笔账）。
 *
 * 为什么必须有这一组：与 ⑬ 同一条理由 —— 把登记表里一条 `absent` 改成 `ported` 再配一个能对上
 * 的锚，`wiring-completeness.test.ts` 就会变绿，**而锚只证明那段文本在**。这一组断言的是这条
 * 能力今天真的渲染得出、三档分支各自成立、且与同屏那颗同名的清空键分得开。
 *
 * 🔴 **射程自曝（不许把这一组读成行为验证）**：本仓 vitest 跑 `environment:'node'`、刻意不装
 * jsdom ⇒ `useEffect` 不执行、事件也派发不了。故：
 *
 *  · **能真渲染、这一组管**：三档渲染分支（无筛选词 / 有筛选词 / 筛了没命中）、两颗清空键
 *    同屏并存且各自认得出、筛选框恒在。这些是 `renderToStaticMarkup` 的真 DOM 上的断言。
 *  · **只有源码级取证**：「敲字 → `onHostQuery` → 重算 `hosts` → 重渲染」这条交互链路。
 *    这一组只对拍接线层源码里那条 `hostTopOf(filterByHostQuery(…))`（⑥ 组末条）与呈现层
 *    `onChange` 打在哪个回调上，**不等于**验过了那次交互。
 *  · **归真机验收**：筛选框的输入法 / 软键盘 / `type="search"` 的原生清除叉与我们这颗的共存。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑭ 主机 Top 卡的筛选 + 清空键：能力真的在，且与同屏那颗同名清空键分得开', () => {
  /** 主机 Top 那张卡的 HTML 切片（`data-home-card="host-traffic-top"` 到下一张卡之间）。 */
  const hostCard = (html: string): string => {
    const at = html.indexOf('data-home-card="host-traffic-top"');
    expect(at, '主机 Top 卡没渲染出来 —— 下面几条会变成在空串上断言').toBeGreaterThan(-1);
    const next = html.indexOf('data-home-card=', at + 1);
    return html.slice(at, next < 0 ? html.length : next);
  };

  it('筛选框落在**主机 Top 卡内部**（八张卡由 IA §2.1 定死，不许为它新开一张）', () => {
    const card = hostCard(render());
    expect(card, '筛选框不在这张卡里').toContain('data-host-filter="query"');
    expect(card).toContain('home.searchTopology');
    // 卡的集合没变：⑭ 不许把 `HOME_CARD_ORDER` 撑大。那条等式由 ① 组守，这里只做一次点名对照。
    expect(HOME_CARD_ORDER).toHaveLength(7);
  });

  it('清空键只在有筛选词时出现（无筛选词时它是常驻噪音）', () => {
    expect(hostCard(render({ hostQuery: 'cdn' })), '有筛选词却没有清空键').toContain(
      'data-host-filter="clear"',
    );
    expect(hostCard(render({ hostQuery: '' })), '没筛选词也画了清空键').not.toContain(
      'data-host-filter="clear"',
    );
  });

  it('🔴 筛选框**恒在**，即便一条都没命中 —— 否则用户再也够不着那颗清空键', () => {
    const card = hostCard(render({ hostQuery: 'zzz', hosts: [] }));
    expect(card, '没命中就把筛选框一起收掉了 —— 用户被困在一个筛空的卡里').toContain(
      'data-host-filter="query"',
    );
    expect(card, '没命中却没有清空键 —— 唯一的出路没了').toContain('data-host-filter="clear"');
  });

  it('空态两句分得开：窗口里没有连接 vs 筛了没命中（同一句会被读成「代理断了」）', () => {
    const noConn = hostCard(render({ hosts: [], hostQuery: '' }));
    expect(noConn).toContain('connections.noActive');
    expect(noConn).not.toContain('home.searchTopologyNoMatch');

    const noMatch = hostCard(render({ hosts: [], hostQuery: 'zzz' }));
    expect(noMatch).toContain('home.searchTopologyNoMatch');
    expect(noMatch, '筛空了却说「没有活动连接」').not.toContain('connections.noActive');

    // 反向对照：有命中时两句一句都不出现（否则上面两条可能是「空态恒渲染」的假绿）。
    const hit = hostCard(render({ hostQuery: 'example' }));
    expect(hit).not.toContain('connections.noActive');
    expect(hit).not.toContain('home.searchTopologyNoMatch');
  });

  /*
   * 🔴 **两颗 `home.clear` 的分身判据。**
   *
   * 同屏有两颗清空键，两颗都用 `aria-label={t('home.clear')}`：一颗是本批接的主机筛选
   * （桌面拓扑图卡头那个搜索框的落点，就是登记表 `ConnectionTopology.tsx|k:home.clear` 那笔账），
   * 另一颗是「合并进已有规则」选择器的搜索框（⑬ 组守着它，它**不抵**这笔账）。
   *
   * 登记表按 id 认人，锚指的是 `data-host-filter="clear"` 而**不是** `aria-label` —— 后者两颗
   * 都命中。这一条正面证明「分得开」：两颗同屏并存、数得出、且各自有一条只认得自己的判据。
   */
  it('🔴 两颗同名清空键同屏并存，且各自认得出（登记的锚指的是主机筛选那一颗）', () => {
    const html = render({ hostQuery: 'cdn', ruleSubject: SUBJECT_VM, ruleSubjectView: 'pick', appendQuery: 'net' });
    /* 逐颗取出带 `home.clear` 的那个 `<button>` 开标签。按颗取而不是整片 slice：规则选择器那张
       弹层**渲染在主机 Top 卡内部**（`RuleSubjectPanel` 就挂在那张 Card 里），按 DOM 区间切
       分不开两颗 —— 只有逐颗看它自己的属性才分得开。 */
    const clearButtons = [...html.matchAll(/aria-label="home\.clear"/g)].map((m) =>
      html.slice(html.lastIndexOf('<button', m.index), m.index),
    );
    // 两颗都在：`aria-label` 这一面上它们无从区分，故这里数的是**颗数**。
    expect(
      clearButtons.length,
      '同屏应当恰有两颗 `home.clear`：主机筛选一颗、规则选择器一颗',
    ).toBe(2);
    // 恰好一颗带主机筛选的标记 —— 多一颗或少一颗，登记的锚都不再唯一指人。
    const tagged = clearButtons.filter((b) => b.includes('data-host-filter="clear"'));
    expect(
      tagged.length,
      '带 `data-host-filter="clear"` 的不是恰好一颗 ⇒ 锚要么同时命中两颗（账变僵尸）、要么一颗都不命中',
    ).toBe(1);
    // 另一颗**不带**任何 `data-host-filter` —— 这就是锚分得开的全部理由。
    expect(
      clearButtons.filter((b) => !b.includes('data-host-filter="clear"'))[0],
      '规则选择器那颗也带上了主机筛选的标记 ⇒ 锚会同时命中两颗',
    ).not.toContain('data-host-filter');
    expect(hostCard(html)).toContain('data-host-filter="clear"');
    // 反向对照：把规则选择器关掉，`home.clear` 只剩主机筛选那一颗 —— 证明上面数到的 2 不是同一颗
    // 被重复计数，也证明「删掉规则选择器的搜索」不会把这笔账一起带走。
    const onlyHost = render({ hostQuery: 'cdn' });
    expect((onlyHost.match(/aria-label="home\.clear"/g) ?? []).length).toBe(1);
    expect(onlyHost).toContain('data-host-filter="clear"');
  });

  it('清空键与输入框打在同一个回调上（点一下等于把筛选词清成空串）', () => {
    const view = read(join(HERE, 'HomeScreenView.tsx'));
    const at = view.indexOf('data-host-filter="clear"');
    expect(at, '主机筛选的清空键不在源码里').toBeGreaterThan(-1);
    /* 切到 `</button>`，不是切到第一个 `>` —— `onClick={() => …}` 里那个箭头本身带一个 `>`，
       按第一个 `>` 收口会把回调整条切掉，于是这条断言变成在读一段不含回调的文本。 */
    const btn = view.slice(view.lastIndexOf('<button', at), view.indexOf('</button>', at));
    expect(btn, '清空键没有把筛选词清空的腿').toContain("props.onHostQuery('')");
    // 输入框那一侧：受控值与回调都接在同一格上（少任一半，框要么不动要么清不掉）。
    const input = view.slice(view.lastIndexOf('<input', at), at);
    expect(input).toContain('value={props.hostQuery}');
    expect(input).toContain('props.onHostQuery(e.currentTarget.value)');
  });
});


describe('首页路由和检测分行', () => {
  it('三种模式直接可见，且只有当前模式被选中', () => {
    for (const mode of ['smart', 'global', 'direct'] as const) {
      const html = cardSegments(render({ routing: mode })).get('mode-chips')!;
      expect((html.match(/data-write-control="routing"/g) ?? []).length).toBe(3);
      expect((html.match(/aria-pressed="true"/g) ?? []).length).toBe(1);
      expect(html).toContain(`aria-pressed="true">home.routing${mode[0].toUpperCase() + mode.slice(1)}`);
    }
  });
  it('出口 IP 与检测按钮位于检测卡，IPv6 保持完整', () => {
    const html = cardSegments(render({exitIp: '2001:db8:1234:5678:90ab:cdef:1234:5678'})).get('unlock-detection')!;
    expect(html).toContain('data-write-control="network-check"');
    expect(html).toContain('2001:db8:1234:5678:90ab:cdef:1234:5678');
    expect(html).toContain('mobileHome.checkNow');
  });
});
