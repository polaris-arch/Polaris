/**
 * 移动端「规则」屏的门。
 *
 * 落地设计：`~/docs/polaris/design/polaris-mobile-screen-rules-2026-09-04.md`。
 * 相邻的门（射程不重叠，别在这里重复）：
 *  · `../../mobile-entry.test.ts` —— 入口链路（文档 / 装载面 / 契约 A1 的 CSS 集合 / 五个目的地）；
 *  · `../../../styles/css-feature-floor.test.ts` —— 本文件配套 CSS 只用 Chromium ≤105 的特性；
 *  · `../../../i18n/i18n-coverage.test.ts` —— 裸 CJK、键的双向对账。
 *
 * 本门守三条**这一屏专有、且丢了不会有任何别的门说话**的不变式：
 *
 *  ① **有序规则列表在任何断点下都不进双列。** 行号就是优先级；双列会把第 5 条摆在第 1 条右边，
 *     阅读序不再等于求值序。判据落在**两个面**上（DOM 祖先链 + CSS 选择器面），因为这条规矩
 *     有两条独立的破坏路径：把列表塞进多列容器（JSX），或给列表本身加一条多列规则（CSS）。
 *     只守一面就留了另一条路。
 *
 *  ② **不得出现「哪些应用走隧道」的入口或暗示它以后会有的文案**（IA 裁定 #7）。
 *     判据不是排期而是**兑现不了**。
 *     🔴 **理由 2026-09-06 重写（原理由已被同仓拆掉）**：原文写的是「配置里仍有多个零认证回环
 *     入站、其中数个指向代理 ⇒ 被排除出隧道的应用照样能直连绕过分流」，而
 *     `crates/config-engine/src/builder/inbounds.rs:104` 今天写死
 *     `platform != "android" && platform != "ios"` 才发 mixed inbound（`:76-88` 逐字解释「洞的
 *     载体不存在」）—— 那个洞没了，裁定却还挂着它当理由。今天成立的理由是
 *     `ui/src/mobile/destinations.ts` 头注那条：**这个入口承诺一张双向名单，而实现只有减法
 *     那一半**（`inbounds.rs:717 android_exclude_packages` 发 `tun.exclude_package`；同函数
 *     映射表 `:692` 写死 `include_package` **恒不发射**），且只在 Android 上、只覆盖带包名的
 *     内置预设。除了字面量扫描，本门还从**行为面**断言：应用行的策略控件是只读的。
 *
 *  ③ **§4.12：每一处只靠 `data-tip` 承载的解释都必须换常驻通道。** 触屏没有 hover。
 *     判据是「一个都不许剩」+「每一条都落到了渲染出来的文字里」——只查前者会被
 *     「把 tip 连同解释一起删掉」骗过。
 *
 * ── 判据的自污染，本仓踩过 ──────────────────────────────────────────────────
 * 本文件里写着 ② 那些被禁字面量。若取材面把本文件也扫进去，门会对着自己的禁令列表报红；
 * 而「为了让它变绿」最自然的动作就是把取材面缩窄，最后缩成什么都不扫、恒绿。
 * 故取材面**显式排除测试文件**，并由 ⓪ 组正面断言「排除生效 + 取材面仍然是活的」。
 */

import { afterAll, describe, expect, it, vi } from 'vitest';
import { IS_TEST_ONLY_MODULE } from '@/contracts/test-only-modules';
import { ALL, UNCONDITIONAL, winners, winnersText } from '@/styles/css-cascade.test-support';
import { closeOracle, measure } from '@/styles/css-oracle.test-support';
import { inMobileShell } from '@/styles/mount.test-support';
import { renderToStaticMarkup } from 'react-dom/server';

/**
 * ⑤ 组要渲染**容器**（它是唯一碰 i18next 的文件）。恒等 `t` 让断言比的是 key 而不是译文；
 * `initReactI18next` 也要给，否则 `i18n/index.ts` 的异步初始化会在这份 mock 上抛。
 * ①–④ 组的分段是纯呈现、`t` 由 props 注入，不受这条 mock 影响。
 */
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key, i18n: { language: 'zh-CN' } }),
  initReactI18next: { type: '3rdParty', init: () => {} },
}));
import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import type { Rule } from '@/contracts/types';
import { TrafficSegment, type RuleRowBinding } from './TrafficSegment';
import { DnsResourcePage, DnsSegment } from './DnsSegment';
import { AppsSegment, type AppRowModel } from './AppsSegment';
import { dropIndexAt } from './RuleRow';
import { fmtBytes } from '@/components/screens/shared/format';
import { ResourcesSegment } from './ResourcesSegment';
/*
 * 容器**静态**引入（⑤ 组用）。刻意不写成 `it()` 里的 `await import()`：那会把整张模块图
 * （104 个文件）的导入成本算进那条测试的 5s 超时里 —— 单跑 1s、全量并行跑 5.4s 就红，
 * 而且红的原因与被测不变式毫无关系。判据不该随机器负载抖动。
 * `vi.mock` 由 vitest 提升到所有 import 之上，静态引入照样吃得到那份 mock。
 */
import { MobileRulesScreen } from './RulesScreen';

const HERE = fileURLToPath(new URL('.', import.meta.url));
const read = (name: string): string => readFileSync(join(HERE, name), 'utf8');

/* ═══════════ 取材面 ═══════════ */

/**
 * 剥注释（含字符串识别）。与 `mobile-entry.test.ts` 同一口径：本目录几乎每个文件的头注里都
 * 引用着反面例子（`data-tip`、被禁的字面量），不剥就会把文档说明当成真实代码报红。
 * 必须识别字符串 —— 正则版会被含 `*​/` 的字符串吃掉整份文件（那才是真正危险的失效形态：
 * 取材面被自己吃光后一路绿灯）。
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

/** 本屏的**生产**源码文件名（测试文件显式排除，见文件头注「判据的自污染」）。 */
const SOURCE_FILES = readdirSync(HERE)
  .filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f))
  .sort();

/** 剥注释后的生产源码面。 */
const SOURCE = SOURCE_FILES.map((f) => strip(read(f))).join('\n');
/** 未剥注释的原文（只用于「这个字面量确实写在文件里」这类自检）。 */
const SOURCE_RAW = SOURCE_FILES.map((f) => read(f)).join('\n');
const CSS = read('rules-screen.css');

/** 本屏新增的文案面：`mobileRules` 命名空间的五语取值。 */
const COPY = ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa']
  .map((lang) => {
    const json = JSON.parse(
      readFileSync(join(HERE, '../../../i18n/locales', `${lang}.json`), 'utf8'),
    ) as Record<string, unknown>;
    return JSON.stringify(json['mobileRules']);
  })
  .join('\n');

/* ═══════════ 夹具与渲染 ═══════════ */

/** i18n 恒等：门比的是「哪个 key 落到了哪里」，不比译文。 */
const t = (key: string): string => key;

/** 注入式的写失败查询：`ERR_KEYS` 里的键返回一条可辨识的红字，其余返回 undefined。 */
const ERR_TEXT = 'INJECTED-WRITE-FAILURE';
const errorOfWith =
  (...keys: string[]) =>
  (key: string): string | undefined =>
    keys.includes(key) ? ERR_TEXT : undefined;
const noErrors = (): undefined => undefined;

function ruleOf(id: string, over: Partial<Rule> = {}): Rule {
  return {
    id,
    type: 'domainSuffix',
    values: ['example.com'],
    action: 'proxy',
    enabled: true,
    remarks: `rule-${id}`,
    ...over,
  };
}

function bindingOf(rule: Rule, over: Partial<RuleRowBinding> = {}): RuleRowBinding {
  return {
    rule,
    enabled: true,
    deleteConfirming: false,
    sheetOpen: false,
    onOpenSheet: () => {},
    onToggle: () => {},
    onMove: () => {},
    onReorder: () => {},
    onDuplicate: () => {},
    onDelete: () => {},
    onEdit: undefined,
    editUnavailableReason: 'mobileRules.formUnavailable',
    ...over,
  };
}

/** 三行规则，且把四个角标 + 「路由效果未生效」一次全点亮 —— §4.12 的枚举要它们都在场。 */
const RULE_ROWS: RuleRowBinding[] = [
  bindingOf(ruleOf('a'), {
    routeInactive: true,
    stagedOnly: true,
    hasMeshOverlap: true,
    sheetOpen: true,
  }),
  bindingOf(ruleOf('b'), { hasMissingResource: true, targetNodeName: 'HK-01' }),
  bindingOf(ruleOf('c'), { targetMissing: true }),
];

const trafficMarkup = (
  rows: readonly RuleRowBinding[] = RULE_ROWS,
  errorOf: (key: string) => string | undefined = noErrors,
): string =>
  renderToStaticMarkup(
    <TrafficSegment
      t={t}
      modeInactive
      regionRouting={{ enabled: true, region: 'cn', reverse: false }}
      onRegionChange={() => {}}
      isSmartMode={false}
      onBackToSmart={() => {}}
      rows={rows}
      errorOf={errorOf}
    />,
  );

/**
 * 四档**各占一行**：策略选择器的调色、当前值、以及「阻断也点得开」这三件事都要有对象可判。
 * `nodeName` 那一行的 `policyText` 刻意是**运行期数据**（节点名），不是 i18n key ——
 * 触发器必须显示它而不是选项表里那句「指定节点」（`valueLabel` 这一格的存在理由）。
 */
const APP_ROWS: AppRowModel[] = [
  {
    id: 'netflix',
    name: 'Netflix',
    emoji: '🎬',
    processes: 'netflix.exe',
    policyText: 'appPolicy.followGlobal',
    policyTone: 'follow',
    policyValue: 'follow',
    isCustom: false,
    category: 'video',
  },
  {
    id: 'telegram',
    name: 'Telegram',
    emoji: '✈️',
    policyText: 'appPolicy.action.direct',
    policyTone: 'direct',
    policyValue: 'direct',
    isCustom: false,
    category: 'social',
  },
  {
    id: 'youtube',
    name: 'YouTube',
    emoji: '📺',
    policyText: 'HK-01',
    policyTone: 'proxy',
    /* 「指定节点」那一行当前选中的就是 `srv-hk-01` ⇒ 选择器要在节点组里勾中它，
       且默认展开含它的那一组。 */
    policyValue: 'node:srv-hk-01',
    isCustom: false,
    category: 'video',
  },
  {
    id: 'tiktok',
    name: 'TikTok',
    emoji: '🎵',
    policyText: 'appPolicy.action.block',
    policyTone: 'block',
    policyValue: 'block',
    /* 唯一一条自定义应用：删除键只该在这一行出现（内置预设删不掉）。 */
    isCustom: true,
    category: 'video',
  },
];

/** 资源行的「更新于」时基。固定值 ⇒ 判据不随跑测时刻抖动。 */
const RES_UPDATED_ISO = '2020-01-01T00:00:00.000Z';

/**
 * 策略选择器的候选：一组不可折叠的策略 + 两组可折叠的节点（与桌面 `PolicySelector` 同形）。
 * 两组节点是必需的：只有一组时「组头点得开」与「只有一个组头」分不清。
 */
const APP_POLICY_GROUPS = [
  {
    label: 'appPolicy.policy',
    options: [
      { value: 'follow', label: 'appPolicy.followGlobal' },
      { value: 'direct', label: 'appPolicy.action.direct' },
      { value: 'block', label: 'appPolicy.action.block', danger: true },
    ],
  },
  {
    id: 'sub-a',
    label: 'Sub A',
    options: [{ value: 'node:srv-hk-01', label: 'HK-01' }],
  },
  {
    id: 'sub-b',
    label: 'Sub B',
    options: [{ value: 'node:srv-jp-02', label: 'JP-02' }],
  },
];

const APP_CATEGORIES = [
  { key: 'all', label: 'appPolicy.cat.all' },
  { key: 'video', label: 'appPolicy.cat.video' },
  { key: 'game', label: 'appPolicy.cat.game' },
];

const appsMarkup = (
  masterError?: string,
  {
    masterEnabled = false,
    isSmartMode = false,
    categorySheetOpen = false,
    policySheetAppId = null,
    rows = APP_ROWS,
    errorOf = noErrors,
    view = 'list',
    removeConfirmingId = null,
    policyOpenGroups = new Set<string>(),
  }: {
    masterEnabled?: boolean;
    isSmartMode?: boolean;
    categorySheetOpen?: boolean;
    policySheetAppId?: string | null;
    rows?: readonly AppRowModel[];
    errorOf?: (key: string) => string | undefined;
    view?: 'cards' | 'list';
    removeConfirmingId?: string | null;
    policyOpenGroups?: ReadonlySet<string>;
  } = {},
): string =>
  renderToStaticMarkup(
    <AppsSegment
      t={t}
      summary={{ total: 4, follow: 1, node: 1, direct: 1, block: 1 }}
      presetsPending={false}
      presetsFailed
      masterEnabled={masterEnabled}
      isSmartMode={isSmartMode}
      onBackToSmart={() => {}}
      categories={APP_CATEGORIES}
      category="all"
      onCategoryChange={() => {}}
      search=""
      onSearchChange={() => {}}
      groups={[{ key: 'video', label: 'appPolicy.cat.video', rows }]}
      platformNote="mobileRules.appRoutingMatchNote"
      view={view}
      onViewChange={() => {}}
      masterError={masterError}
      categorySheetOpen={categorySheetOpen}
      onCategorySheetOpen={() => {}}
      onCategorySheetClose={() => {}}
      policySheetAppId={policySheetAppId}
      onPolicySheetOpen={() => {}}
      onPolicySheetClose={() => {}}
      policyGroups={APP_POLICY_GROUPS}
      policyOpenGroups={policyOpenGroups}
      onPolicyPick={() => {}}
      onAddCustom={() => {}}
      onRemoveCustom={() => {}}
      removeConfirmingId={removeConfirmingId}
      errorOf={errorOf}
    />,
  );

const resourcesMarkup = (
  sheetId: string | null,
  {
    errorOf = noErrors,
    deleteConfirmingId = null,
    resetConfirming = false,
  }: {
    errorOf?: (key: string) => string | undefined;
    deleteConfirmingId?: string | null;
    resetConfirming?: boolean;
  } = {},
): string =>
  renderToStaticMarkup(
    <ResourcesSegment
      t={t}
      source="all"
      onSourceChange={() => {}}
      loading={false}
      error={false}
      groups={[
        {
          key: 'Geosite',
          label: 'Geosite',
          /*
           * `size` / `updatedAt` 两格（`data-contract.json#sources.rule-resources`）。
           * 四行刻意覆盖四种态：**正常**（r1，时间戳该画）、**失败**（r2）、
           * **缺时间戳**（r3，解析不出 ⇒ 退回破折号而不是「刚刚」）、**下载中**（r4）。
           * 后三种都不许把 `updatedAt` 印出来 —— 见 `ResourcesSegment` 头注第 3 条。
           */
          rows: [
            {
              id: 'r1',
              name: 'geosite-cn',
              builtin: true,
              references: 0,
              size: 1536,
              updatedAt: RES_UPDATED_ISO,
            },
            {
              id: 'r2',
              name: 'my-set',
              builtin: false,
              references: 2,
              failed: true,
              size: 2048,
              updatedAt: RES_UPDATED_ISO,
            },
            { id: 'r3', name: 'idle-set', builtin: false, references: 0, size: 0 },
            {
              id: 'r4',
              name: 'busy-set',
              builtin: false,
              references: 0,
              progress: 0.4,
              size: 4096,
              updatedAt: RES_UPDATED_ISO,
            },
          ],
        },
      ]}
      onUpdateAll={() => {}}
      updatingAll={false}
      updatingIds={new Set()}
      onResetBuiltin={() => {}}
      resetConfirming={resetConfirming}
      errorOf={errorOf}
      onUpdateOne={() => {}}
      onCancel={() => {}}
      onDelete={() => {}}
      deleteConfirmingId={deleteConfirmingId}
      sheetId={sheetId}
      onOpenSheet={() => {}}
      onCatalog={() => {}}
      onUrlDownload={() => {}}
    />,
  );

const dnsMarkup = (errorOf: (key: string) => string | undefined = noErrors): string =>
  renderToStaticMarkup(
    <DnsSegment
      t={t}
      rows={RULE_ROWS}
      defaultActionText="rules.dnsResolverInherit"
      systemPaneAbsentReason="mobileRules.dnsSystemPaneAbsent"
      errorOf={errorOf}
    />,
  );

const dnsResourceMarkup = (
  errorOf: (key: string) => string | undefined = noErrors,
  deleteConfirmingId: string | null = null,
): string =>
  renderToStaticMarkup(
    <DnsResourcePage
      t={t}
      rows={[
        {
          id: 'builtin-domestic',
          name: 'domestic',
          description: 'https',
          enabled: true,
          builtin: true,
          /* 内置那条**被引用着**：删除护栏的两半（不画删除键 / 引用数在场）要同时有对象可判。 */
          references: 2,
        },
        { id: 'x', name: 'custom', description: 'https', enabled: false, references: 0 },
      ]}
      emptyText="rules.dnsWorkspace.groupsEmpty"
      onToggle={() => {}}
      onEdit={() => {}}
      onDelete={() => {}}
      deleteConfirmingId={deleteConfirmingId}
      errorOf={errorOf}
    />,
  );

/* ═══════════ 静态标记的祖先链走查 ═══════════ */

const VOID_TAGS = new Set(['input', 'br', 'img', 'hr', 'meta', 'link', 'source', 'track']);

/**
 * 找出每一个「起始标签含 `needle`」的元素，返回它的**祖先起始标签**列表（由外到内）。
 *
 * 直接对 HTML 文本 `indexOf` 是不行的：那只能说「这两个字符串都在」，说不出谁包着谁 ——
 * 而本门 ① 要判的恰恰是包含关系。
 */
function ancestorChains(html: string, needle: string): string[][] {
  const chains: string[][] = [];
  const stack: string[] = [];
  const tag = /<(\/?)([a-zA-Z][\w-]*)((?:"[^"]*"|'[^']*'|[^>])*?)(\/?)>/g;
  let m: RegExpExecArray | null;
  while ((m = tag.exec(html)) !== null) {
    const [, closing, name, attrs, selfClose] = m;
    if (closing === '/') {
      stack.pop();
      continue;
    }
    const openTag = `<${name}${attrs}>`;
    if (attrs.includes(needle)) chains.push([...stack]);
    if (selfClose !== '/' && !VOID_TAGS.has(name.toLowerCase())) stack.push(openTag);
  }
  return chains;
}

/** 只留文字节点：剥掉全部标签。用于「这句话用户看得见吗」这类判据。 */
const textNodesOf = (html: string): string => html.replace(/<[^>]*>/g, '\n');

/* ═══════════ CSS 的多列面 ═══════════ */

/**
 * 从 CSS 里穷举出**会产生一列以上**的规则块，返回它们的选择器。
 *
 * 覆盖三种写法：`column-count` / `columns` 简写（本仓 CSS 下限白名单里没有它们，
 * 但判据不能假设别人不会加）、以及 `grid-template-columns` 里出现两条以上轨道
 * （`repeat(N,…)` 的 N ≥ 2，或空格分隔的多条轨道）。
 */
function multiColumnSelectors(css: string): string[] {
  const body = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const out: string[] = [];
  const block = /([^{}]+)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = block.exec(body)) !== null) {
    const selector = m[1].trim().replace(/\s+/g, ' ');
    const decls = m[2];
    // `@container` / `@media` 的 prelude 自己也会被上面的正则当成一个「块」；它没有声明，跳过。
    if (selector.startsWith('@')) continue;
    let multi = false;
    const cc = /\bcolumn-count\s*:\s*([0-9]+)/.exec(decls);
    if (cc && Number(cc[1]) > 1) multi = true;
    // `\bcolumns` 会命中 `grid-template-columns`（`-` 是词边界）——那条另有分支处理，
    // 在这里重复命中会让下面的轨道数判断变成死代码。用「前面不是 `-` 或字母」把它挡开。
    if (/(?<![-\w])columns\s*:/.test(decls)) multi = true;
    // 这三种同样能造出一列以上，且都不写 `grid-template-columns`：
    if (/\bgrid-auto-flow\s*:[^;]*\bcolumn\b/.test(decls)) multi = true;
    if (/(?<![-\w])grid\s*:/.test(decls)) multi = true;
    if (/(?<![-\w])grid-template\s*:/.test(decls)) multi = true;
    const gtc = /\bgrid-template-columns\s*:\s*([^;]+)/.exec(decls);
    if (gtc) {
      const value = gtc[1].trim();
      const rep = /^repeat\(\s*([0-9]+)/.exec(value);
      if (rep) {
        if (Number(rep[1]) > 1) multi = true;
      } else if (value.replace(/\([^)]*\)/g, '').trim().split(/\s+/).length > 1) {
        multi = true;
      }
    }
    if (multi) out.push(selector);
  }
  return [...new Set(out)].sort();
}

// ─────────────────────────────────────────────────────────────────────────────

describe('⓪ 自检：取材面是活的，且没有把自己扫进去', () => {
  it('生产源码面非空、有量级，且**不含**本测试文件', () => {
    expect(SOURCE_FILES.length, `本屏源码只扫到 ${SOURCE_FILES.length} 个文件`).toBeGreaterThan(5);
    expect(SOURCE_FILES).not.toContain('rules-screen.test.tsx');
    expect(SOURCE_FILES).toContain('RulesScreen.tsx');
    // 正面：面里确实有本屏的东西（而不是恰好读到了一堆空文件）。
    expect(SOURCE).toContain('mr-row');
    expect(SOURCE.length).toBeGreaterThan(20_000);
  });

  it('剥注释真的把头注里的反面例子挡在外面（否则下面每条禁令都会对着自己的文档报红）', () => {
    // 这些字符串**确实**写在本屏源码的注释里；剥完之后不该再出现。
    expect(SOURCE_RAW).toContain('data-tip');
    expect(SOURCE_RAW).toContain('The production RulesScreen owns state');
    expect(strip(SOURCE_RAW)).not.toContain('The production RulesScreen owns state');
    // 剥注释器本身：字符串里的 `/*` 不是注释起点。
    expect(strip("const g = ['**/x/**'];\nlet a = 1;")).toContain('**/x/**');
    /*
     * 剥注释器把 `'` 当字符串起点，而 JSX 的**文字节点**里也可能出现单引号（`don't`）——
     * 那会让它从这个撇号一路吞到下一个引号，把中间的代码从取材面里抹掉，
     * 于是 `data-tip` 禁令与 ② 的字面量扫描一起静默失效。用「剥前剥后可执行结构不变」钉住：
     * 今天七个文件都没有裸撇号，将来加一个就红。
     */
    const before = (SOURCE_RAW.match(/\bexport (?:function|const|interface|type)\b/g) ?? []).length;
    const after = (SOURCE.match(/\bexport (?:function|const|interface|type)\b/g) ?? []).length;
    expect(after, '剥注释吞掉了代码 —— 多半是 JSX 文字里出现了裸单引号').toBe(before);
    expect(before).toBeGreaterThan(20);
  });

  it('渲染面非空、有量级（渲染塌了会退化成「什么都没渲染所以全绿」）', () => {
    for (const [name, html] of [
      ['traffic', trafficMarkup()],
      ['dns', dnsMarkup()],
      ['apps', appsMarkup()],
      ['resources', resourcesMarkup('r2')],
      ['dnsResource', dnsResourceMarkup()],
    ] as const) {
      expect(html.length, `${name} 段渲染出来是空的`).toBeGreaterThan(500);
    }
  });

  it('CSS 面非空，且多列机制确实存在（否则 ① 的「唯一入口」判据恒真）', () => {
    expect(CSS.length).toBeGreaterThan(2_000);
    expect(
      multiColumnSelectors(CSS).length,
      'CSS 里一条多列规则都没有 —— 判据面塌了',
    ).toBeGreaterThan(0);
  });
});

describe('① 有序规则列表在任何断点下都不进双列', () => {
  it('DOM：有序列表的祖先链里没有多列容器，它自己也不是', () => {
    for (const [name, html] of [
      ['流量', trafficMarkup()],
      ['DNS', dnsMarkup()],
    ] as const) {
      const chains = ancestorChains(html, 'data-ordered="1"');
      expect(chains.length, `${name} 段里没找到有序列表 —— 判据面塌了`).toBe(1);
      for (const ancestor of chains[0]) {
        expect(
          ancestor,
          `${name} 段的有序规则列表被放进了多列容器 —— 行号是优先级，双列会让第 5 条落在第 1 条` +
            '右边，阅读序不再等于求值序',
        ).not.toContain('data-flow="multi"');
      }
      // 它自己也不许**是**多列容器（把 `data-flow` 直接加到列表上是同一条路的另一半）。
      expect(html).not.toMatch(/data-ordered="1"[^>]*data-flow/);
      expect(html).not.toMatch(/data-flow[^>]*data-ordered="1"/);
    }
  });

  it('正向对照：无序集合**确实**在走多列（否则上一条会被「整屏都没有多列」骗过）', () => {
    // 应用卡与 DNS 资源仍是无序多列；资源下载列表改为连续单列以便扫描状态。
    for (const [name, html] of [
      ['应用', appsMarkup()],
      ['DNS 资源', dnsResourceMarkup()],
    ] as const) {
      expect(html, `${name} 段没有任何多列容器 —— 双列流被整个删掉了？`).toContain(
        'data-flow="multi"',
      );
    }
  });

  it('CSS：多列入口只有登记过的那两个，有序列表的选择器一次都不在多列块里', () => {
    /*
     * 从「唯一入口」放宽成「两个登记过的入口」：IA §2.2 Breakpoints 要的是**两个面**——
     * 卡片块（`.mr-cards`，必做）与无序行集合（`.mr-flow`，可做）。
     *
     * 放宽的**不是**这条判据守的东西。它守的是「有序列表不许多列」，那一条落在下面两处：
     * ① 祖先链判据按 `data-flow="multi"` 写（不是按类名写）⇒ 两个入口都在它的射程里；
     * ② 这里逐个选择器断言不含 `.mr-list`，加上下一条对 `.mr-list` 声明面的正面断言。
     * 名单是白名单：再多一个未登记的多列选择器照样红。
     */
    expect(multiColumnSelectors(CSS)).toEqual(['.mr-cards', '.mr-flow']);
    for (const selector of multiColumnSelectors(CSS)) {
      expect(selector, '有序规则列表被直接写成了多列').not.toContain('.mr-list');
    }
  });

  it('接线：`.mr-cards` 真的在生产渲染路径上，且两个多列入口都挂了 `data-flow="multi"`', () => {
    /*
     * 「机制有门、接线没门」在本仓反复撞过：CSS 里写了 `.mr-cards` 双列，JSX 里没人用它，
     * 上一条照样全绿。故这里断言的是**渲染出来的 DOM**，不是「文件里出现过这个名字」。
     *
     * 取材面自检在前：拿不到标记就说明夹具塌了，不许空跑绿。
     */
    for (const [name, html] of [
      ['流量', trafficMarkup()],
      ['DNS', dnsMarkup()],
    ] as const) {
      expect(html, `${name} 段没有渲染出卡片列流容器 —— 双列的必做面没接上`).toContain(
        'class="mr-cards"',
      );
      const cards = [...html.matchAll(/<div class="mr-cards"[^>]*>/g)];
      expect(cards.length, `${name} 段的 .mr-cards 数量不对`).toBe(1);
      expect(
        cards[0][0],
        `${name} 段的 .mr-cards 没挂 data-flow="multi" —— 有序列表的祖先链判据就漏了这条路`,
      ).toContain('data-flow="multi"');
    }
    // 反面对照：卡片容器与行容器是两个不同的类，没被合并成一个。
    expect(CSS).toContain('.mr-cards');
    expect(CSS).toContain('.mr-flow');
  });

  it('CSS：`.mr-list` 自己的声明面里没有任何能造出第二列的属性', () => {
    /*
     * 上一条查的是「谁被声明成了多列容器」。这一条查的是**有序列表自己的声明面** ——
     * `flex-wrap: wrap`、`display: grid`、`grid-auto-flow` 都能在不写
     * `grid-template-columns` 的情况下把一列变两列，而它们不会进上一条的集合。
     * （`flex-wrap` 不进上一条：本屏的 pill 行、地区按钮组都靠它换行，那是换行不是分列；
     * 把它当成全局多列信号会让上一条恒红。它只在这里、只对有序列表成立。）
     */
    const body = CSS.replace(/\/\*[\s\S]*?\*\//g, '');
    const blocks = [...body.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(
      (m) => !m[1].trim().startsWith('@') && m[1].includes('.mr-list'),
    );
    expect(blocks.length, '`.mr-list` 一条规则都没有 —— 判据面塌了').toBeGreaterThan(0);
    for (const [, selector, decls] of blocks) {
      expect(decls, `${selector.trim()} 没有把方向钉成单列`).toMatch(
        /flex-direction\s*:\s*column/,
      );
      for (const banned of [
        /grid-template-columns\s*:/,
        /grid-auto-flow\s*:/,
        /column-count\s*:/,
        /(?<![-\w])columns\s*:/,
        /flex-wrap\s*:/,
        /display\s*:\s*grid/,
      ]) {
        expect(
          decls,
          `${selector.trim()} 出现了能造出第二列的声明 —— 行号是优先级，双列会让阅读序不等于求值序`,
        ).not.toMatch(banned);
      }
    }
  });

  it('CSS：`.mr-list` 在任何断点块里都没有覆写（多一条 @container 规则就是多一条破坏路径）', () => {
    const atBlocks = CSS.replace(/\/\*[\s\S]*?\*\//g, '').match(
      /@(?:container|media|supports)[^{]*\{[\s\S]*?\n\}/g,
    );
    expect(atBlocks, '本文件里一个断点块都没有 —— 判据面塌了').not.toBeNull();
    for (const block of atBlocks ?? []) {
      expect(block, '`.mr-list` 出现在了断点块里：有序列表不许随断点改排布').not.toContain(
        '.mr-list',
      );
    }
  });
});

describe('② IA 裁定 #7：不得出现「哪些应用走隧道」的入口，也不得暗示它以后会有', () => {
  /**
   * 被禁的是**按应用出入隧道**那一套（`VpnService` 的 allow/disallow 列表）。
   * 与本屏「应用」分段的「按应用分流」（哪个应用用哪个出站）是**两个概念** ——
   * 后者在场且只读，是 §4.1 明写的处置，因此不在禁令里。
   */
  const BANNED = [
    '走隧道',
    '入隧道',
    '隧道分应用',
    '分应用隧道',
    'addAllowedApplication',
    'addDisallowedApplication',
    'allowedApplication',
    'disallowedApplication',
    'split tunnel',
    'split-tunnel',
    'splitTunnel',
    'tunnelApps',
    'tunnel-apps',
    'perAppVpn',
    'per-app VPN',
    '即将支持',
    '敬请期待',
    '后续版本',
    'coming soon',
    'Coming soon',
  ];

  it('生产源码面（剥注释后）一个都不含', () => {
    for (const banned of BANNED) {
      expect(SOURCE, `「${banned}」出现在本屏源码里 —— 裁定 #7 定为不做、UI 上不出现`).not.toContain(
        banned,
      );
    }
  });

  it('本屏新增的五语文案面一个都不含（标签自从进了 locale，文案面就有一半在那边）', () => {
    expect(COPY.length, '`mobileRules` 命名空间读不到 —— 判据面塌了').toBeGreaterThan(500);
    for (const banned of BANNED) {
      expect(COPY, `「${banned}」出现在 mobileRules 的译文里`).not.toContain(banned);
    }
  });

  it('行为面：每一行都有一颗**可开**的策略选择器，且逐行附着同一句平台理由（§4.1）', () => {
    const html = appsMarkup();
    // 正面：每一行都真的渲染了一颗 `select-sheet` 触发器（不是只读 span，也不是新造的可点 pill）。
    const triggers = html.match(/class="mr-sel-trigger[^"]*"/g) ?? [];
    expect(
      triggers.length,
      '策略触发器数 ≠ 行数 + 分类筛选那一颗 —— 判据面塌了',
    ).toBe(APP_ROWS.length + 1);
    expect(html.match(/aria-haspopup="listbox"/g)?.length).toBe(APP_ROWS.length + 1);
    // 反面：只读时期那枚 pill 已经没了 —— 留着它就是同一件事有两个载体。
    expect(html, '策略 pill 还在只读态 —— 解禁没落到行上').not.toContain('data-readonly="1"');
    // 每一行都附着**同一句**平台理由（§4.1 明写 same reason attached）。
    const notes = html.match(/mobileRules\.appRoutingMatchNote/g) ?? [];
    expect(notes.length, '每行匹配详情没有附上完整平台理由').toBe(APP_ROWS.length);
    expect(textNodesOf(html)).toContain('mobileHelp.appRoutingMatch');
    expect(html).toContain('class="m-info-trigger"');
  });

  it('🔴 触发器的可访问名是**控件名**，不是它的某一档（读屏那条腿）', () => {
    const html = appsMarkup();
    // 正面：每一行的触发器都念成「<应用名> · 策略」。
    for (const row of APP_ROWS) {
      expect(html, `${row.name} 那一行的触发器没有可访问名`).toContain(
        `aria-label="${row.name} · appPolicy.policy"`,
      );
    }
    // 🔴 反面：**不许**拿这颗控件自己的某一档当名字。上一版写的是 `appPolicy.summary.node`
    // （「指定节点」）—— 那还正是这一屏里点不动的那一档：一行当前为「直连」的 Telegram，
    // 读屏会念成「Telegram · 指定节点，按钮，直连」，把控件命名成它拒绝提供的档位。
    for (const tier of [
      'appPolicy.summary.node',
      'appPolicy.action.direct',
      'appPolicy.action.block',
      'appPolicy.followGlobal',
    ]) {
      expect(
        html,
        `触发器的 aria-label 里出现了档位名「${tier}」—— 控件被命名成了它的一个值`,
      ).not.toContain(`· ${tier}"`);
    }
    // 正向对照：谓词认得出「aria-label 里带档位名」这种形态（否则上面那串否定断言可能只是
    // 「aria-label 根本没渲染」）。
    expect(
      /aria-label="[^"]* · appPolicy\.[^"]*"/.test(html),
      'aria-label 整个没渲染出来 ⇒ 上面的否定断言零信息量',
    ).toBe(true);
  });

  it('触发器显示的是**这一行当前的档**，不是选项表里那句静态标签', () => {
    const html = appsMarkup();
    // 「指定节点」那一行的当前值是运行期数据（节点名）。触发器显示 `HK-01` 才算说对了自己选了什么；
    // 显示「指定节点」= 用户会读成「我选的那个节点没了」。
    expect(html, '触发器没有显示节点名 —— `valueLabel` 那一格断了').toContain(
      '<span class="mr-sel-val">HK-01</span>',
    );
  });

  it('触发器带**当前动作**的调色类，且「阻断」落在 `act-block`（动作标签轴恒 --err）', () => {
    const html = appsMarkup();
    expect(html, '阻断那一行的触发器没上 err 族的类').toContain('mr-sel-trigger act-block');
    expect(html).toContain('mr-sel-trigger act-direct');
    expect(html).toContain('mr-sel-trigger act-proxy');
    // 正向对照：调色不是恒挂的 —— 全部行都是「跟随全局」时，三个 act-* 一个都不该出现。
    const allFollow = appsMarkup(undefined, {
      rows: APP_ROWS.map((row) => ({ ...row, policyTone: 'follow' as const })),
    });
    expect(
      /mr-sel-trigger act-/.test(allFollow),
      '默认态也在上色 ⇒ 上一条断言只是「类名恒在」，说不出它跟着档位走',
    ).toBe(false);
  });

  it('面板：三档策略 + 按订阅折叠的节点组，四档**都点得动**（批 10 起）', () => {
    const html = appsMarkup(undefined, { policySheetAppId: 'tiktok' });
    /*
     * 🔴 结构 2026-09-13 改过一次，连同判据一起重写。
     *
     * 旧形态是「四档平铺，其中两档 `disabled`」，旧断言钉的是「恰好两颗 disabled」。
     * 现在与桌面 `PolicySelector` 同形：**「指定节点」本来就不是一个档位**，它是节点列表本身
     * （桌面 `AppPolicyScreen.tsx:942-994`）。把它压成一个点不动的档位，正是上一版让
     * 「选哪个节点」整条消失的那一压。
     */
    for (const key of [
      'appPolicy.followGlobal',
      'appPolicy.action.direct',
      'appPolicy.action.block',
    ]) {
      expect(html, `策略面板里少了「${key}」这一档`).toContain(`>${key}</span>`);
    }
    const options = [...html.matchAll(/<button[^>]*role="option"[^>]*>[\s\S]*?<\/button>/g)].map(
      (m) => m[0],
    );
    /* 三档策略在场；两组节点默认**折叠** ⇒ 它们的选项此刻不渲染（`buildCselRows` 的口径）。 */
    expect(options.length, '选项列表塌了').toBe(3);
    expect(
      options.filter((opt) => /<button[^>]*\sdisabled/.test(opt)).length,
      '还有点不动的档 —— 四档全开那次改动没落到面板上',
    ).toBe(0);
    /* 「阻断」那一档带 danger（动作标签轴在面板里的落点）。 */
    expect(options.find((opt) => opt.includes('appPolicy.action.block'))).toContain(
      'mr-sel-opt danger',
    );
    /* 当前档要带勾（`select-sheet` 与 `action-sheet` 的唯一视觉分界）。 */
    expect(options.filter((opt) => opt.includes('aria-selected="true"')).length).toBe(1);
    expect(options.find((opt) => opt.includes('aria-selected="true"'))).toContain(
      'appPolicy.action.block',
    );
    /* §4.12：这台设备按什么识别应用，必须是**可见文字**常驻在面板顶部。 */
    expect(
      textNodesOf(html),
      '策略面板顶部没有关键平台说明',
    ).toContain('mobileHelp.appRoutingMatch');
  });

  it('🔴 节点按订阅分组，组头可折叠、带计数，且默认全折叠（不猜第一组）', () => {
    /*
     * 🔴 **只在面板那一段上判**。节点名在**行的触发器**上也出现一次
     * （`<span class="mr-sel-val">HK-01</span>` —— 那正是「当前选的是哪个节点」该显示的地方），
     * 对整份 HTML 判 `not.toContain('>HK-01</span>')` 会被那一处喂红，而它与折叠无关。
     * 这是取材面宽于意图面的老形态：判据没错，取材切歪了。
     */
    const panelOf = (html: string): string => html.slice(html.indexOf('mr-sheet-scrim'));
    const closed = panelOf(appsMarkup(undefined, { policySheetAppId: 'youtube' }));
    const headers = [...closed.matchAll(/<button[^>]*class="mr-sel-grp mr-sel-grp-t[^"]*"[\s\S]*?<\/button>/g)].map(
      (m) => m[0],
    );
    expect(headers.length, '可折叠的分组组头没渲染 —— `Csel` 的组头这一维又没跟过来').toBe(2);
    for (const header of headers) {
      /* 组头**刻意不给 role**（listbox 里没有「可展开组头」这个角色）：靠原生 button 的隐含角色
         + `aria-expanded`，与桌面 `.csel-grp-t` / `.ns-grp` 同形。 */
      expect(header, '组头没有展开态语义').toContain('aria-expanded="false"');
      expect(header, '组头没有计数').toContain('class="mr-cnt"');
      expect(header, '组头缺 chevron 字形').toContain('M9 6l6 6-6 6');
    }
    /* 折叠时组内选项**不渲染**（这正是它存在的理由：390 宽的屏摆不下几十个节点）。 */
    expect(closed, '折叠着却把节点渲染出来了').not.toContain('>HK-01</span>');

    /* 展开那一组之后节点才出现 —— 正向对照：否则上一条会被「节点从来没渲染过」骗过。 */
    const opened = panelOf(
      appsMarkup(undefined, {
        policySheetAppId: 'youtube',
        policyOpenGroups: new Set(['sub-a']),
      }),
    );
    expect(opened, '展开之后节点仍不渲染 ⇒ 组头是个死控件').toContain('>HK-01</span>');
    expect(opened, '另一组不该跟着一起展开（多组可同时开，但不是手风琴式联动）').not.toContain(
      '>JP-02</span>',
    );
    /* 当前选中的那个节点要带勾 —— 几十个节点里勾不出「现在是哪一个」等于没选过。 */
    const nodeOption = /<button[^>]*role="option"[^>]*aria-selected="true"[\s\S]*?<\/button>/.exec(
      opened.slice(opened.indexOf('Sub A')),
    );
    expect(nodeOption?.[0], '展开组里当前节点没带选中态').toContain('HK-01');
  });

  it('自定义应用才有删除键，内置预设一颗都没有', () => {
    const html = appsMarkup();
    /* 夹具里恰好一条自定义（TikTok）⇒ 删除键恰好一颗。数量判据比「包含」强：
       「每一行都长出删除键」与「只有自定义那行有」在 `toContain` 下长得一样。 */
    expect(
      [...html.matchAll(/class="mr-btn danger[^"]*"/g)].length,
      '删除键数 ≠ 自定义应用数 —— 内置预设也长出了一颗删不掉的删除键',
    ).toBe(1);
    /* 武装态落 `.confirming`（与 `lib/confirm-twice.ts` 的跨文件契约，漏了它永远删不掉）。 */
    const armed = appsMarkup(undefined, { removeConfirmingId: 'tiktok' });
    expect(armed).toContain('class="mr-btn danger confirming"');
    expect(armed, '武装态没换成「再点一次」').toContain('common.confirmAgain');
  });

  it('卡片视图与列表视图承载的第二格不同（这是双视图存在的理由）', () => {
    const list = appsMarkup(undefined, { view: 'list' });
    const cards = appsMarkup(undefined, { view: 'cards' });
    /* 列表行第二格是**进程名**，卡片第二格是**分类** —— 桌面就是这么分的。 */
    expect(list, '列表行没画进程名').toContain('>netflix.exe</div>');
    expect(cards, '卡片视图不该画进程名（它的第二格是分类）').not.toContain('>netflix.exe</div>');
    expect(cards, '卡片没画分类').toContain('mr-app-card');
    /* 两种排布都走多列容器（无序集合）；有序列表那条例外只针对规则列表。 */
    expect(cards, '卡片视图没走多列容器').toContain('data-flow="multi"');
    expect(list, '列表视图没走多列容器').toContain('data-flow="multi"');
  });

  it('正向对照：面板不开时它一个字都不渲染（否则上一条会被「面板恒在」骗过）', () => {
    const closed = appsMarkup();
    expect(closed).not.toContain('role="option"');
    expect(closed.match(/role="dialog"/g) ?? []).toEqual([]);
  });

  it('写失败贴在**动手的那一行**下面（移动端没有 toast 宿主）', () => {
    const html = appsMarkup(undefined, { errorOf: errorOfWith('appRule:telegram') });
    expect(html, '策略写失败没有可见通道 —— 表现就是「点了没反应」').toContain(ERR_TEXT);
    // 反面：没失败时不许挂着红字（否则上一条会被「恒显一条红」骗过）。
    expect(appsMarkup()).not.toContain(ERR_TEXT);
  });
});

describe('③ §4.12：没有一处解释只挂在 `data-tip` 上', () => {
  it('本屏源码里 `data-tip` 出现 0 次（触屏没有 hover）', () => {
    // 正面对照：这个词在桌面**确实**是活的写法，不是一个抓不到的幽灵。
    const desktop = readFileSync(join(HERE, '../../../components/screens/rules/RuleItem.tsx'), 'utf8');
    expect(desktop, '桌面 `RuleItem.tsx` 里没有 data-tip —— 对照失效，本条断言无信息量').toContain(
      'data-tip=',
    );
    expect(SOURCE, '移动端出现了 `data-tip`：触屏没有 hover，挂上去等于没有').not.toContain(
      'data-tip',
    );
  });

  /**
   * 逐条枚举「桌面只靠 tip 承载的解释」，断言它在移动端**渲染出来的文字里**。
   * §4.12 的验收口径写得很死：per item, not in aggregate —— 一个计数，不是一种印象。
   */
  const RESIDENT: ReadonlyArray<readonly [string, string, () => string]> = [
    ['RuleItem.tsx:307 路由效果未生效', 'rules.routeInactiveHint', () => trafficMarkup()],
    ['RuleItem.tsx:340 待保存', 'home.stagedOnlyHint', () => trafficMarkup()],
    ['RuleItem.tsx:346 覆盖组网', 'rules.meshOverlapTip', () => trafficMarkup()],
    ['RuleItem.tsx:352 资源缺失', 'rules.resourceMissingTip', () => trafficMarkup()],
    ['RuleItem.tsx:361 节点已失效', 'rules.targetMissingTip', () => trafficMarkup()],
    ['RuleItem.tsx:389 置顶', 'rules.moveTop', () => trafficMarkup()],
    ['RuleItem.tsx:401 上移', 'rules.moveUp', () => trafficMarkup()],
    ['RuleItem.tsx:413 下移', 'rules.moveDown', () => trafficMarkup()],
    ['RuleItem.tsx:425 置底', 'rules.moveBottom', () => trafficMarkup()],
    ['RuleItem.tsx:437 启停', 'rules.toggleEnabled', () => trafficMarkup()],
    ['RuleItem.tsx:449 复制', 'rules.duplicate', () => trafficMarkup()],
    ['RuleItem.tsx:462 编辑', 'common.edit', () => trafficMarkup()],
    ['RuleItem.tsx:476 删除', 'common.delete', () => trafficMarkup()],
    ['RulesScreen.tsx:680 优先级说明', 'rules.priorityTip', () => trafficMarkup()],
    ['GeoCard.tsx:147 回国说明', 'rules.backHomeTip', () => trafficMarkup()],
    [
      'RulesScreen.tsx:653 系统规则恒优先',
      'rules.dnsWorkspace.systemRulesAlwaysFirst',
      () => dnsMarkup(),
    ],
    [
      'DnsPolicyWorkspace.tsx:291 内置服务器必须启用',
      'settings.dns.builtinRequired',
      () => dnsResourceMarkup(),
    ],
    ['AppPolicyScreen.tsx:369 应用分流总开关', 'appPolicy.masterTip', () => appsMarkup()],
    ['ResourcesScreen.tsx:532 取消下载', 'resources.cancel', () => resourcesMarkup('r4')],
    ['ResourcesScreen.tsx:547 更新', 'resources.update', () => resourcesMarkup('r3')],
    ['ResourcesScreen.tsx:547 立即重试', 'resources.retryNow', () => resourcesMarkup('r2')],
    ['ResourcesScreen.tsx:565 删除资源', 'common.delete', () => resourcesMarkup('r2')],
    ['ResourcesScreen.tsx:650 未引用', 'resources.unreferenced', () => resourcesMarkup(null)],
  ];

  it.each(RESIDENT)('%s → 常驻渲染（%s）', (_desktop, key, render) => {
    /*
     * **必须只看文字节点。** 直接对整段 HTML `toContain` 会被 `aria-label="..."` 满足 ——
     * 而属性不是「常驻通道」：屏幕上看不见它。实证过一次：`rules.toggleEnabled` 当时只出现在
     * 开关的 `aria-label` 里，这条断言照样绿。剥掉所有标签（React 已把属性值里的 `<>&"'` 转义，
     * 故 `[^>]*` 不会越界）后再比。
     */
    const text = textNodesOf(render());
    expect(text, `${key} 在移动端没有任何**可见**的常驻落点（只在属性里不算）`).toContain(key);
  });

  it('计数：桌面这四个屏的 tip 解释一条不落（枚举本身不许缩水）', () => {
    // 缩短这张表 = 悄悄放弃几条解释。用一个下界钉住它，改表要先改这个数。
    expect(RESIDENT.length).toBeGreaterThanOrEqual(23);
  });
});

describe('④ 内容顺序与 IA §2.2 对拍（DOM 顺序即重要性顺序）', () => {
  it('流量段：模式警告 → 地区分流 → 两条限定说明 → 规则数 → 有序列表', () => {
    const html = trafficMarkup();
    const at = (needle: string): number => {
      const i = html.indexOf(needle);
      expect(i, `${needle} 不在流量段里 —— 判据面塌了`).toBeGreaterThan(-1);
      return i;
    };
    const order = [
      at('rules.modeWarn'),
      at('rules.regionRouting'),
      at('rules.modeNote'),
      at('rules.priorityTip'),
      at('data-ordered="1"'),
    ];
    // 反面：手动接管那条说明**整条不移植**（Android 没有 manual 接管，见 `TrafficSegment` 注释）。
    expect(html, '「手动接管」说明又出现在移动端流量段里 —— 那条指引指向一个不存在的本地端口').not.toContain(
      'rules.manualNote',
    );
    expect(order, 'DOM 顺序与 IA §2.2 的内容顺序不一致').toEqual([...order].sort((a, b) => a - b));
  });

  it('规则行的序号就是 1-based 优先级（丢了它，列表读不出求值次序）', () => {
    const html = trafficMarkup();
    const nums = [...html.matchAll(/class="mr-pri"[^>]*>(\d+)</g)].map((m) => m[1]);
    expect(nums).toEqual(['1', '2', '3']);
  });
});

describe('⑤ 接线：容器在 store 全空（冷启动）时渲染得出来', () => {
  /**
   * ①–④ 测的是**方法体**（分段拿夹具渲染）。这一条测的是**接线**：容器把 store / IPC / 派生量
   * 串起来的那一层。两者是两件事 —— 分段全绿而容器在冷启动第一帧就抛（`config` 还是 undefined、
   * 预设表还没到、资源列表还没拉），是移动端最典型的白屏形态，而且它在真机上**零日志**。
   *
   * 用 `renderToStaticMarkup`：`useEffect` 不会跑 ⇒ 不发任何 IPC，测的正是「首帧」。
   */
  it('首帧不抛，且渲染出了屏头与四路分段条', () => {
    const html = renderToStaticMarkup(<MobileRulesScreen />);
    expect(html.length, '容器首帧渲染是空的').toBeGreaterThan(500);
    expect(html).toContain('mobileNav.rules');
    for (const seg of ['traffic', 'dns', 'apps', 'resources']) {
      expect(html, `分段条少了 ${seg}`).toContain(`mobileRules.seg.${seg}`);
    }
    // 冷启动落在流量分段：规则集为空 ⇒ 空态文案，而不是一片白。
    expect(html).toContain('rules.empty');
  });
});

/* ═══════════ 写失败的可见性 ═══════════ */

/** 取出每个 `catch (...) { … }` 的**块体**（花括号配平，能穿过嵌套块与字符串）。 */
function catchBlocks(src: string): string[] {
  const out: string[] = [];
  const head = /\bcatch\s*(?:\([^)]*\)\s*)?\{/g;
  let m: RegExpExecArray | null;
  while ((m = head.exec(src)) !== null) {
    let depth = 1;
    let i = m.index + m[0].length;
    const start = i;
    while (i < src.length && depth > 0) {
      if (src[i] === '{') depth += 1;
      else if (src[i] === '}') depth -= 1;
      i += 1;
    }
    out.push(src.slice(start, i - 1));
  }
  return out;
}

describe('⑤bis 资源行的大小与更新时间（`data-contract.json#sources.rule-resources`，2026-09-06 登记）', () => {
  /**
   * 这一组是「登记之后必须真的画出来」那一半。另一半（登记表里确实有这条源）在下面第三条。
   *
   * 三态优先于时间戳是**桌面同一条分支顺序**（`ResourcesScreen.tsx:488-494`）：一次没落地的
   * 更新旁边印「刚刚更新」是关于该次抓取的假话。故失败 / 已取消 / 下载中三态各自换掉它。
   */
  it('正常行：大小与更新时间**两格都画**，且走的是全仓那两条格式化腿', () => {
    const html = resourcesMarkup(null);
    const meta = /<div class="rc-meta" data-res-meta="r1">([\s\S]*?)<\/div>/.exec(html);
    expect(meta, '正常行的大小/更新时间那一格不见了 —— 登记了却没画').not.toBeNull();
    // `fmtBytes(1536)` = `1.5 KB`：断言的是**这条腿算出来的值**，不是「有个数字」。
    expect(meta![1]).toContain('resources.col.size');
    expect(meta![1]).toContain(fmtBytes(1536));
    expect(meta![1]).toContain('resources.col.updated');
    // 时基固定 ⇒ 2020 年那个时间戳恒落在「N 天前」那一档。
    expect(meta![1]).toContain('common.relDaysAgo');
  });

  it('反向对照：数字不是写死的 —— 换一个 size，画出来的就得跟着换', () => {
    const html = resourcesMarkup(null);
    expect(html).toContain(fmtBytes(2048));
    expect(html).toContain(fmtBytes(4096));
    expect(html, '0 字节要如实画成 0，不许当成「没有值」吞掉').toContain(fmtBytes(0));
  });

  it('三态优先：下载中 / 失败 / 已取消的行**不印**时间戳（那是上一次那个文件的）', () => {
    const html = resourcesMarkup(null);
    const metaOf = (id: string): string => {
      const hit = new RegExp(`<div class="rc-meta" data-res-meta="${id}">([\\s\\S]*?)</div>`).exec(html);
      expect(hit, `${id} 那一行的 meta 格没渲染`).not.toBeNull();
      return hit![1];
    };
    // 正面：三态行仍然画大小（大小描述的是盘上那个文件，它没有骗人）。
    for (const id of ['r2', 'r4']) expect(metaOf(id)).toContain('resources.col.size');
    // 反面：三态行不许出现「更新于」。
    expect(metaOf('r2'), '失败的行印了更新时间 —— 那是上一次成功那份的').not.toContain(
      'resources.col.updated',
    );
    expect(metaOf('r4'), '下载中的行印了更新时间').not.toContain('resources.col.updated');
    // 缺时间戳的行：画「更新于 —」，不许伪造成「刚刚」。
    expect(metaOf('r3')).toContain('resources.col.updated');
    expect(metaOf('r3')).toContain('—');
    expect(metaOf('r3')).not.toContain('common.relJustNow');
  });

  it('那句「未登记所以不画」的说明必须已经撤掉（缺口没了，描述缺口的话就成了假话）', () => {
    expect(resourcesMarkup(null)).not.toContain('resourceMetaUnregistered');
    expect(SOURCE, '容器里还在消费那条已撤销的键').not.toContain('resourceMetaUnregistered');
    for (const lang of ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa']) {
      const json = JSON.parse(
        readFileSync(join(HERE, '../../../i18n/locales', `${lang}.json`), 'utf8'),
      ) as { mobileRules: Record<string, unknown> };
      expect(
        json.mobileRules.resourceMetaUnregistered,
        `${lang} 里那条键还在 —— 键留着就还会有人去消费它`,
      ).toBeUndefined();
    }
  });

  /**
   * 数据源那一侧。**不读 `data-contract.json` 本体**：那份契约住在 vault（仓外 `~/docs`），
   * CI 上根本不存在 —— 读它的门在本机绿、在 CI 上 ENOENT 红，那不是判据是绊线。
   * 能在仓内钉住的是「这两格接的是哪条数据线」：容器必须从 `RuleResourceListItem` 的
   * `size` / `downloadedAt` 取值，而不是从别处编一个出来。契约文件里那条登记是**人**要核的，
   * 由 `ResourcesSegment` 头注与 commit 说明指路。
   */
  it('数据线：两格取的是 `RuleResourceListItem` 的 `size` / `downloadedAt`，不是别处编的', () => {
    const container = strip(read('RulesScreen.tsx'));
    const from = container.indexOf('const row: ResourceRowModel = {');
    expect(from, '资源行的视图模型装配点找不到了 —— 切片口径变了').toBeGreaterThan(0);
    const slice = container.slice(from, container.indexOf('};', from));
    expect(slice, '`size` 不是从列表项取的').toMatch(/size:\s*item\.size\b/);
    expect(slice, '`updatedAt` 不是从列表项的 `downloadedAt` 取的').toMatch(
      /updatedAt:\s*item\.downloadedAt\b/,
    );
    // 反向对照：切片确实切得住 —— 换掉取值处，谓词要跟着翻。
    const drifted = slice.replace('size: item.size', 'size: 0');
    expect(drifted).not.toBe(slice);
    expect(/size:\s*item\.size\b/.test(drifted)).toBe(false);
  });
});

describe('⑥ 写操作失败必须有**可见**回显（行内，贴着那颗控件）', () => {
  /*
   * 立项时的根因，两处：
   *  ① `lib/error-handler.ts` 的 `toast` 默认实现是 `consoleToast`（只 `console.*`）；
   *  ② 真实实现由 `setToastImpl` 注入，而当时唯一注入点是 `components/layout/Toaster.tsx`，
   *     它只挂在桌面 `components/layout/AppShell.tsx` 上。
   * ⇒ 移动端 `toast.error(...)` = `console.error(...)`。写失败时用户只看到「开关弹回去」，
   * 一个字都没有 —— 正是 `contracts/action-failure-visibility.test.ts` 那批真机缺陷的形态。
   *
   * 🔴 **② 2026-09-06 已经不成立**：移动端有了自己的宿主（`mobile/MobileToaster.tsx`，
   * 由 `MobileApp` 挂在停靠区），本屏那批 `toast.*` 不再静音。本组**不因此撤销**，
   * 它守的判据换了理由，没换内容：
   *  · 行内回显**贴着那颗控件**，toast 是全局瞬态的。本屏一次可以有好几处并发的写
   *    （逐条规则的开关、排序、资源下载），一条飘过去的 toast 说不清是哪一条失败了；
   *  · 两者不互斥 —— 本组要的是前者，下面那条源码断言一格没松。
   * 自检也随之改成**正面**取证：证明宿主真的在（而不是证明它不在），
   * 这样「有人把宿主拆了」与「有人把行内回显删了」各有一条判据说话。
   */
  it('自检：门面仍是可注入的，且移动端的宿主真的在（正面取证）', () => {
    const repoUi = fileURLToPath(new URL('../../../', import.meta.url));
    const handler = readFileSync(join(repoUi, 'lib/error-handler.ts'), 'utf8');
    const toaster = readFileSync(join(repoUi, 'components/layout/Toaster.tsx'), 'utf8');
    const shell = readFileSync(join(repoUi, 'components/layout/AppShell.tsx'), 'utf8');
    expect(handler, 'toast 的默认实现不再是 console —— 本组的前提变了，请重判').toContain(
      'let toastImpl: ToastImpl = consoleToast;',
    );
    expect(toaster, '桌面注入点变了').toContain('setToastImpl(');
    expect(shell, '`<Toaster />` 不再挂在桌面外壳上 —— 前提变了，请重判').toContain('<Toaster />');
    // 移动端那一侧：宿主在自己的文件里注入，且由 `MobileApp` 装配。
    const mobileToaster = readFileSync(join(repoUi, 'mobile/MobileToaster.tsx'), 'utf8');
    expect(mobileToaster, '移动端宿主不再注入门面 —— 那 24 处反馈会重新落 console').toContain(
      'setToastImpl(',
    );
    const mobileApp = readFileSync(join(repoUi, 'mobile/MobileApp.tsx'), 'utf8');
    expect(mobileApp, '`<MobileToaster />` 没有被装配 —— 宿主写了但没人挂 = 仍然静音').toContain(
      '<MobileToaster />',
    );
    // 移动外壳**仍然**不许直接引桌面 Toaster（它的外观住在桌面层叠链上，契约 A1）。
    const mobileShell = readFileSync(join(repoUi, 'mobile/MobileShell.tsx'), 'utf8');
    expect(
      mobileShell,
      '移动外壳引了桌面 Toaster —— 整条桌面 CSS 层叠链会被拖进移动包',
    ).not.toContain("from '@/components/layout/Toaster'");
  });

  it('源码面：每一个 catch 块都要走一条**可见**通道，不许只 console/toast/回滚', () => {
    const blocks = catchBlocks(SOURCE);
    // 自检：真的扫到了 catch（扫不到就退化成「没有 catch 所以全绿」）。
    expect(blocks.length, `只扫到 ${blocks.length} 个 catch 块 —— 判据面塌了`).toBeGreaterThanOrEqual(8);
    /*
     * 本屏只有两条**可见**通道，逐条枚举（不是「含某个关键字就放行」的万能豁免）：
     *  · `reportWriteError(` → 控件旁边的行内红字；
     *  · `setResError(`      → 资源列表的「加载失败」整段替换（下面另有一条断言证明它真会渲染）。
     */
    const offenders = blocks.filter(
      (b) => !b.includes('reportWriteError(') && !b.includes('setResError('),
    );
    expect(
      offenders.map((b) => b.trim().slice(0, 110)),
      '这些 catch 没有任何用户可见的回显 —— 失败会表现成「点了没反应」',
    ).toEqual([]);
  });

  it('源码面：不许有空 catch / `.catch(() => {})` 这类静默吞错', () => {
    expect(SOURCE).not.toMatch(/\.catch\(\s*\(\s*\)\s*=>\s*\{\s*\}\s*\)/);
    for (const block of catchBlocks(SOURCE)) {
      expect(block.trim().length, '出现了空 catch 块').toBeGreaterThan(0);
    }
  });

  /** 逐段注入一个写失败，断言它出现在**文字节点**里（属性里不算，同 ③ 组的口径）。 */
  const FAILURE_SURFACES: ReadonlyArray<readonly [string, () => string]> = [
    ['流量段 · 切回智能失败', () => trafficMarkup(RULE_ROWS, errorOfWith('proxyMode'))],
    ['流量段 · 地区分流保存失败', () => trafficMarkup(RULE_ROWS, errorOfWith('regionRouting'))],
    ['流量段 · 重排失败', () => trafficMarkup(RULE_ROWS, errorOfWith('order:route'))],
    ['流量段 · 单条规则写失败', () => trafficMarkup(RULE_ROWS, errorOfWith('rule:a'))],
    ['DNS 段 · 重排失败', () => dnsMarkup(errorOfWith('order:dns'))],
    ['DNS 段 · 单条规则写失败', () => dnsMarkup(errorOfWith('rule:b'))],
    ['DNS 二级页 · 开关写失败', () => dnsResourceMarkup(errorOfWith('dnsres:x'))],
    ['应用段 · 总开关写失败', () => appsMarkup(ERR_TEXT)],
    ['资源段 · 全部更新失败', () => resourcesMarkup(null, { errorOf: errorOfWith('res:all') })],
    ['资源段 · 重置内置失败', () => resourcesMarkup(null, { errorOf: errorOfWith('res:reset') })],
    ['资源段 · 单条资源写失败', () => resourcesMarkup(null, { errorOf: errorOfWith('res:r2') })],
  ];

  it.each(FAILURE_SURFACES)('行为面：%s → 用户看得见', (_name, render) => {
    expect(textNodesOf(render()), '写失败没有任何可见回显').toContain(ERR_TEXT);
  });

  it('行为面：没有失败时不许挂着红字（否则上一组会被「恒显一条红」骗过）', () => {
    for (const html of [
      trafficMarkup(),
      dnsMarkup(),
      dnsResourceMarkup(),
      appsMarkup(),
      resourcesMarkup(null),
    ]) {
      expect(html).not.toContain(ERR_TEXT);
      expect(html).not.toContain('data-write-error');
    }
  });

  it('第二条可见通道确实会渲染：列表加载失败 → 整段换成失败文案', () => {
    const html = renderToStaticMarkup(
      <ResourcesSegment
        t={t}
        source="all"
        onSourceChange={() => {}}
        loading={false}
        error
        groups={[]}
        onUpdateAll={() => {}}
        updatingAll={false}
        updatingIds={new Set()}
        onResetBuiltin={() => {}}
        resetConfirming={false}
        errorOf={noErrors}
        onUpdateOne={() => {}}
        onCancel={() => {}}
        onDelete={() => {}}
        deleteConfirmingId={null}
        sheetId={null}
        onOpenSheet={() => {}}
          onCatalog={() => {}}
      onUrlDownload={() => {}}
      />,
    );
    expect(textNodesOf(html)).toContain('resources.loadError');
  });
});

describe('⑦ 「再点一次确认」的 `.confirming` 契约（跨文件，静态可测）', () => {
  /*
   * `lib/confirm-twice.ts` 在武装期间于 document 上挂 capture 阶段的 `pointerdown`，
   * 凡是没落在 `.confirming` 里的按下一律 `reset()`。**类名不是样式，是判据**：
   * 漏了它，第二次点击的 `pointerdown` 先把 armed 清掉、随后的 `click` 又重新武装 ⇒
   * 文案在「删除 ↔ 再点一次确认」之间无限翻转，那条删除**永远执行不到**。
   * 桌面 `RuleItem.tsx` / `ResourcesScreen.tsx` 两处都带它。
   */
  it('自检：契约仍在（`confirm-twice` 确实按 `.confirming` 判定）', () => {
    const core = readFileSync(
      fileURLToPath(new URL('../../../lib/confirm-twice.ts', import.meta.url)),
      'utf8',
    );
    expect(core, '`confirm-twice` 不再按 `.confirming` 判定 —— 本组的前提变了').toContain(
      "CONFIRMING_CLASS = '.confirming'",
    );
    expect(core).toContain('closest');
  });

  it('规则删除：武装态的面板条目带 `.confirming`，未武装不带', () => {
    const armedHtml = trafficMarkup([
      bindingOf(ruleOf('a'), { sheetOpen: true, deleteConfirming: true }),
    ]);
    expect(armedHtml, '武装态的删除条目没有 `.confirming` —— 这条删除永远确认不了').toMatch(
      /class="mr-sheet-item danger confirming"/,
    );
    const idleHtml = trafficMarkup([bindingOf(ruleOf('a'), { sheetOpen: true })]);
    expect(idleHtml).not.toContain('confirming');
  });

  it('资源删除：同一条契约', () => {
    const armed = resourcesMarkup('r2', { deleteConfirmingId: 'r2' });
    expect(armed).toMatch(/class="mr-sheet-item danger confirming"/);
    expect(resourcesMarkup('r2')).not.toContain('confirming');
  });

  it('重置内置资源：同一条契约（它不在面板里，是屏内一颗按钮）', () => {
    expect(resourcesMarkup(null, { resetConfirming: true })).toMatch(/class="rc-reset confirming"/);
    expect(resourcesMarkup(null)).not.toContain('confirming');
  });
});

/* ═══════════ 分段控件的轨道层 ═══════════ */

/**
 * 取出某个选择器那一条规则的声明面（已剥注释）。找不到返回 `null` —— 调用点必须把
 * `null` 当红，不许当「没这条规则所以通过」。
 */
/**
 * `selector` 上每个**语义分量的胜出值**（`键: 值;` 逐条拼接），走共享解析器
 * `styles/css-cascade.test-support.ts`。选择器一条都没命中 ⇒ **抛**（不是返回 null 让上面恒真）。
 *
 * 迁移前是「按 `m[1].trim().replace(/\s+/g,' ')` **字符串全等**配对选择器、收全部规则体拼接」。
 * 收全部已经修掉了「首条命中」那一半，剩下的两个洞 2026-09-05 都实测溜过去了：
 *  · **选择器按字面配对**：在文件末尾补一条**选择器组**
 *    `.mr-geo-regions .mr-btn[aria-pressed='true'], .mr-geo-regions .mr-btn[aria-selected='true']{…}`，
 *    与本函数传入的单个选择器字符串不全等 ⇒ 整条收不到，被桌面当面否决过的 `flow-weak` 铺底原样复活而全绿。
 *    现在按**归一化键**逐项配对，选择器组里的每一项都算数。
 *  · **同族属性不认**：`min-(height|width)` 的禁令看不见 `min-block-size` / `min-inline-size`，
 *    48px 命中面被砍到 24 仍全绿。现在两者折成同一个语义分量键 `min-size-block` / `min-size-inline`。
 * 顺带把「拼接 = 写过就算」也改成「胜出值」：末尾追加 `.mr-btn{min-height:24px}` 现在会红。
 *
 * **射程边界**：只收本选择器键自己的声明；「更窄选择器从旁边压下来」由 [`narrowerOf`] 那一桶自曝。
 *
 * **条件面**：本函数吃解析器默认谓词 —— 只看无条件声明，而同键上一旦出现条件块里的声明就**抛**
 * （BLK-2 失败关闭）。此处此前写着「断点块能不能胜出由 `styles/cascade-dead-rules.test.ts`
 * 全仓看住」，那句话不成立：该门守的是「条件块写在同名顶层规则**之前**」这种死规则，
 * 对「条件块写在顶层规则**之后**」（也就是真能生效的那种写法）一句话都不说。
 * 否定/穷举判据要看条件档的，走下面的 [`declsAnywhere`]。
 */
const declsOf = (selector: string): string => winnersText({ sel: selector, ctx: 'mobile', where: UNCONDITIONAL });

/**
 * 同上，但**条件档一并收**（`where: ALL`）。
 *
 * 2026-09-05 MAJ-9：迁移时把三处**否定/穷举**判据的条件面从「扫进 @container/@media 块体」
 * 收窄成了默认的 UNCONDITIONAL，实测两条真回归（`@container` 里补一句 `pointer-events: none`
 * 或另开一个 sticky 入口，新门 rc=0 而旧读法必红）。
 * 分工：**正面取值**判据只量基础档（`declsOf`），**否定/穷举**判据必须覆盖全部条件档（本函数）——
 * 「某一档下这句话不成立」正是这类判据要抓的形状。
 */
const declsAnywhere = (selector: string): string =>
  winnersText({ sel: selector, ctx: 'mobile', where: ALL });
/** 同上，但拿到分量表本身（要点名某个分量的值时用）。 */
const wonMap = (selector: string) => winners({ sel: selector, ctx: 'mobile', where: UNCONDITIONAL }).won;

/**
 * `selector` 上「有人从更窄处压你」的**选择器全集**（`file:line 选择器` 去重排序）。
 *
 * 2026-09-05 MAJ-3：本文件的 `declsOf` / `wonMap` 全程只调 `winners`，而那时 `winners` 不算
 * narrower 桶 ⇒ 「更窄选择器从旁边压下来」这一族对 ⑧ 等于不存在。
 * ⚠️ 它只收**语法上严格更窄**的选择器：互不包含却命中同一元素的那一族（`.mr-screen .mr-ap-body.dim`
 * 之于 `.mr-ap-body.dim` 是更窄的，但 `.mn-row .mn-lat` 之于 `.mn-lat.fast` 不是）进不了任何桶，
 * 那一族归 ⑮ 组的浏览器裁判。此前桶里只收**与本选择器重叠的
 * 分量**，所以清单是可穷举的：下面把它钉成具名清单，多一条少一条都红。
 */
const narrowerOf = (selector: string): string[] =>
  [
    ...new Set(
      winners({ sel: selector, ctx: 'mobile', where: UNCONDITIONAL }).narrower.map((c) => c.decl.rawSels.join(', ')),
    ),
  ].sort();

describe('⑧ 分段控件：当前两层呈现与实际命中面', () => {
  afterAll(closeOracle, 30_000);

  it('应用列表/卡片选择仍是互斥双选，选中态有单独可见 surface', () => {
    const html = appsMarkup();
    expect(html).toContain('class="mr-geo-regions"');
    const track = html.match(/<div class="mr-geo-regions"[^>]*>([\s\S]*?)<\/div>/)?.[1] ?? '';
    expect((track.match(/aria-pressed="true"/g) ?? [])).toHaveLength(1);
    expect((track.match(/<button/g) ?? [])).toHaveLength(2);
    expect(declsOf('.mr-geo-regions')).toContain('border-radius');
    expect(declsOf('.mr-geo-regions .mr-btn')).toContain('background-color: transparent');
    expect(narrowerOf('.mr-btn')).toContain('.mr-screen .mr-btn');
  });

  it('窄屏浏览器里两个按钮各自可点，选中底只画在有限伪元素上', async () => {
    const html = inMobileShell(`<div class="mr-screen">${appsMarkup()}</div>`, 'rules');
    const m = await measure({ ctx: 'mobile', html, viewport: { width: 390, height: 844 } }, [
      { select: '.mr-geo-regions .mr-btn', props: ['min-height', 'background-color'], many: 'all' },
      { select: '.mr-geo-regions .mr-btn[aria-pressed="true"]', pseudo: '::before', props: ['background-color'] },
    ]);
    const buttons = m.rectAll('.mr-geo-regions .mr-btn');
    expect(buttons).toHaveLength(2);
    for (const button of buttons) expect(button.height).toBeGreaterThanOrEqual(44);
    expect(m.get('.mr-geo-regions .mr-btn[aria-pressed="true"]', 'background-color', '::before'))
      .not.toBe('rgba(0, 0, 0, 0)');
  }, 30_000);
});

describe('⑨ 分类筛选是 `select-sheet`，不是 `segmented-tabs`（IA §1.2 + §3.3 第 1 条）', () => {
  /*
   * §1.2 那一行写的是 “Category **select-sheet**”，桌面对应物是 `Csel`
   * （`AppPolicyScreen.tsx:466`）。§3.3 第 1 条又单独说了一遍：`.mini-menu` 与 `Csel` 是两套词汇，
   * 合并任何一对都会造出「两端都不存在的移动端专有词汇」。
   * 这里断言的是**渲染出来的 DOM**：类名在 CSS 里存在不算数，得真的渲染在分类筛选那一格上。
   */
  it('触发器在场，显示当前值，且分类那一格不再是 tablist', () => {
    const html = appsMarkup();
    expect(html, '分类筛选没有渲染 `select-sheet` 触发器').toContain('class="mr-sel-trigger"');
    expect(html, '触发器没有说出当前选中的分类 —— 收起来的下拉不说值就只是一颗按钮').toContain(
      'appPolicy.cat.all',
    );
    expect(html).toContain('aria-haspopup="listbox"');
    /*
     * 反面：本段不许再出现 `role="tablist"`。`.mr-seg` 只服务屏级四路分段条（那条在容器里，
     * 不在本段），分类筛选借用它正是把两套桌面词汇并成一套。
     */
    expect(html, '分类筛选又变回 `segmented-tabs` 了').not.toContain('role="tablist"');
    expect(html).not.toContain('class="mr-seg"');
  });

  it('面板列出全部分类，恰好一个选中，且选中那行带勾', () => {
    const open = appsMarkup(undefined, { categorySheetOpen: true });
    const opts = [...open.matchAll(/<button[^>]*class="mr-sel-opt"[^>]*>/g)];
    expect(opts.length, '面板里的分类数与夹具对不上 —— 判据面塌了').toBe(APP_CATEGORIES.length);
    expect(
      opts.filter((m) => m[0].includes('aria-selected="true"')).length,
      '不是恰好一个选中 —— `select-sheet` 的语义就是「一组里选一个」',
    ).toBe(1);
    expect(open, '选中勾没渲染 —— 它是 `select-sheet` 与 `action-sheet` 的唯一视觉分界').toContain(
      'class="mr-sel-ck"',
    );
    // 正面对照：关着的时候面板整个不在 DOM 里（否则上一条会被「面板恒在」骗过）。
    expect(appsMarkup()).not.toContain('class="mr-sel-opt"');
  });
});

describe('⑩ 总开关关闭 / 非智能模式下，应用段正文整块置灰（IA §2.2 States）', () => {
  /*
   * 规范那一行要的是**两件事**：`ap-off-note` 提示条 **plus the body dimmed**。
   * 只做提示条不够 —— 用户往下滚两屏，提示条早已滚出视野，屏上只剩一列看起来正常的应用行。
   * 桌面 `AppPolicyScreen.tsx:334` 的 `bodyDimmed = !enabled || !isSmartMode`，两个来源都要。
   */
  const bodyClass = (html: string): string | null =>
    /<div class="(mr-ap-body[^"]*)"/.exec(html)?.[1] ?? null;

  it('两个来源各自都能让正文变灰，且都开时不灰', () => {
    for (const [name, over] of [
      ['总开关关闭', { masterEnabled: false, isSmartMode: true }],
      ['非智能模式', { masterEnabled: true, isSmartMode: false }],
      ['两者都关', { masterEnabled: false, isSmartMode: false }],
    ] as const) {
      const cls = bodyClass(appsMarkup(undefined, over));
      expect(cls, `${name}：正文包裹层不见了 —— 判据面塌了`).not.toBeNull();
      expect(cls, `${name} 下正文没有置灰`).toContain('dim');
    }
    // 正向对照：两者都开时**不**灰（否则「恒灰」也能把上面三条骗过去）。
    const on = bodyClass(appsMarkup(undefined, { masterEnabled: true, isSmartMode: true }));
    expect(on, '两者都开时包裹层不见了 —— 判据面塌了').not.toBeNull();
    expect(on, '两者都开时正文仍是灰的').not.toContain('dim');
  });

  it('置灰只降透明度，不动可交互性（桌面 `.ap-disabled` 同一条口径）', () => {
    // **否定判据先跑**：它的取材面（`where: ALL`，全部条件档）比下面那条正面取值判据宽，
    // 而正面那条吃默认谓词、遇到条件声明会先抛 —— 先跑窄的那条，红的就永远是「没显式给 where」
    // 而不是「某一档下这句话不成立」，后者才是本条要说的话。
    const anywhere = declsAnywhere('.mr-ap-body.dim');
    expect(anywhere, '`.mr-ap-body.dim` 这条规则不见了 —— 判据面塌了').not.toBe('');
    // `@container` 里补一句 `pointer-events: none` 同样把正文点不动，
    // 而只看基础档的读法一个字都不会说（2026-09-05 MAJ-9 实测）。
    expect(
      anywhere,
      '置灰顺手把正文变成不可点了 —— 规范明写 controls stay editable',
    ).not.toMatch(/pointer-events\s*:|user-select\s*:|visibility\s*:|display\s*:/);
    expect(declsOf('.mr-ap-body.dim')).toMatch(/opacity\s*:/);
    // 被灰掉的正文里，行控件本身没有被加上 disabled。
    const dimmed = appsMarkup(undefined, { masterEnabled: false, isSmartMode: false });
    const search = /<input[^>]*type="search"[^>]*>/.exec(dimmed);
    expect(search, '搜索框不在灰掉的正文里 —— 判据面塌了').not.toBeNull();
    expect(search?.[0], '置灰把搜索框也禁用了').not.toContain('disabled');
  });

  /*
   * 置灰包裹层的**边界**：底部面板不许落在它里面。
   *
   * `opacity < 1` 有两个后果，都不是「看起来淡一点」：① 整棵子树作为**一组**合成 ⇒ 面板连同
   * 它那层本该压住全屏的遮罩一起变成半透明，正文透过遮罩可读，读起来像是坏了；
   * ② 就地建立**层叠上下文**，而 `.mr-ap-body` 自身非定位（`z-index: auto`）⇒ 整组排在
   * 「已定位且 z-index > 0」的元素之前绘制，`position: sticky; z-index: 2` 的屏头会画在面板之上。
   * 面板的 `position: fixed` 与它的矩形都是对的 —— 坏的是透明度与绘制序，肉眼查不出原因。
   *
   * 而置灰态恰恰是规范要求「controls stay editable」的那个态，也就是这个筛选器本该可用的时候。
   * 本屏原有的 `ActionSheet` 由容器渲染在 `</section>` 那一层，从来没这问题；这条判据把
   * 「面板归容器层」变成对**所有**面板成立的结构约束，而不是某一个面板的偶然摆法。
   */
  it('置灰态下打开分类面板：面板与遮罩都不在 `.mr-ap-body dim` 的子树内', () => {
    const html = appsMarkup(undefined, {
      masterEnabled: false,
      isSmartMode: false,
      categorySheetOpen: true,
    });
    const scrim = html.indexOf('class="mr-sheet-scrim"');
    expect(scrim, '面板没渲染出来 —— 下面每条恒绿').toBeGreaterThan(-1);

    // `.mr-ap-body dim` 那个 `<div>` 的子树范围，按标签深度配平切（不能用 `indexOf('</div>')`：
    // 正文里嵌了好几层 div，首个闭合标签落在最内层）。
    const open = html.indexOf('<div class="mr-ap-body dim">');
    expect(open, '置灰的正文包裹层不见了 —— 判据面塌了').toBeGreaterThan(-1);
    let depth = 0;
    let end = -1;
    for (const m of html.slice(open).matchAll(/<div\b[^>]*>|<\/div>/g)) {
      depth += m[0] === '</div>' ? -1 : 1;
      if (depth === 0) {
        end = open + m.index + m[0].length;
        break;
      }
    }
    expect(end, '置灰包裹层的标签没配平 —— 切片塌了').toBeGreaterThan(open);

    expect(
      scrim > open && scrim < end,
      '分类面板渲染在 `.mr-ap-body.dim` 内 ⇒ 置灰态下整个底部面板连遮罩一起半透明，' +
        '还会被 sticky 屏头压在上面。面板要和 `ActionSheet` 一样由容器渲染在置灰包裹之外。',
    ).toBe(false);

    // 正向对照：触发器**仍在**正文里（否则「面板与触发器一起被挪走了」也能让上一条通过）。
    const trigger = html.indexOf('class="mr-sel-trigger"');
    expect(trigger, '触发器不见了').toBeGreaterThan(-1);
    expect(
      trigger > open && trigger < end,
      '触发器被一起挪出了正文 —— 它是正文里的一个控件，该跟着置灰',
    ).toBe(true);
  });
});

describe('⑪ DNS 段屏头 = 溢出 + 添加，两颗封顶（IA §1.2、§2.1、§3.1 #10）', () => {
  /*
   * ⚠️ **这一组是源码面判据，不是渲染面。** 屏头的动作簇由容器渲染，而当前分段是容器内部的
   * `useState`，`renderToStaticMarkup(<MobileRulesScreen />)` 只到得了 traffic 那一帧 ——
   * 没有任何入口能把它渲成 dns。故这里退到源码面，并且**必须**带取材面自检：
   * 切不出块就红，不许「没找到所以通过」。渲染面能覆盖的那半（首帧屏头）在下面单列一条。
   */
  const SRC_RULES = strip(read('RulesScreen.tsx'));
  /*
   * 🔴 锚**从后往前**找。2026-09-13（批 10）给 DNS 两个二级页的屏头也加了一个 `.mr-acts`
   * （那颗「新建服务器 / 新建分组」），而它在源码里**排在根页前面** ⇒ 原来的
   * `indexOf('className="mr-acts"')` 会把切片起点落到二级页那一簇上，下面数出来的按钮数
   * 是两簇之和。取材面漂了而判据本身没错：从根页唯一的 `<SegmentedTabs` 往回找最近的一个
   * `.mr-acts`，切到的就恒是根页那一簇。
   */
  const ACTS = ((): string => {
    const to = SRC_RULES.indexOf('<SegmentedTabs');
    const from = to === -1 ? -1 : SRC_RULES.lastIndexOf('className="mr-acts"', to);
    return from === -1 || to === -1 ? '' : SRC_RULES.slice(from, to);
  })();

  /** 二级页那一簇（源码里第一处 `.mr-acts`，到 `<DnsResourcePage` 为止）。 */
  const PUSHED_ACTS = ((): string => {
    const from = SRC_RULES.indexOf('className="mr-acts"');
    const to = SRC_RULES.indexOf('<DnsResourcePage', from);
    return from === -1 || to === -1 ? '' : SRC_RULES.slice(from, to);
  })();

  it('自检：动作簇这一块切得出来，且切到的是真东西', () => {
    expect(ACTS.length, '`.mr-acts` 块没切出来 —— 下面每条断言都会变成恒真').toBeGreaterThan(200);
    expect(ACTS, '切出来的不是动作簇').toContain("segment === 'dns'");
    expect(ACTS).toContain("segment === 'apps'");
    /* 切点自检：根页那一簇里**不许**含二级页的标记 —— 含了就说明锚又漂回第一处去了，
       而那种漂移会让下面的按钮计数变成两簇之和（那正是本组 2026-09-13 修过的形态）。 */
    expect(ACTS, '切片跨进了二级页那一簇 —— 锚点漂了').not.toContain('mobileRules.back');
  });

  it('二级页的屏头只有「返回 + 新建」两颗，且新建按当前页分派', () => {
    expect(PUSHED_ACTS.length, '二级页动作簇没切出来 —— 下面恒真').toBeGreaterThan(100);
    expect(
      [...PUSHED_ACTS.matchAll(/<button\b/g)].length,
      '二级页动作簇里不止一颗按钮（加上返回键就超过 §3.1 #10 的两颗上限）',
    ).toBe(1);
    /* 新建的是服务器还是分组，由**当前在哪一页**决定 —— 桌面那颗 `+` 按 `dnsView` 分派，语义同。 */
    expect(PUSHED_ACTS).toContain("kind: 'dns-server'");
    expect(PUSHED_ACTS).toContain("kind: 'dns-group'");
  });

  it('DNS 分支只剩一颗溢出按钮，两个二级页入口移进了 `action-sheet`', () => {
    /* 只切 dns 专属那一支：它到流量段专属那支（`segment === 'traffic' && (`）为止。 */
    const from = ACTS.indexOf("{segment === 'dns' && (");
    expect(from, "dns 专属分支没切出来 —— 判据面塌了").toBeGreaterThan(-1);
    const trafficFrom = ACTS.indexOf("{segment === 'traffic' && (", from);
    expect(trafficFrom, '流量段专属那支没切出来 —— 判据面塌了').toBeGreaterThan(from);
    const to = ACTS.indexOf("{(segment ===", trafficFrom);
    expect(to, '「添加规则」那支没切出来 —— 判据面塌了').toBeGreaterThan(trafficFrom);
    const dnsBranch = ACTS.slice(from, trafficFrom);
    const trafficBranch = ACTS.slice(trafficFrom, to);
    expect(
      [...dnsBranch.matchAll(/<button\b/g)].length,
      'dns 专属分支里不止一颗按钮 —— 加上共用的「添加规则」就超过了 §3.1 #10 的两颗上限',
    ).toBe(1);
    /* 流量段同形：网络场景入口（spec §6.1，两个平面同一个入口）也从溢出进，不直接摆上屏头。 */
    expect(
      [...trafficBranch.matchAll(/<button\b/g)].length,
      '流量段专属分支里不止一颗按钮 —— 加上共用的「添加规则」就超过了 §3.1 #10 的两颗上限',
    ).toBe(1);
    expect(trafficBranch, '流量段溢出按钮没用 `MoreIcon`').toContain('<MoreIcon />');
    /* 屏头源码里一共几颗：dns / 流量两个互斥分支各一颗溢出 + 两段共用的「添加规则」= 3；
       **每个分段可见的**仍是「溢出 + 添加规则」两颗，正好压在上限上（互斥由上面两条分支切片证明）。 */
    expect(
      [...ACTS.matchAll(/<button\b/g)].length,
      '动作簇里的按钮总数变了 —— 重新数一遍每个分段实际可见几颗再改这个数',
    ).toBe(3);
    expect(ACTS, '网络场景入口直接摆在了屏头上 —— 它从 header overflow 进').not.toContain(
      'rules.networkProfile.entry',
    );
    expect(dnsBranch, '溢出按钮没用 `MoreIcon`').toContain('<MoreIcon />');
    expect(
      ACTS,
      '「服务器」「分组」还直接摆在屏头上 —— 规范要的是它们从 header overflow 进',
    ).not.toContain('serversTab');
    expect(ACTS).not.toContain('groupsTab');
  });

  it('溢出面板确实把那两个二级页推出去（入口没有变成一颗死按钮）', () => {
    const src = strip(read('RulesScreen.tsx'));
    const at = src.indexOf('headSheet && (');
    expect(at, '溢出面板那段的锚没找到 —— 判据面塌了').toBeGreaterThan(-1);
    const sheet = src.slice(at);
    expect(sheet.length, '溢出面板那段没切出来 —— 判据面塌了').toBeGreaterThan(200);
    expect(sheet).toContain('rules.dnsWorkspace.serversTab');
    expect(sheet).toContain('rules.dnsWorkspace.groupsTab');
    expect(sheet, '面板条目没有落到推页写路径上').toContain("setPage('dns-servers')");
    expect(sheet).toContain("setPage('dns-groups')");
    /* 网络场景（spec §6.1）：两个平面同一个入口，进同一页。 */
    expect(sheet).toContain('rules.networkProfile.entry');
    expect(sheet, '网络场景入口没有落到推页写路径上').toContain("setPage('network-profiles')");
  });

  it('渲染面能覆盖的那半：首帧屏头的动作控件不超过两颗', () => {
    const html = renderToStaticMarkup(<MobileRulesScreen />);
    const acts = /<div class="mr-acts">([\s\S]*?)<\/div><(?:div|nav)/.exec(html);
    expect(acts, '首帧没渲染出 `.mr-acts` —— 判据面塌了').not.toBeNull();
    expect(
      [...(acts?.[1] ?? '').matchAll(/<button\b/g)].length,
      '屏头动作控件超过两颗（§3.1 #10 的上限）',
    ).toBeLessThanOrEqual(2);
  });
});

describe('⑫ 规则行有抓握条，长按拖拽落到与四颗按钮同一条写路径上（IA §4.13）', () => {
  it('渲染面：每一条规则行都有抓握条，位置在序号之后、行主体之前', () => {
    for (const [name, html] of [
      ['流量', trafficMarkup([bindingOf(ruleOf('a')), bindingOf(ruleOf('b'))])],
      ['DNS', dnsMarkup()],
    ] as const) {
      /* 逐行拆开看：整段 `toContain` 只能证明「某处有一个抓握条」，证不了**每一行**都有。 */
      // 前瞻挡开 `mr-row-main` / `mr-row-title` 这些同前缀的类名（下一个字符是 `-`）。
      const rows = html.split(/<div class="mr-row(?=["\s])/).slice(1);
      expect(rows.length, `${name} 段没渲染出规则行 —— 判据面塌了`).toBeGreaterThan(0);
      for (const [i, row] of rows.entries()) {
        const pri = row.indexOf('class="mr-pri"');
        const grip = row.indexOf('data-drag-handle="1"');
        const main = row.indexOf('class="mr-row-main"');
        expect(grip, `${name} 段第 ${i + 1} 行没有抓握条 —— §4.13 要的第一件事就没有`).toBeGreaterThan(
          -1,
        );
        // 位置：`.mr-pri` → 抓握条 → `.mr-row-main`，与桌面 `RuleItem.tsx:279-283` 同序。
        expect(pri, `${name} 段第 ${i + 1} 行没有序号 —— 判据面塌了`).toBeGreaterThan(-1);
        expect(grip, `${name} 段第 ${i + 1} 行的抓握条摆错了位置`).toBeGreaterThan(pri);
        expect(main, `${name} 段第 ${i + 1} 行的抓握条摆错了位置`).toBeGreaterThan(grip);
      }
    }
    // 反面对照：四颗移动按钮**一颗都没少**（规范要的是「拖拽 **and** 保留四颗」）。
    const sheet = trafficMarkup([bindingOf(ruleOf('a'), { sheetOpen: true })]);
    for (const key of ['rules.moveTop', 'rules.moveUp', 'rules.moveDown', 'rules.moveBottom']) {
      expect(sheet, `${key} 被拖拽顶掉了 —— 规范要的是两件都做`).toContain(key);
    }
  });

  it('源码面：手势的三个 pointer 事件挂在抓握条**那一个**元素上', () => {
    /*
     * 本仓 vitest 是 `environment:'node'`（`vite.config.ts:91`），没有 jsdom ⇒ 驱动不了真手势。
     * 能做的是把「元素在生产 DOM 里」（上一条）与「手柄上挂了哪些事件」（这一条）
     * 各钉一半，中间那段计算由下一条的纯函数覆盖。**真机手感不在本门射程内。**
     */
    const src = strip(read('RuleRow.tsx'));
    const at = src.indexOf('data-drag-handle="1"');
    expect(at, '抓握条不在源码里 —— 判据面塌了').toBeGreaterThan(-1);
    const el = src.slice(src.lastIndexOf('<span', at), src.indexOf('>', at) + 1);
    expect(el.length, '抓握条那个元素没切出来').toBeGreaterThan(40);
    for (const handler of ['onPointerDown', 'onPointerMove', 'onPointerUp', 'onPointerCancel']) {
      expect(el, `抓握条没挂 ${handler} —— 手势会在某一步断掉`).toContain(handler);
    }
    const grip = declsOf('.mr-grip');
    expect(grip, '`.mr-grip` 这条规则不见了 —— 判据面塌了').not.toBe('');
    expect(wonMap('.mr-grip').get('min-size-block')?.value, '抓握条的命中面不是 `--tap-min`').toBe(
      'var(--tap-min)',
    );
  });

  /*
   * 抓握条与**滚动**的关系，两条腿一起钉。
   *
   * 缺陷形态：`.mr-grip` 是 `align-self: stretch` + `min-width: var(--tap-min)` 的一条竖条，
   * 跟满整行高度。给它写 `touch-action: none`，每条规则行的左侧 48px 就成了一块整行高的
   * **滚动死区** —— 十几条规则的列表上，用户在屏幕左侧划不动。而 `touch-action` 在 touchstart
   * 那一刻按命中元素求值、之后不可改 ⇒ 「拖拽时才禁滚动」写成 `.mr-row.dragging .mr-grip` 是
   * **无效**的（求值时机已过），`DRAG_SLOP` 取消长按也救不回来（取消的是拖拽态，浏览器的平移
   * 早在 touchstart 就关掉了）。
   *
   * 所以正确形态是两半：CSS 平时放行纵向平移（`pan-y`），长按**成立之后**由 JS 挂一个
   * **非被动** `touchmove` 监听 + `preventDefault()` 接管。少任何一半这条手势就退化 ——
   * 少 CSS 那半是死区，少 JS 那半是「拖着拖着列表也跟着滚」。故两半各一条断言。
   */
  it('抓握条不是滚动死区：CSS 放行纵向平移，接管由长按成立后的非被动监听做', () => {
    const gripAnywhere = declsAnywhere('.mr-grip');
    expect(gripAnywhere, '`.mr-grip` 这条规则不见了 —— 判据面塌了').not.toBe('');
    expect(
      gripAnywhere,
      '`.mr-grip` 又写回了 `touch-action: none` —— 它跟满整行高，这等于把每条规则行的左侧' +
        '变成一块滚动死区，用户在列表左侧划不动（否定判据吃 `where: ALL`：某一档写回来也算）',
    ).not.toMatch(/touch-action\s*:\s*none/);
    expect(declsOf('.mr-grip'), '抓握条没有放行纵向平移（`touch-action: pan-y`）').toMatch(
      /touch-action\s*:\s*pan-y/,
    );

    // JS 那半：长按成立的那个分支里，必须真的挂上非被动的 touchmove 接管。
    const src = strip(read('RuleRow.tsx'));
    const armed = src.slice(src.indexOf('armTimer.current = setTimeout('), src.indexOf('LONG_PRESS_MS)'));
    expect(armed.length, '长按成立的那个分支没切出来 —— 判据面塌了').toBeGreaterThan(40);
    expect(armed, '长按成立后没有接管平移 —— 拖拽时列表会跟着滚').toMatch(/seizeScroll\(/);

    const seize = src.slice(src.indexOf('const seizeScroll'), src.indexOf('const cancelArm'));
    expect(seize.length, '`seizeScroll` 没切出来 —— 判据面塌了').toBeGreaterThan(100);
    expect(seize, '接管走的不是原生 touchmove').toContain("addEventListener('touchmove'");
    expect(
      seize,
      '监听不是非被动的 —— React 的 onTouchMove 与被动监听里 preventDefault() 都不生效，' +
        '真机上表现为「拖拽时列表还在滚」，且只有一条控制台警告',
    ).toContain('passive: false');
    expect(seize, '接管里没有 preventDefault() —— 挂了监听但什么都没拦').toContain('preventDefault()');
    // 摘钩子：每条结束路径都经过 `cancelArm`，它必须解开接管，否则接管会一直留在那个节点上。
    // 切点自检：`useEffect` 首现在**import 表**里，从文件头找会切出空串（空串对 toContain 恒假，
    // 红的理由与真实缺陷无关）。故从 `cancelArm` 处**向后**找它的下一处。
    const cancelAt = src.indexOf('const cancelArm');
    expect(cancelAt, '`cancelArm` 不在源码里 —— 判据面塌了').toBeGreaterThan(-1);
    const cancel = src.slice(cancelAt, src.indexOf('useEffect(', cancelAt));
    expect(cancel.length, '`cancelArm` 的函数体没切出来').toBeGreaterThan(60);
    expect(cancel, '`cancelArm` 没有解开平移接管 —— 一次长按之后这一行就再也滚不动了').toContain(
      'unseizeScroll()',
    );
  });

  it('落点计算：按兄弟行的真实矩形算，且往下拖不多走一格', () => {
    // 四行等高 100，midpoint 分别是 50 / 150 / 250 / 350。
    const rects = [0, 1, 2, 3].map((i) => ({ top: i * 100, bottom: i * 100 + 100 }));
    // 拖第 0 行到第 2、3 行之间（y=260，落在 C 的下半 / D 的上半）⇒ 目标 2。
    expect(dropIndexAt(rects, 0, 260)).toBe(2);
    // 拖第 3 行到第 0、1 行之间（y=60）⇒ 目标 1。
    expect(dropIndexAt(rects, 3, 60)).toBe(1);
    // 拖到最上面 ⇒ 0；拖到最下面 ⇒ 末位。
    expect(dropIndexAt(rects, 2, -20)).toBe(0);
    expect(dropIndexAt(rects, 0, 999)).toBe(3);
    // 没动 ⇒ 原位（容器据此不发 IPC）。
    expect(dropIndexAt(rects, 1, 160)).toBe(1);
    // 行高不等时仍按各自的矩形算（这正是不能用「行高 × 位移」的原因）。
    const uneven = [
      { top: 0, bottom: 40 },
      { top: 40, bottom: 240 },
      { top: 240, bottom: 280 },
    ];
    expect(dropIndexAt(uneven, 0, 100)).toBe(0);
    expect(dropIndexAt(uneven, 0, 150)).toBe(1);
  });

  /*
   * 落点算出来之后**有没有真的提交** —— ⑫ 组此前缺的正是这一环。
   *
   * 其余四段各钉一半：渲染面钉 grip 在场与位置、源码面钉 pointer handler 挂在 grip 上、
   * 纯函数面钉 `dropIndexAt`、写路径面钉 `handleReorder` 里出现 `commitOrder`。
   * 中间「`onGripUp` 把 `dropIndexAt` 的结果原样交给 `onReorder`」没有任何断言 ⇒
   * 把那一句改成 `void to;`（落点照算，永不提交）全量测试仍然全绿，只有 tsc 因为
   * `onReorder` 变成未使用形参才**偶然**报 TS6133 —— 那是巧合不是设计：改成
   * `onReorder(rule, 0)`（永远拖到置顶）`onReorder` 仍被使用，tsc 与 vitest 双绿，
   * 而真机上每次拖拽都落错格。
   */
  it('接线：`onGripUp` 把 `dropIndexAt` 的结果原样交给 `onReorder`，中间不换值', () => {
    const src = strip(read('RuleRow.tsx'));
    const at = src.indexOf('const onGripUp');
    expect(at, '`onGripUp` 不在源码里 —— 判据面塌了').toBeGreaterThan(-1);
    const body = src.slice(at, src.indexOf('const onGripCancel', at));
    expect(body.length, '`onGripUp` 的函数体没切出来').toBeGreaterThan(200);

    expect(body, '`onGripUp` 里没有算落点 —— 判据面塌了').toContain('const to = dropIndexAt(');
    expect(
      body,
      '落点算完之后没有交给 `onReorder(rule, to)` —— 拖拽在屏幕上动了，顺序一条都没改',
    ).toContain('onReorder(rule, to)');
    // 唯一性：整个 handler 里 `onReorder(` 只许出现这一次。多一处 = 有另一条不走落点的提交路径，
    // 少一处（含改成常量实参）= 上一条断言已经红。
    expect(
      [...body.matchAll(/onReorder\(/g)].length,
      '`onGripUp` 里有不止一处 `onReorder(` —— 落点与提交之间被插了别的值',
    ).toBe(1);
  });

  it('写路径：拖拽与四颗按钮都落到 `commitOrder`，不是各写各的顺序', () => {
    const src = strip(read('RulesScreen.tsx'));
    const reorder = src.slice(src.indexOf('const handleReorder'), src.indexOf('const handleToggle'));
    expect(reorder.length, '`handleReorder` 没切出来 —— 判据面塌了').toBeGreaterThan(200);
    expect(reorder, '拖拽绕过了 `commitOrder`（暂存 / 乐观回滚 / 失败回显都在那里面）').toContain(
      'commitOrder(plane, ordered)',
    );
    // 越界不许写：落点被夹在 [0, len-1] 内，且原位不发 IPC。
    expect(reorder).toContain('Math.min(Math.max(toIndex, 0), list.length - 1)');
    expect(reorder).toContain('if (target === from) return;');
    expect(src, '行没有把落点接到容器的写路径上').toContain('onReorder: (r, toIndex) =>');
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑬ 断点档位的**值**：expanded 的行高与列间距真的在场（不是死值，也不是没写）。
 *
 * 与 `styles/cascade-dead-rules.test.ts` 分工：那道门是全仓的，只判「断点块有没有机会胜出」
 * （源码顺序），判不了「值还在不在」—— 把整条 expanded 规则删掉，它照样绿。本组补另一半：
 * 值在场 + 顺序正确，两条合起来才等于「平板上真的量得到 64px」。
 *
 * 此前这一层一条门都没有：把 K4-08 那批改动整条退回基点写法，1126 个测试全绿。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑬ rules-screen.css 的断点档位落地（layout.responsive 的 rowMinHeight / columnGap）', () => {
  /** 剥掉块注释再取材：注释里的示例规则不算数。**保长度** ⇒ 偏移量比较仍对得上原文。 */
  const cssOf = (): string => CSS.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '));

  /**
   * 同一个条件的容器查询块在本文件里**有好几个**（卡片面、开关行各写各的），
   * 所以这里收**全部**，不是首条命中即返回 —— 「取首条」正是 `declsOf` 上一版抓不到层叠的原因。
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
    expect(out.length, `rules-screen.css 里没有 ${em}em 的容器查询块 —— 取材面塌了`).toBeGreaterThan(0);
    return out;
  };

  const blocksDeclaring = (css: string, em: string, rule: RegExp): { at: number; body: string }[] =>
    containerBlocks(css, em).filter((b) => rule.test(b.body));

  it('开关行行高：基线 54，expanded 64，且 64 那条写在基线**之后**（否则是死值）', () => {
    const css = cssOf();
    const base = /\.mr-switch-row\s*\{[^}]*min-height:\s*54px/.exec(css);
    expect(base, '`.mr-switch-row` 的基线行高 54 不见了').not.toBeNull();

    const carriers = blocksDeclaring(css, '52\\.5', /\.mr-switch-row\s*\{[^}]*min-height:\s*64px/);
    expect(carriers.length, 'expanded 档没有 `.mr-switch-row` 的行高覆写 —— 三档只落地了两档').toBe(1);
    // `@container` 不贡献特异性 ⇒ 同名同特异性只按源码顺序决胜。写在基线之前就是死值。
    expect(
      carriers[0].at,
      'expanded 的 64px 块写在了基线 54px 之前 —— 层叠上永远输，平板量到的还是 54',
    ).toBeGreaterThan(base?.index ?? Number.POSITIVE_INFINITY);
  });

  it('卡片列间距两档：medium 32 / expanded 40（`layout.responsive.columnGap`）', () => {
    const css = cssOf();
    expect(blocksDeclaring(css, '37\\.5', /\.mr-cards\s*\{[^}]*column-gap:\s*32px/).length).toBe(1);
    expect(blocksDeclaring(css, '52\\.5', /\.mr-cards\s*\{[^}]*column-gap:\s*40px/).length).toBe(1);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑭ 字形出处：移动端画的每一个图元都必须逐字来自桌面（kit 的 icon-registry 政策落成代码）。
 *
 * # 为什么需要这条门
 *
 * kit 的 `icons/icon-registry.json` 是那张唯一的白名单，政策写着
 * `reuse-current-client-only` + `allowRedesign: false` + 「缺图标就省掉或申请一个已批准的来源，
 * **绝不许自己发明**」。但那份文件住在 `~/docs/polaris/design/mobile-kit/` —— 不在这个仓里，
 * 不进 CI，对执行**没有任何强制力**。往里补一行登记只是把纸面补齐；真正会咬人的形态是
 * 「移动端画了一个桌面根本没有的字形」，而那件事只有仓内的判据拦得住。
 *
 * 所以分工是：registry 记「这一格叫什么、出处是谁」，本门守「画出来的东西确实是那个出处的东西」。
 * 本条正是 K4-07 那一半（`action.drag-handle` 未登记）暴露出来的缺口 —— 登记补了，
 * 但如果没有这条门，下一个人照样可以随手画一个新字形，五份 locale 与桌面都不会有任何反应。
 *
 * # 判据面
 *
 * 扫本批三个屏的全部非测试 `.tsx`，取出每一个 SVG 图元（`path`/`circle`/`rect`/`line`/
 * `polyline`/`polygon`/`ellipse`）的**几何属性**（`d`/`cx`/`cy`/`r`/`points`/…，属性名排序后拼接，
 * 与书写顺序无关），断言每一个都能在桌面 `src/components/**` 里找到逐字相同的一个。
 *
 * 视觉属性（`stroke-width` / `fill` / `class`）**不进判据**：它们由主题与上下文决定，
 * 同一个字形在两端合法地取不同线宽。判的是「形状是不是同一个」。
 *
 * ## 盖不住什么
 *
 * 桌面自己发明的字形本门放行（它比对的是两端一致，不是某个绝对来源）；
 * 桌面已有而移动端**漏画**的那一格也不在射程内 —— 那是各屏自己的在场判据的事。
 * ═════════════════════════════════════════════════════════════════════════ */

describe('⑭ 移动端字形逐字来自桌面（icon-registry 的 allowRedesign:false 落成代码）', () => {
  const UI_SRC = join(HERE, '..', '..', '..');

  const walkTsx = (dir: string): string[] => {
    const out: string[] = [];
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      const p = join(dir, e.name);
      if (e.isDirectory()) out.push(...walkTsx(p));
      // 共享谓词（`contracts/test-only-modules.ts` 头注：三道门需要同一个概念，不许各留一份拷贝）。
      // `.test-support.` 同样不进产物，且产品代码不许 import 它们（`i18n-coverage` G0-b 锁着）——
      // 把它们留在产品面上，判据会被别的判据的**锚文本**喂饱（2026-09-06 在 app-wiring ⑩/⑫ 实测过一次假绿）。
      else if (/\.tsx?$/.test(e.name) && !IS_TEST_ONLY_MODULE.test(e.name)) out.push(p);
    }
    return out;
  };

  /**
   * 一个 SVG 图元的几何指纹：`tag|属性=值|…`（属性名排序 ⇒ 与书写顺序无关）。
   *
   * ⚠️ **只认字面量**：几何属性写成 JS 表达式（`d={d}` / `d={ICONS[k]}` / `width={NODE_WIDTH}`）时
   * 一个属性都取不到，指纹退化成裸的 `path|` / `rect|`。这一格此前是 **fail-open** ——
   * 桌面侧同样有 8 处表达式写法，于是 `desktopGlyphs` 里坐着一个裸 `path|`，
   * 移动端任何 `<path d={任意表达式} />` 都能被它匹配上，**凭空发明的字形照过**
   * （2026-09-05 实测：tsc rc=0 / vitest rc=0 / 84 passed）。本批屏里 `Primitives.tsx` 的
   * `MoveIcon` 四枚字形正走这条路，从来没被比对过。
   *
   * 现在改成三条一起：取不到几何属性的图元收进 `unfingerprintable` 逼作者显式改判；
   * 桌面白名单剔掉裸指纹；正比对里裸指纹一律算 orphan。
   */
  const GEOMETRY_ATTRS = /\b(d|cx|cy|r|rx|ry|x|y|x1|y1|x2|y2|width|height|points)=\{?["']([^"']*)["']\}?/g;
  /** 裸指纹（`tag|`，一个几何属性都没取到）。 */
  const IS_BARE = /^\w+\|$/;
  const fingerprints = (source: string): string[] => {
    const body = strip(source);
    const out: string[] = [];
    for (const m of body.matchAll(/<(path|circle|rect|line|polyline|polygon|ellipse)\b([^>]*?)\/?>/g)) {
      const attrs = [...m[2].matchAll(GEOMETRY_ATTRS)]
        .map((a) => `${a[1]}=${a[2].replace(/\s+/g, ' ').trim()}`)
        .sort();
      out.push(`${m[1]}|${attrs.join('|')}`);
    }
    return out;
  };

  /** 本批三个屏 = 本条判据的作用面（首页 / 设置由各自那条线看住）。 */
  const MOBILE_DIRS = ['nodes', 'connections', 'screens/rules'].map((d) => join(UI_SRC, 'mobile', d));

  const mobileGlyphs = new Map<string, string[]>();
  for (const dir of MOBILE_DIRS)
    for (const f of walkTsx(dir))
      for (const fp of fingerprints(readFileSync(f, 'utf8'))) {
        if (!mobileGlyphs.has(fp)) mobileGlyphs.set(fp, []);
        mobileGlyphs.get(fp)?.push(f.slice(UI_SRC.length + 1));
      }

  /**
   * 桌面白名单**剔掉裸指纹**：一个裸 `path|` 坐在这里，等于给移动端所有表达式写法开一张免死金牌。
   * 桌面自己那 8 处表达式写法（拓扑图的连线、`TARGET_ICON_PATHS[kind]` …）不在本门射程内 ——
   * 本门守的是「移动端画的东西来自桌面」，不是「桌面全都可静态比对」。
   */
  const desktopGlyphs = new Set(
    walkTsx(join(UI_SRC, 'components'))
      .flatMap((f) => fingerprints(readFileSync(f, 'utf8')))
      .filter((fp) => !IS_BARE.test(fp)),
  );

  it('取材面自检：两侧都真的取到了图元，且本批两个新字形在取材面内', () => {
    expect(mobileGlyphs.size, '移动端一个 SVG 图元都没取到 —— 下面那条恒绿').toBeGreaterThan(20);
    expect(desktopGlyphs.size, '桌面一个 SVG 图元都没取到 —— 下面那条会把全部字形误判成越界').toBeGreaterThan(100);
    // 抓握条（`action.drag-handle`）与选中勾：K4-07 那半条漏登记就是这两格，必须真在面内。
    expect([...mobileGlyphs.keys()], '抓握条的圆点没进取材面').toContain('circle|cx=9|cy=6|r=1');
    expect([...mobileGlyphs.keys()], '选中勾没进取材面').toContain('path|d=M5 12l5 5 9-11');
  });

  it('反向对照：凭空发明一个字形会被抓出来（证明上一条不是恒绿）', () => {
    const invented = fingerprints('<svg><path d="M1 1 L23 23" /></svg>');
    expect(invented).toEqual(['path|d=M1 1 L23 23']);
    expect(desktopGlyphs.has(invented[0]), '这条合成路径居然在桌面存在 —— 换一条再做对照').toBe(false);
    // 🔴 fail-open 那一格的对照：几何属性写成表达式时指纹是**裸的**，而裸指纹不许再当成「匹配上了」。
    expect(fingerprints('<svg><path d={INVENTED_D} /></svg>')).toEqual(['path|']);
    expect(IS_BARE.test('path|')).toBe(true);
    expect(IS_BARE.test('path|d=M1 1'), '判据把有指纹的图元也当成裸的了').toBe(false);
    expect(desktopGlyphs.has('path|'), '桌面白名单里还坐着一个裸 `path|` —— 它给一切表达式写法开免死金牌').toBe(
      false,
    );
  });

  it('🔴 移动端没有取不到几何指纹的图元（取不到 ⇒ 这枚字形从来没被比对过）', () => {
    const unfingerprintable = [...mobileGlyphs.entries()]
      .filter(([fp]) => IS_BARE.test(fp))
      .map(([fp, files]) => `${fp}  ←  ${[...new Set(files)].join(', ')}`);
    expect(
      unfingerprintable,
      '这些图元的几何属性写成了 JS 表达式（`d={d}` 之类），静态面上取不到指纹 ⇒ 出处比对对它们' +
        '**一个字都没说**。要么把几何写成字面量（`Primitives.tsx` 的 `MoveIcon` 是同仓先例：' +
        '四条 path 逐字写在 JSX 里，渲染结果不变），要么显式说明它为什么不需要出处比对。',
    ).toEqual([]);
  });

  it('三个屏画的每一个图元都能在桌面找到逐字相同的一个', () => {
    // 裸指纹（几何写成表达式）一律算 orphan —— 上一条已经点名报过它们，这里不许它们从
    // `desktopGlyphs.has()` 那条腿上溜回来。
    const orphans = [...mobileGlyphs.entries()]
      .filter(([fp]) => IS_BARE.test(fp) || !desktopGlyphs.has(fp))
      .map(([fp, files]) => `${fp}  ←  ${[...new Set(files)].join(', ')}`);
    expect(
      orphans,
      '这些图元在桌面 `src/components/**` 里找不到逐字相同的来源 ⇒ 要么是自己发明的字形' +
        '（kit 政策 `allowRedesign: false` 明禁），要么是照抄时改了坐标。' +
        '两者在 kit 的 icon-registry 里都没有对应的白名单条目。',
    ).toEqual([]);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑮ **浏览器裁判**：两条否定判据由 Chrome 对真 DOM 算。
 *
 * ⑩ 与 ⑫ 那两条否定判据（`.mr-ap-body.dim` 不许变得点不动 / `.mr-grip` 不许成为滚动死区）
 * 此前读的是「本选择器自己的声明面」。2026-09-05 第四轮实测它们对三种真回归全绿：
 *  · R5-02 —— `.mr-screen .mr-ap-body.dim{ pointer-events: none }`：更窄的规则**新增**一个
 *    本选择器没写过的属性。静态模型的 narrower 桶只报「与本选择器重叠的分量」，整类排除。
 *  · R5-03 —— `.mr-ap-body{ pointer-events: none }`：更宽且非 important 被无条件丢弃，
 *    而 `.mr-ap-body.dim` 自己压根没声明这个属性 ⇒ 浏览器里生效的就是更宽那条，
 *    应用段正文**无论置不置灰都点不动**。
 *  · R5-05 —— `:is(.mr-ap-body, …){ … !important }`：`isBroader` 是纯语法超集判定，包一层就溜过去。
 * 三种都只有一个共同的裁判能一次答完：把真 DOM 与整条 CSS 链交给浏览器，问它 computed 值。
 *
 * **祖先链由真屏提供**：`.mr-screen` 是 `RulesScreen.tsx:1118` 的屏根，段体渲染在它里面。
 * `MobileRulesScreen` 无 props、`useState` 默认停在 traffic 档（⑪ 头注同一条理由），
 * 故这里把段体注进真屏的 `.mr-screen` 里，并由下面的自检钉住「注进去了、且屏根确实是 `.mr-screen`」。
 * ═══════════════════════════════════════════════════════════════════════════ */
describe('⑮ 浏览器裁判：置灰正文仍可点、抓握条仍可滚（真 DOM + 整条移动端 CSS 链）', () => {
  // 收尾走 CDP 的 `Browser.close`，让 Chrome 自己退（全程不 kill 进程）。
  // 超时给到 30s：全量并行跑时机器争用，默认的 10s hookTimeout 会在收尾这一步假红。
  afterAll(closeOracle, 30_000);

  /** 真屏外壳（`.mr-screen` + 屏头 + 分段条），段体注在它的收口之前。 */
  const inScreen = (segmentHtml: string): string => {
    const shell = renderToStaticMarkup(<MobileRulesScreen />);
    const at = shell.lastIndexOf('</section>');
    if (at < 0) throw new Error('真屏没有 `</section>` 收口 —— 注入切法失效');
    return shell.slice(0, at) + segmentHtml + shell.slice(at);
  };

  const paint = (segmentHtml: string, asks: Parameters<typeof measure>[1]) =>
    measure(
      {
        ctx: 'mobile',
        // 真挂载树（第五轮 B01）：屏根之上还有 `#root > .m-shell > .m-scroll > .m-page`。
        html: inMobileShell(inScreen(segmentHtml), 'rules'),
        rootAttrs: { 'data-theme': 'dark' },
        viewport: { width: 390, height: 844 },
      },
      asks,
    );

  it('自检：真屏根是 `.mr-screen`，段体确实注进了它里面（祖先链在场）', async () => {
    const shell = renderToStaticMarkup(<MobileRulesScreen />);
    expect(shell, '真屏根不是 `.mr-screen` —— 下面几条的祖先链前提垮了').toContain(
      '<section class="mr-screen"',
    );
    const m = await paint(appsMarkup(undefined, { masterEnabled: false, isSmartMode: false }), [
      { select: '.mr-screen .mr-ap-body.dim', props: ['pointer-events'] },
    ]);
    expect(
      m.count('.mr-screen .mr-ap-body.dim'),
      '置灰正文没落在 `.mr-screen` 里 —— R5-02 那条变异的前提就不成立，这一组的射程要重写',
    ).toBeGreaterThan(0);
  }, 30_000);

  it('置灰只降透明度，不动可交互性：浏览器算出来仍然可点、可选、可见', async () => {
    const dimAsks = [
      {
        select: '.mr-ap-body.dim',
        props: ['pointer-events', 'user-select', 'visibility', 'display', 'opacity'] as const,
      },
    ];
    const m = await paint(appsMarkup(undefined, { masterEnabled: false, isSmartMode: false }), dimAsks);
    expect(
      m.get('.mr-ap-body.dim', 'pointer-events'),
      '置灰顺手把正文变成不可点了 —— 规范明写 controls stay editable。' +
        '（更宽的 `.mr-ap-body` 供着这个属性、或更窄的 `.mr-screen .mr-ap-body.dim` 新增它，' +
        '静态模型两种都看不见。）',
    ).toBe('auto');
    expect(m.get('.mr-ap-body.dim', 'user-select'), '置灰把正文变成不可选了').not.toBe('none');
    expect(m.get('.mr-ap-body.dim', 'visibility'), '置灰把正文藏起来了').toBe('visible');
    expect(m.get('.mr-ap-body.dim', 'display'), '置灰把正文从布局里摘了').not.toBe('none');
    // 正面：它**真的**在降透明度（只写否定判据会被「什么都没发生」骗过）。
    const dimmed = Number(m.get('.mr-ap-body.dim', 'opacity'));
    expect(dimmed, '置灰档的 opacity 不在 (0,1) 内 —— 要么没灰，要么灰到看不见').toBeGreaterThan(0);
    expect(dimmed).toBeLessThan(1);
    // 反向对照：两者都开时不灰 —— 上面那条不是「恒灰」。
    const on = await paint(appsMarkup(undefined, { masterEnabled: true, isSmartMode: true }), [
      { select: '.mr-ap-body', props: ['opacity', 'pointer-events'] },
    ]);
    expect(Number(on.get('.mr-ap-body', 'opacity')), '两者都开时正文仍是灰的').toBe(1);
    expect(on.get('.mr-ap-body', 'pointer-events')).toBe('auto');
  }, 30_000);

  it('抓握条不是滚动死区：浏览器算出来的 `touch-action` 放行纵向平移（逐条，不是第一条）', async () => {
    // traffic 夹具下 `.mr-grip` 有三条。上一版只读第一条，而紧挨着的 `count(...) > 0` 让人读成
    // 「这一族都查了」—— 第三条改坏门照绿（第五轮 M02）。`many: 'all'` ⇒ 断言吃全集。
    const m = await paint(trafficMarkup(), [
      { select: '.mr-grip', props: ['touch-action'], many: 'all' },
    ]);
    expect(m.count('.mr-grip'), '抓握条没渲染出来 —— 判据面塌了').toBeGreaterThan(0);
    const seen = m.getAll('.mr-grip', 'touch-action');
    expect(seen.length, '一条抓握条都没量到 ⇒ 下面这条恒真').toBe(m.count('.mr-grip'));
    for (const [i, v] of seen.entries())
      expect(
        v,
        `第 ${i + 1} 条 \`.mr-grip\` 算出来不是 \`pan-y\` —— 它跟满整行高，\`none\` 等于把每条规则行的左侧变成` +
          '一块整行高的滚动死区，用户在列表左侧划不动。' +
          '（`touch-action` 在 touchstart 那一刻按命中元素求值、之后不可改。）',
      ).toBe('pan-y');
  }, 30_000);
});
