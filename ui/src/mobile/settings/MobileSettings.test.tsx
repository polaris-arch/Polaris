/**
 * 移动端「设置」屏的判据（F4）。
 *
 * # 判据面：**渲染出来的 DOM** 为主，源码扫描为辅
 *
 * 本仓 vitest 跑 node 环境、刻意不装 jsdom，但 `renderToStaticMarkup` 不需要 jsdom
 * （同 `i18n/i18n-coverage.test.ts` 的 G5c）。选它当主判据面的理由是**射程**：
 * 「模型里没登记这颗开关」挡不住有人直接往 JSX 里塞一个 `<MobileSwitch>`，而
 * 「渲染出来的这一页上带 `role="switch"` 的行 id 集合**恰好等于**这一串」两种写法都挡得住。
 *
 * # 每一组都带正面断言，不只有「不许出现 X」
 *
 * 纯否定式判据会被「什么都没渲染出来」骗过：组件抛错、fixture 不合法、正则敲错一个字母，
 * 全都让「不含某某」一路绿灯。故每组的形态是**集合逐值全等**（下面那些 `EXPECTED_SWITCHES`），
 * 外加 ⓪ 一组自检（取材面非空、量级合理）。
 *
 * # 判据不能被自己污染
 *
 * 下面写了 `strictRoute` / `allowLan` / 「允许局域网」这些**要断言其不存在**的字面量。
 * 若取材面把本文件也扫进去，删掉生产代码里的违规项之后判据照样红（或者更糟：加回违规项也不红）。
 * 故 `sourceFace()` 显式排除 `.test.`，并由 ⓪ 一组正面断言「本文件不在取材面里」。
 * 渲染那一侧天然免疫：markup 来自组件，本文件的字符串进不去。
 *
 * # 抓不到什么（如实标注）
 *
 *  · **视觉**：颜色、间距、触控目标的实际像素要真机看，本批看不了（模拟器归 K5 线）。
 *  · **交互**：点击后的状态迁移需要 jsdom/真机；这里只看首帧静态输出。
 *  · **iOS**：本批 iOS 不在射程内，关于页的产物许可按 Android 写死，判据也只对 Android 口径。
 */

import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { IS_TEST_ONLY_MODULE } from '@/contracts/test-only-modules';
import { ALL, context, declaringSites } from '@/styles/css-cascade.test-support';
import { createElement, type ReactElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import i18next, { type i18n as I18n } from 'i18next';
import { I18nextProvider, initReactI18next } from 'react-i18next';

import type { DnsServerGroup, DnsServerResource, UserConfig } from '@/contracts/types';
import type { CselGroup } from '@/components/dialogs/csel-logic';
import { buildDnsActionGroups } from '@/components/dialogs/dns-action-options';
import { SelectSheetPanel } from '../screens/rules/Primitives';
import { BACKUP_CATEGORIES } from '@/domain/backup-categories';
import {
  domesticPresets,
  needsFakeIpOffConfirm,
  remotePresets,
} from '@/components/screens/settings/settings-dns-logic';
import zhCN from '@/i18n/locales/zh-CN.json';

import { maskRustComments } from '@/contracts/rust-source.test-support';
import { AboutPage, narrowVersionInfo } from './AboutPage';
import { BackupPage } from './BackupPage';
import { DisplayPage } from './DisplayPage';
import { collapsibleGroups, DnsPage, openGroupOf } from './DnsPage';
import { GeneralPage } from './GeneralPage';
import { NetworkPage } from './NetworkPage';
import { MobileSettingsScreen, SettingsRoot, watchVpnAuth, type VisibilityTarget } from './MobileSettingsScreen';
import { MobileShell, setPushedPage } from '../MobileShell';
import { SettingsHeader } from './SettingsChrome';
import { useAppStore } from '@/store/app-store';
import type { VpnAuthState } from '@/ipc/api/vpn';
import { updateApi } from '@/ipc/api-client';
import { markAppVersionSkipped } from '@/components/layout/app-update-banner';
import * as ts from '@/test/ts-compiler';
import { createCommit, WriteErrorsContext, type WriteErrors } from './write-feedback';
import { TunPage } from './TunPage';
import { UpdatePage } from './UpdatePage';
import {
  openSystemVpnSettings,
  VPN_SETTINGS_BRIDGE,
  VPN_SETTINGS_FAILED,
  VPN_SETTINGS_OK,
  VPN_SETTINGS_OPEN,
} from './system-settings-bridge';
import {
  appUpdateEntryVersion,
  appUpdateHint,
  checkAppUpdateViaIpc,
  readAppUpdateCheck,
  resetAppUpdateCheck,
  runAppUpdateCheck,
  runAutoAppUpdateCheck,
} from './app-update-check';
import {
  MOBILE_ABSENT_SETTINGS_PAGE,
  MOBILE_SETTINGS_PAGES,
  type MobileSettingsPageId,
} from './settings-pages';

const HERE = dirname(fileURLToPath(import.meta.url));
const UI_SRC = resolve(HERE, '../../');
const REPO = resolve(UI_SRC, '../../');
const LOCALES = ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa'] as const;

/**
 * 剥注释。**这一步不是洁癖，是判据的正确性**：本屏几乎每个文件的头注都在解释「为什么不接
 * `setPrivacyMode`」「为什么不用 `InfoIcon`」——不剥就会把这些**反面例子**当成真实调用抓进来，
 * 于是判据对着自己的文档说明报红（首跑实测：⑦ 与 ⑧ 各红一条，红在两句注释上）。
 *
 * **必须识别字符串**，不能用 `/\/\*[\s\S]*?\*\//g` 那种正则：本仓 `mobile-entry.test.ts` 记过一次
 * 真实事故 —— 一个同时含 `*​/` 与 `/​*` 的 glob 会被正则当成块注释起点，一路吃到文件末尾。
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

/** 本屏的**源码**取材面：本目录下的产品代码，显式排除测试自身（见头注「判据不能被自己污染」）。 */
function sourceFace(): { file: string; text: string }[] {
  return readdirSync(HERE)
    // 共享谓词（`contracts/test-only-modules.ts`）：`.test-support.` 也不是产品代码。
    // 本目录里那张四屏对差登记表按名字点了 `allowLan` / `bypassLAN` 等一串**移动端不提供**的能力，
    // 那是登记不是接线（它是一个 `ScreenParityRegister` 常量，接不了任何东西）。
    .filter((n) => /\.tsx?$/.test(n) && !IS_TEST_ONLY_MODULE.test(n))
    .sort()
    .map((n) => ({ file: n, text: stripComments(readFileSync(join(HERE, n), 'utf8')) }));
}

const SOURCES = sourceFace();
const SOURCE_TEXT = SOURCES.map((s) => s.text).join('\n');

/** zh-CN 里按点分路径取一条文案。取不到抛错 —— 断言不该悄悄拿 undefined 去比。 */
function copy(key: string): string {
  const value = key
    .split('.')
    .reduce<unknown>((cur, seg) => (cur as Record<string, unknown> | undefined)?.[seg], zhCN);
  if (typeof value !== 'string') throw new Error(`zh-CN 里没有这条文案：${key}`);
  return value;
}

function localeJson(lang: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(UI_SRC, 'i18n/locales', `${lang}.json`), 'utf8')) as Record<
    string,
    unknown
  >;
}

function leaf(tree: Record<string, unknown>, key: string): unknown {
  return key.split('.').reduce<unknown>((cur, seg) => (cur as Record<string, unknown> | undefined)?.[seg], tree);
}

/* ── 渲染 ─────────────────────────────────────────────────────────────────── */

/**
 * 判据用配置。取值刻意**把条件行全部打开**（隐私锁开 ⇒ 密码行在；schema v1 ⇒ 非 v2 的 DNS 三行在；
 * 单上游档 ⇒ 那颗下拉在），否则「集合全等」会因为半数行没渲染而变成一条弱得多的断言。
 *
 * ⚠️ `proxyModeType` 这一项是**反向探针**，取的是全新安装 Android 上盘里真正的那个值
 * （`crates/config-engine/src/user_config/app_config.rs` 的默认值 `SystemProxy`），不是 `'tun'`。
 * Android 恒 TUN、且移动端已经没有任何控件能改这个字段（首页那颗接管方式芯片已整颗移除）⇒
 * **移动端设置页一处都不许再从它取分支**；这里喂进那个「错的」值，⑪ 组据此断言 TUN 专属的
 * 那几行照样渲染。改回 `'tun'` 会让 ⑪ 组退化成恒绿。
 */
const CONFIG = {
  autoConnect: true,
  autoCheckUpdate: true,
  autoPrivacyMode: true, // 桌面配置里可能是开的；移动端**不该**因此长出任何控件（⑦）
  disableLogFile: false,
  language: 'auto',
  uiTheme: 'system',
  desktopNotifications: true,
  proxyModeType: 'systemProxy',
  configSchemaVersion: 1,
  blockQuic: false,
  tlsFragment: false,
  autoSwitchNode: false,
  restartOnNodeChange: false,
  enableIPv6: true,
  speedTestUrl: '',
  // 非预设值 ⇒ gh 下拉落 `custom` ⇒ 自定义域名那一行也渲染，它的写腿才测得到。
  ghProxyPrefix: 'https://cdn.example/',
  subscriptionUpdateIntervalHours: 12,
  ruleResourceUpdateIntervalHours: 12,
  tunConfig: { stack: 'auto', autoRoute: true, strictRoute: true },
  dnsConfig: {
    domesticDns: '223.5.5.5',
    foreignDns: 'https://1.1.1.1/dns-query',
    enableFakeIp: true,
    resolveNodeDomainsAhead: false,
  },
} as unknown as UserConfig;

let i18n: I18n;

beforeAll(async () => {
  // 自建实例而不是 import `@/i18n`：那个模块在 import 期就要读 localStorage / navigator 并挑语言，
  // node 环境下那条链路不是本门要验的东西。判据要的只是「文案能解析成 zh-CN 的原文」。
  i18n = i18next.createInstance();
  await i18n.use(initReactI18next).init({
    lng: 'zh-CN',
    fallbackLng: 'zh-CN',
    resources: { 'zh-CN': { translation: zhCN } },
    interpolation: { escapeValue: false },
  });
});

function render(node: ReactElement, writeErrors: WriteErrors = {}): string {
  return renderToStaticMarkup(
    createElement(
      I18nextProvider,
      { i18n },
      createElement(WriteErrorsContext.Provider, { value: writeErrors }, node),
    ),
  );
}

/** 二级页 id → 文件名（判据要按文件读源码）。 */
const pageFile = (id: string): string =>
  `${id.charAt(0).toUpperCase()}${id.slice(1)}Page.tsx`.replace('DnsPage', 'DnsPage');

const pageOf = (id: MobileSettingsPageId): ReactElement => {
  const props = { config: CONFIG, update: async () => undefined, commit: () => undefined };
  switch (id) {
    case 'general':
      return createElement(GeneralPage, props);
    case 'display':
      return createElement(DisplayPage, props);
    case 'network':
      return createElement(NetworkPage, props);
    case 'dns':
      return createElement(DnsPage, props);
    case 'tun':
      return createElement(TunPage, props);
    case 'update':
      return createElement(UpdatePage, props);
    case 'backup':
      return createElement(BackupPage, props);
    case 'about':
      return createElement(AboutPage, props);
  }
};

/**
 * DNS 页的**第二份夹具**：按 dnsConfig / 顶层字段各打一个补丁再渲染。
 *
 * 存在的理由是那一页有一处**互斥分档** —— race 关时露出单上游下拉，race 开时露出竞速池三颗开关，
 * 一份夹具只照得到一半。凡是「逐行都要成立」的判据（⑩ 的失败回显、⑫ 的池状态）都必须把两档
 * 都渲染一次，否则永远渲染不到的那几行会被读成「这几行没接」。
 */
const dnsMarkup = (
  dnsPatch: Record<string, unknown> = {},
  topPatch: Record<string, unknown> = {},
  writeErrors: WriteErrors = {},
): string =>
  render(
    createElement(DnsPage, {
      config: {
        ...CONFIG,
        ...topPatch,
        dnsConfig: { ...(CONFIG as unknown as { dnsConfig: object }).dnsConfig, ...dnsPatch },
      } as unknown as UserConfig,
      update: async () => undefined,
      commit: () => undefined,
    }),
    writeErrors,
  );

/**
 * DNS 页的**第三份夹具口径**：v2 默认策略那一档（`configSchemaVersion >= 2 && dnsDefaults != null`）。
 *
 * `CONFIG` 固定停在 schema v1（那是「远程/国内 DNS 三行 + FakeIP 开关」那一档的前提），故 v2 下
 * 才出现的两格必须另喂一份 `topPatch`。
 *
 * 未命中动作取 `hostsFirst` 而不是更常见的 `fakeIp`：兜底那一行（`dns-default-fallback`）**只在
 * 这一档渲染**，取别的档会让它永远渲染不到，而「每一行都有失败回显」在那一格上就成了空话。
 * 服务器与组各给一条真的（不是空表）：候选表里没有它们时，当前档会落到 `missingOption` 的
 * 禁用行上 —— 那是另一种形态，不该被当成常态判据。
 */
const DNS_V2_TOP: Record<string, unknown> = {
  configSchemaVersion: 2,
  dnsDefaults: {
    directServerId: 'builtin-domestic',
    proxyServerId: 'builtin-remote',
    unmatchedAction: {
      type: 'hostsFirst',
      hostsServerId: 'sys-hosts',
      fallback: { type: 'server', serverId: 'builtin-domestic' },
    },
  },
  dnsServers: [
    { id: 'builtin-domestic', name: '', enabled: true, type: 'https', endpoint: { host: '223.5.5.5' }, outbound: { type: 'direct' } },
    { id: 'builtin-remote', name: '', enabled: true, type: 'https', endpoint: { host: '1.1.1.1' }, outbound: { type: 'currentExit' } },
    { id: 'sys-hosts', name: 'Hosts', enabled: true, type: 'hosts', outbound: { type: 'direct' } },
  ],
  dnsServerGroups: [
    { id: 'grp-race', name: '竞速组', enabled: true, mode: 'race', members: ['builtin-domestic', 'builtin-remote'] },
  ],
};

/** 逐页 markup（模块级求值一次；任何一页渲染抛错都会在这里当场炸，不会退化成空跑）。 */
const MARKUP: Record<MobileSettingsPageId, string> = {} as Record<MobileSettingsPageId, string>;
let ROOT_MARKUP = '';
/**
 * DNS 页 race **开**的那一档（`MARKUP.dns` 是 race 关那一档，两者互斥）。
 *
 * 凡是「逐行/逐颗都要成立」的判据都必须把两档都渲染一次 —— 这是 `dnsMarkup` 头注写下的口径，
 * ⑫ 的失败回显那条一直照做，而 ⑬-3（每一颗开关的 48 命中盒）与 ⑯（每一行的行高/盒模型）
 * 此前只吃 `allMarkup() + ROOT_MARKUP`：**竞速池那三行连同它们的三颗开关，永远渲染不到**，
 * 于是那三格上「每一颗都成立」是一句空话（2026-09-05 复核实测）。
 */
let DNS_RACE_MARKUP = '';

beforeAll(() => {
  for (const id of MOBILE_SETTINGS_PAGES) MARKUP[id] = render(pageOf(id));
  DNS_RACE_MARKUP = dnsMarkup({ resolveNodeDomainsAhead: true });
  // `vpnAuth` 由屏组件读来（`useVpnAuthState`），根页只是渲染它。判据面固定喂 `unknown`：
  // 那是「读不到」，也是本文件下面那条「不许编一个已授权」断言要盯的那一支。
  // `appUpdate: null` = 没查到新版本（本会话的缺省态）⇒ 根页只有那八行进入行 + VPN 授权行。
  // W-20 那一行由 ⑱ 组另喂一份 `appUpdate` 渲染，与这份基线互不干扰。
  ROOT_MARKUP = render(
    createElement(SettingsRoot, { onOpen: () => undefined, vpnAuth: 'unknown', appUpdate: null }),
  );
});

const allMarkup = (): string => MOBILE_SETTINGS_PAGES.map((id) => MARKUP[id]).join('\n');

/** 一页里的「行」= 相邻两个 `data-*` 标记之间的片段。行不嵌套，故切片边界就是下一个标记。 */
const MARK = /data-(?:setting|note|push|status|external|back)="([^"]+)"/g;

function rows(markup: string): { id: string; chunk: string }[] {
  const marks = [...markup.matchAll(MARK)];
  return marks.map((m, i) => ({
    id: m[1],
    chunk: markup.slice(m.index ?? 0, i + 1 < marks.length ? marks[i + 1].index : markup.length),
  }));
}

/** 带 `role="switch"` 的行 id（顺序即 DOM 顺序）。 */
function switchRows(markup: string): string[] {
  return rows(markup)
    .filter((r) => r.chunk.includes('role="switch"'))
    .map((r) => r.id);
}

/* ── 期望：逐页开关集合（**逐值全等**，多一颗少一颗都红）────────────────────── */

const EXPECTED_SWITCHES: Record<MobileSettingsPageId, readonly string[]> = {
  // `auto-privacy-mode` 2026-09-06（W-16）落地：闲置计时 + 锁屏遮罩 + 密码三条腿都有了
  // （行为判据在 `mobile/privacy-lock.test.ts*`）。此前它**不在**这张表里，理由是「拨开了也
  // 锁不住任何东西」—— 那条理由不再成立，故这颗开关进表。
  // 锁屏**密码**那一行仍然不是开关（它是「设置/修改密码」按钮 + 状态文案，同桌面）。
  // `auto-start` 2026-09-25 落地：应用自己的「开机自动连接」（执行侧 `vpn/BootReceiver.kt`）。
  // 系统的「始终开启的 VPN」那一行（`boot-auto-connect`）仍是跳转项，**不在**这张表里。
  general: ['auto-connect', 'auto-check-update', 'auto-start', 'auto-privacy-mode', 'disable-log-file'],
  display: ['notifications'],
  network: [
    'block-quic',
    'tls-fragment',
    'auto-switch-node',
    'mesh-login-fallback',
    'interrupt-on-switch',
    'restart-on-node-change',
    'main-session-via-proxy',
  ],
  // race 档在夹具里是「单上游」⇒ 竞速池那三颗不渲染（⑫ 另有一份 race 开着的夹具逐值对拍）。
  dns: ['fake-ip', 'takeover-system-dns', 'optimistic-cache', 'fakeip-filter', 'block-browser-doh'],
  // `tun-bypass-lan` 2026-09-25（A6）落地：「绕过局域网」在 Android 上有了消费方（`inbounds.rs` 的
  // android 臂在开关开着时发清单网段子集）。此前它登记为 platform-absent，依据只覆盖回环前缀。
  tun: ['tun-auto-route', 'tun-ipv6', 'tun-bypass-lan'],
  // `auto-download-update` 2026-09-13（批 15）落地：应用内下载那条腿接上了，于是「要不要自动
  // 下载」才有了对象。此前它**不在**这张表里，理由是「那条腿还没画，没有可自动下载的东西」——
  // 那条理由随这一批不再成立（后端 `spawn_auto_download` 在 Android 上照跑，且 `decide_install_plan`
  // 认 `.apk`），故这颗开关进表。
  update: ['auto-download-update', 'sub-auto-on-start', 'rule-resource-auto'],
  // `system-backup` 2026-09-25 落地：系统自动备份（Google 云备份 + 换机迁移）的运行期闸门，默认关。
  // 执行侧 `PolarisBackupAgent.kt`，判据 ⑤c + `scripts/check-android-bridge.mjs` A14。
  backup: ['backup-select-all', ...BACKUP_CATEGORIES.map((c) => `backup-${c}`), 'system-backup'],
  about: [],
};

// ═══════════════════════════════════════════════════════════════════════════
describe('⓪ 自检：取材面是活的（否则下面每条都恒绿）', () => {
  it('源码取材面非空，且**不含本判据文件自己**', () => {
    expect(SOURCES.length, '本目录一个源码文件都没扫到 —— 取材面塌了').toBeGreaterThanOrEqual(10);
    expect(SOURCES.map((s) => s.file)).not.toContain('MobileSettings.test.tsx');
    // 正面：该在的文件在（改名/搬走会让下面的源码级断言变成对空气说话）。
    for (const file of ['NetworkPage.tsx', 'TunPage.tsx', 'GeneralPage.tsx', 'AboutPage.tsx']) {
      expect(SOURCES.map((s) => s.file), `${file} 不在取材面里`).toContain(file);
    }
  });

  it('八页都真的渲染出了东西（组件抛错会让「不含某某」全部空跑通过）', () => {
    for (const id of MOBILE_SETTINGS_PAGES) {
      expect(MARKUP[id].length, `${id} 页几乎没渲染出内容`).toBeGreaterThan(400);
    }
    expect(ROOT_MARKUP.length).toBeGreaterThan(400);
  });

  it('剥注释器把注释里的反面例子挡在取材面外，且不吃掉字符串', () => {
    // 这两句正是首跑时把 ⑦ ⑧ 弄红的形态：产品代码的**注释**里必须允许出现反面例子。
    expect(stripComments("/* 不接 setPrivacyMode */\nconst a = 1;\n")).not.toContain('setPrivacyMode');
    expect(stripComments("// 不引 InfoIcon\nconst b = 2;\n")).not.toContain('InfoIcon');
    // 反向对照：真的写在代码里就必须留下来（否则这个剥法会把整条判据变成恒绿）。
    expect(stripComments("const c = 'setPrivacyMode';\n")).toContain('setPrivacyMode');
    // 字符串里的 `/*` 不是注释起点（本仓踩过的那次事故形态）。
    expect(stripComments("const g = ['**/x/**'];\nconst d = 4;\n")).toContain('const d = 4');
  });

  it('切行器真的切得出行，且认得出开关', () => {
    expect(rows(MARKUP.general).length).toBeGreaterThan(5);
    expect(switchRows(MARKUP.general).length).toBeGreaterThan(0);
    // 反向对照：一段没有开关的 markup 必须切出 0 颗开关。
    expect(switchRows('<div data-setting="x">nothing</div>')).toEqual([]);
  });

  it('zh-CN 的 `mobileSettings` 命名空间在（本屏新增文案的落点）', () => {
    expect(copy('mobileSettings.back').length).toBeGreaterThan(0);
  });

  /*
   * `mobileSettings.pending`（「待接线」）2026-09-06 整条删掉。
   *
   * 它此前只有一个消费点：更新页那颗「应用版本读不到」的芯片。而**版本号读不到是一个运行期
   * 状态，不是一条没接的腿** —— 那颗芯片把两件事说成了一件。现在那一格与关于页同形（`—`），
   * 键随之无消费点，`i18n-coverage` 的死键门要求它必须被删掉（本仓死键零容忍）。
   *
   * 这条断言在这里，是为了让「哪天有人把它加回来当占位符」当场自曝：那多半意味着又有人
   * 拿「待接线」去描述一个运行期的空值。
   */
  it('「待接线」这条通用占位键已经没有了（它曾被用来描述一个运行期空值）', () => {
    for (const lang of LOCALES) {
      expect(
        leaf(localeJson(lang), 'mobileSettings.pending'),
        `${lang}: \`mobileSettings.pending\` 又回来了 —— 它不该被当成通用占位符`,
      ).toBeUndefined();
    }
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('① 分段构成 = 桌面 9 子页去掉 helper（裁定 #4：代码是真值）', () => {
  /** 从 `store/nav-store.ts` 的 `SettingsScreen` 联合里取出 9 个子页 id —— 那就是代码里的真值。 */
  const desktopPages = (() => {
    const src = readFileSync(join(UI_SRC, 'store/nav-store.ts'), 'utf8');
    const block = /export type SettingsScreen =([\s\S]*?);/.exec(src);
    if (!block) throw new Error('nav-store.ts 里找不到 `SettingsScreen` 联合 —— 判据面塌了');
    return [...block[1].matchAll(/'([a-z]+)'/g)].map((m) => m[1]);
  })();

  it('自检：桌面联合解析出了 9 项', () => {
    expect(desktopPages).toHaveLength(9);
    expect(desktopPages).toContain(MOBILE_ABSENT_SETTINGS_PAGE);
  });

  it('移动端 = 桌面九项**恰好**少 `helper` 一项（不是「数量等于 8」）', () => {
    expect([...MOBILE_SETTINGS_PAGES].sort()).toEqual(
      desktopPages.filter((p) => p !== MOBILE_ABSENT_SETTINGS_PAGE).sort(),
    );
  });

  it('`privacy` 不是二级页（它是通用页里的一个组，别为它建路由）', () => {
    expect(MOBILE_SETTINGS_PAGES as readonly string[]).not.toContain('privacy');
    // 正面：它确实以「通用页里的一组」的形态存在（组还在，锁两行缺席，见 ⑦）。
    expect(MARKUP.general).toContain(copy('settings.general.groupPrivacy'));
    expect(rows(MARKUP.general).map((r) => r.id)).toContain('disable-log-file');
  });

  it('二级页路由表的键集 = 八个二级页（少一键 = 有一页点进去是空白）', () => {
    // 编译期已由 `Record<MobileSettingsPageId, …>` 保证完整，但编译期不变式不进产物、
    // 也不会在别人改坏时留下一条能读的红 —— 故门在源码层再对拍一次（同 `mobile-entry.test.ts` ⑤）。
    const src = stripComments(readFileSync(join(HERE, 'MobileSettingsScreen.tsx'), 'utf8'));
    const start = src.indexOf('const SETTINGS_PAGES');
    expect(start, '`SETTINGS_PAGES` 不见了 —— 判据面塌了').toBeGreaterThan(-1);
    const block = /\{([\s\S]*?)\n\};/.exec(src.slice(start));
    expect(block, '`SETTINGS_PAGES` 块解析不出来').not.toBeNull();
    const keys = [...(block?.[1] ?? '').matchAll(/^\s{2}([a-z]+):/gm)].map((m) => m[1]);
    expect(keys).toEqual([...MOBILE_SETTINGS_PAGES]);
  });

  it('根页把八项都画成了可进入的行，顺序即 IA §2.4 的表格顺序', () => {
    const pushed = [...ROOT_MARKUP.matchAll(/data-push="([^"]+)"/g)].map((m) => m[1]);
    expect(pushed).toEqual([...MOBILE_SETTINGS_PAGES]);
  });

  /**
   * 状态芯片的三态**逐支**对拍。
   *
   * 只断言「有这一行」是不够的：本行的整个存在理由是「报一个真值」，而最坏的失效是
   * `unknown` 被渲染成「已授权」—— 用户据此以为权限给过了，起核时才发现没有。故三支各渲一次、
   * 逐支断言它显示的是自己那句，并**点名**断言 `unknown` 不得显示成 `authorized` 那句。
   */
  it('VPN 授权行按三态各说各的话，且 `unknown` 绝不显示成「已授权」（§4.4：没有真值就不许编）', () => {
    const chipOf = (vpnAuth: 'authorized' | 'denied' | 'unknown'): string => {
      const markup = render(
        createElement(SettingsRoot, { onOpen: () => undefined, vpnAuth, appUpdate: null }),
      );
      expect(markup, `${vpnAuth}：这一行整个不见了`).toContain('data-status="vpn-authorization"');
      expect(markup).toContain(copy('mobileSettings.vpnAuth.title'));
      // 这一行同时带 `data-setting` 与 `data-status` 两个标记 ⇒ `rows` 会把它切成两段，
      // 芯片文案在**后**一段里。取全部同 id 的片段拼起来，别只拿第一段（那一段里恒无芯片，
      // 三支都会「找不到」⇒ 断言变成恒红，是判据自己塌了而不是被测对象错了）。
      return rows(markup)
        .filter((r) => r.id === 'vpn-authorization')
        .map((r) => r.chunk)
        .join('');
    };
    expect(chipOf('authorized')).toContain(copy('mobileSettings.vpnAuth.granted'));
    expect(chipOf('denied')).toContain(copy('mobileSettings.vpnAuth.denied'));
    const unknown = chipOf('unknown');
    expect(unknown).toContain(copy('mobileSettings.vpnAuth.unknown'));
    expect(
      unknown,
      '`unknown`（读不到）被显示成了「已授权」—— 那是编一个事实，且方向有害',
    ).not.toContain(copy('mobileSettings.vpnAuth.granted'));
    // 不得借用助手页的字形与文案（§4.4：这是新契约，不是助手页的移植）。
    expect(ROOT_MARKUP).not.toContain(copy('settings.nav.helper'));
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * VPN 授权那一行的**两条接线判据**（补 2026-09-05 实测出的两个洞）。
 *
 * 上面那条渲染判据只看「给定一个状态，芯片说什么」。它对下面两件事完全看不见 ——
 * 两个变异都是 `tsc` rc=0 + `src/mobile/` 336/336 全绿实测出来的：
 *
 *  · **N4 重读接线**：把 `watchVpnAuth` 里的 `visibilitychange` 整段删掉、只留首次读。
 *    真机后果是用户去系统设置撤销授权再回来，芯片永远停在旧值。这是本仓反复踩的
 *    「机制有门、接线没门」形状，故这里走**行为断言**：拿假靶子驱动那条接线，派一次事件
 *    看有没有第二次读 —— 而不是去源码里 grep `visibilitychange` 这个词（那种判据把
 *    「写了这个词」当成「真的会重读」，一次重构就能骗过它）。
 *  · **N5 中途折叠**：`unknown → denied` 折在 hook 与标签表**之间**（屏组件里折一次），
 *    渲染判据拿到的已经是折过的值，故它绿。纪律只钉在 `VPN_AUTH_LABEL` 那张表上是不够的 ——
 *    值到达那张表之前的每一跳都得钉。
 */
describe('①b VPN 授权：重读接线 + 状态一路不被折', () => {
  /**
   * 子树里满足谓词的节点。
   *
   * 下面两条源码级判据一律走 **AST**、不走正则：本仓出过判据被同文件 KDoc 喂绿的先例，而注释与
   * 字符串字面量结构上就不是 `CallExpression`/`Identifier` —— 走 AST 等于取材面天然剥过注释，
   * 不必再维护一份「剥注释」的口径（那份口径本身也会漂）。
   */
  function nodesIn(root: ts.Node, pred: (n: ts.Node) => boolean): ts.Node[] {
    const out: ts.Node[] = [];
    const walk = (n: ts.Node): void => {
      if (pred(n)) out.push(n);
      ts.forEachChild(n, walk);
    };
    walk(root);
    return out;
  }

  /** `watchVpnAuth` 的假靶子。本仓 vitest 跑 node、没有 jsdom，全局 `document` 压根不存在。 */
  class FakeVisibility implements VisibilityTarget {
    visibilityState = 'visible';
    private listeners: (() => void)[] = [];
    addEventListener(_type: 'visibilitychange', listener: () => void): void {
      this.listeners.push(listener);
    }
    removeEventListener(_type: 'visibilitychange', listener: () => void): void {
      this.listeners = this.listeners.filter((l) => l !== listener);
    }
    /** 派发一次 `visibilitychange`（真机上「回到前台」那一刻）。 */
    fire(): void {
      for (const l of [...this.listeners]) l();
    }
    get listenerCount(): number {
      return this.listeners.length;
    }
  }

  /** 拿一串预置回值驱动 `watchVpnAuth`，记下「读了几次」与「apply 收到什么」。 */
  function drive(queue: readonly VpnAuthState[] = ['unknown']) {
    const target = new FakeVisibility();
    const seen: VpnAuthState[] = [];
    let reads = 0;
    const read = (): Promise<VpnAuthState> => {
      const next = queue[Math.min(reads, queue.length - 1)];
      reads += 1;
      return Promise.resolve(next);
    };
    const stop = watchVpnAuth(read, (s) => seen.push(s), target);
    return { target, stop, seen, reads: () => reads };
  }

  const flush = (): Promise<void> => new Promise((r) => setTimeout(r, 0));

  it('挂上就读一次（正面断言：不是「没报错」，是真的发生了一次读）', async () => {
    const d = drive(['authorized']);
    expect(d.reads(), '挂上之后一次都没读 —— 芯片从头到尾是初值').toBe(1);
    await flush();
    expect(d.seen).toEqual(['authorized']);
  });

  it('🔴 回到前台会**再读一次**（N4：这条不在，撤销授权后芯片永远停在旧值）', async () => {
    const d = drive(['authorized', 'denied']);
    await flush();
    d.target.fire();
    expect(
      d.reads(),
      '派了一次 visibilitychange 却没有第二次读 —— 重读接线断了：' +
        '用户去系统设置撤销授权再回来，这颗芯片会一直说「已授权」',
    ).toBe(2);
    await flush();
    expect(d.seen, '第二次读回来的新值没有送进界面').toEqual(['authorized', 'denied']);
  });

  it('反向对照：**不可见**时的那次事件不读（不是「一有事件就读」，也就不是恒绿）', async () => {
    const d = drive(['authorized']);
    await flush();
    d.target.visibilityState = 'hidden';
    d.target.fire();
    expect(d.reads(), '窗口不可见时也去读 —— 那是在后台白烧一次 IPC').toBe(1);
    // 再变回可见：证明上一条不是「事件监听器根本没挂上」造成的假绿。
    d.target.visibilityState = 'visible';
    d.target.fire();
    expect(d.reads(), '可见时那一发也没读 —— 说明监听器压根没挂上（上一条是假绿）').toBe(2);
  });

  it('退订后不再读，且监听器真的被摘掉（屏卸载后还在读 = 泄漏 + 对已卸载组件 setState）', async () => {
    const d = drive(['authorized']);
    await flush();
    expect(d.target.listenerCount).toBe(1);
    d.stop();
    expect(d.target.listenerCount, '退订闭包没有摘掉监听器').toBe(0);
    d.target.fire();
    expect(d.reads(), '退订之后还在读').toBe(1);
  });

  it('三态逐值透传，且读失败落 `unknown` 而**不是** `denied`（N5 的上游那一半）', async () => {
    for (const state of ['authorized', 'denied', 'unknown'] as const) {
      const d = drive([state]);
      await flush();
      expect(d.seen, `后端返 ${state}，送进界面的却不是它 —— 状态在接线里被折了`).toEqual([state]);
    }
    const target = new FakeVisibility();
    const seen: VpnAuthState[] = [];
    watchVpnAuth(() => Promise.reject(new Error('IPC 不通')), (s) => seen.push(s), target);
    await flush();
    expect(
      seen,
      '读失败被折成了 `denied` —— 那会把用户指去授予一个可能已经给过的权限',
    ).toEqual(['unknown']);
    const malformed: VpnAuthState[] = [];
    watchVpnAuth(
      () => Promise.resolve(null as unknown as VpnAuthState),
      (s) => malformed.push(s),
      new FakeVisibility(),
    );
    await flush();
    expect(malformed, 'IPC 回值异常时不应把状态显示成空白').toEqual(['unknown']);
  });

  /* ── N6：那段被测的逻辑，真的在生产路径上吗 ──────────────────────────────── */

  /**
   * 接线判据：`useVpnAuthState` 的 effect **就是**把这件事交给 `watchVpnAuth`，且交的是真东西。
   *
   * # 为什么上面五条行为断言不够
   *
   * 它们驱动的是 `watchVpnAuth` 的**方法体**。生产代码完全可以把 `watchVpnAuth` 原样留在文件里
   * （五条照样全过），自己在 effect 里内联一份**只读一次**的接线 —— 实测 `tsc` rc=0、
   * `src/mobile/` 343/343 全绿（协调者的 N6）。抽成可测函数把逻辑救出来了，却在原地留下一道新缝：
   * **函数被测，接线没被测**。行为断言守「逻辑对不对」，本条守「这段逻辑在不在生产路径上」，
   * 缺哪个都能造出一份全绿而功能没了的树。
   *
   * # 四条正面断言（不是「不许内联 then」——那种否定式会被别的绕法躲开）
   *
   *  1. effect 体里**有且只有一次** `watchVpnAuth(...)`；
   *  2. 三个实参分别是「读真 IPC 的那个」`vpnApi.getAuthStatus`、`useState` 的那个 setter、裸 `document`；
   *  3. effect **返回**那次调用的结果（退订闭包直传 —— 调了却把它丢掉 = 监听器泄漏）；
   *  4. effect 体里除那次调用的实参区间外**不得再出现 `vpnApi.`**（挡「既调它、又自己内联一份」）。
   *
   * 取材面自检：`useVpnAuthState` / 它的 `useEffect` / `useState` 的 setter 名，任一解析不到都进
   * 违规清单（判据自曝），绝不空跑绿。
   */
  function unwiredEffect(sf: ts.SourceFile): string[] {
    const bad: string[] = [];
    const HOOK = 'useVpnAuthState';
    const WIRE = 'watchVpnAuth';

    const hook = (nodesIn(sf, ts.isFunctionDeclaration) as ts.FunctionDeclaration[]).find(
      (f) => f.name !== undefined && ts.isIdentifier(f.name) && f.name.text === HOOK,
    );
    if (!hook) return [`判据塌了：树上找不到 ${HOOK} —— 取材面没了，不许当成「接线还在」`];

    // `const [state, setState] = useState(...)` 的第二个绑定名（不写死 `setState`：改名是合法重构）。
    const setter = (nodesIn(hook, ts.isVariableDeclaration) as ts.VariableDeclaration[])
      .filter(
        (d) =>
          d.initializer !== undefined &&
          ts.isCallExpression(d.initializer) &&
          ts.isIdentifier(d.initializer.expression) &&
          d.initializer.expression.text === 'useState',
      )
      .flatMap((d) =>
        ts.isArrayBindingPattern(d.name) && d.name.elements.length >= 2
          ? [d.name.elements[1]]
          : [],
      )
      .flatMap((el) => {
        if (!ts.isBindingElement(el)) return [];
        const n = el.name;
        return n !== undefined && ts.isIdentifier(n) ? [n.text] : [];
      })[0];
    if (setter === undefined) {
      bad.push(`判据塌了：${HOOK} 里解析不到 useState 的 setter 绑定`);
    }

    const effects = (nodesIn(hook, ts.isCallExpression) as ts.CallExpression[]).filter(
      (c) => ts.isIdentifier(c.expression) && c.expression.text === 'useEffect',
    );
    if (effects.length !== 1) {
      bad.push(`判据塌了：${HOOK} 里解析到 ${effects.length} 个 useEffect（应恰好 1 个）`);
      return bad;
    }
    const body = effects[0].arguments[0];
    if (body === undefined || !ts.isArrowFunction(body)) {
      bad.push(`判据塌了：${HOOK} 的 useEffect 第一个实参不是箭头函数`);
      return bad;
    }

    // ① 有且只有一次 watchVpnAuth(...)
    const wires = (nodesIn(body, ts.isCallExpression) as ts.CallExpression[]).filter(
      (c) => ts.isIdentifier(c.expression) && c.expression.text === WIRE,
    );
    if (wires.length !== 1) {
      bad.push(
        `${HOOK} 的 effect 里调了 ${wires.length} 次 ${WIRE}（应恰好 1 次）—— ` +
          `生产代码绕开了那段被测的接线，它读一次就再也不读了，而 ${WIRE} 的行为断言照样全过`,
      );
    }

    if (wires.length === 1) {
      const call = wires[0];
      // ② 三个实参各是什么
      const args = call.arguments;
      if (args.length !== 3) {
        bad.push(`${WIRE}( ) 收到 ${args.length} 个实参（应为 3：读腿 / apply / 事件靶子）`);
      } else {
        const readsIpc =
          nodesIn(args[0], ts.isPropertyAccessExpression).filter((n) => {
            const pa = n as ts.PropertyAccessExpression;
            return (
              ts.isIdentifier(pa.expression) &&
              pa.expression.text === 'vpnApi' &&
              ts.isIdentifier(pa.name) &&
              pa.name.text === 'getAuthStatus'
            );
          }).length > 0;
        if (!readsIpc) {
          bad.push(
            `${WIRE} 的第 1 个实参不读 vpnApi.getAuthStatus —— 接线挂上了，读的却不是真状态`,
          );
        }
        if (setter !== undefined && !(ts.isIdentifier(args[1]) && args[1].text === setter)) {
          bad.push(
            `${WIRE} 的第 2 个实参不是 useState 的 setter（${setter}），是 ` +
              `${args[1].getText(sf)} —— 读回来的值没有送进这个 hook 的 state`,
          );
        }
        if (!(ts.isIdentifier(args[2]) && args[2].text === 'document')) {
          bad.push(
            `${WIRE} 的第 3 个实参不是裸 document，是 ${args[2].getText(sf)} —— ` +
              `生产上挂的不是真的可见性靶子，那条重读腿在真机上永远不会被触发`,
          );
        }
      }

      // ③ 退订闭包必须直传出去
      const returned =
        body.body === call ||
        (ts.isBlock(body.body) &&
          (nodesIn(body.body, ts.isReturnStatement) as ts.ReturnStatement[]).some(
            (r) => r.expression === call,
          ));
      if (!returned) {
        bad.push(`${WIRE} 的返回值没有被 effect 返回出去 —— 退订闭包被丢掉，监听器随每次重挂泄漏`);
      }

      // ④ 不得在那次调用之外另开一条读 IPC 的路
      const outside = (nodesIn(body, ts.isPropertyAccessExpression) as ts.PropertyAccessExpression[])
        .filter((n) => ts.isIdentifier(n.expression) && n.expression.text === 'vpnApi')
        .filter((n) => !(n.getStart(sf) > call.getStart(sf) && n.end < call.end));
      if (outside.length > 0) {
        bad.push(
          `effect 里还有 ${outside.length} 处绕开 ${WIRE} 的 vpnApi.* 读 —— ` +
            `「既调它、又自己内联一份」同样让重读腿形同虚设`,
        );
      }
    } else {
      // 一次都没调时，把「那它在干什么」也点出来，别只说少了一次调用。
      const inlined = (nodesIn(body, ts.isPropertyAccessExpression) as ts.PropertyAccessExpression[])
        .filter((n) => ts.isIdentifier(n.expression) && n.expression.text === 'vpnApi');
      if (inlined.length > 0) {
        bad.push(
          `effect 里内联了 ${inlined.length} 处 vpnApi.* 读，而不是交给 ${WIRE} —— ` +
            `那份内联接线没有任何门守着（${WIRE} 的行为断言测的是它自己的方法体）`,
        );
      }
    }
    return bad;
  }

  it('🔴 `useVpnAuthState` 的 effect 把接线交给了 `watchVpnAuth`，交的是真 IPC + 真 document（N6）', () => {
    const file = join(HERE, 'MobileSettingsScreen.tsx');
    const sf = ts.parseSourceFile(file, readFileSync(file, 'utf8'));
    expect(
      unwiredEffect(sf),
      '被测的那段重读接线不在生产路径上 —— 上面五条行为断言全过，真机上却只读一次',
    ).toEqual([]);
  });

  it('反向对照：内联一份只读一次的接线**报得出**（证明上一条不是恒绿）', () => {
    const inlined = `
function useVpnAuthState() {
  const [state, setState] = useState('unknown');
  useEffect(() => {
    let alive = true;
    void vpnApi.getAuthStatus().then(
      (n) => { if (alive) setState(n); },
      () => { if (alive) setState('unknown'); },
    );
    return () => { alive = false; };
  }, []);
  return state;
}
export function watchVpnAuth(read, apply, target) {
  return () => undefined;
}
`;
    const hits = unwiredEffect(ts.parseSourceFile('synthetic-vpn-unwired.tsx', inlined));
    expect(hits.length, '协调者那条 N6 变异一模一样地放进来，判据一声不吭').toBeGreaterThan(0);
    expect(hits.join('\n')).toContain('调了 0 次 watchVpnAuth');
    expect(hits.join('\n')).toContain('内联了');
  });

  it('自检：取材面塌了要自曝（找不到 hook ⇒ 报「判据塌了」，不是空跑绿）', () => {
    const gone = unwiredEffect(ts.parseSourceFile('synthetic-vpn-nohook.tsx', 'export const x = 1;\n'));
    expect(gone.join('\n')).toContain('判据塌了');
  });

  /* ── N5：从 hook 到标签表，值一路是同一个裸标识符 ────────────────────────── */

  /**
   * 三跳身份链：`const X = useVpnAuthState()` → `<SettingsRoot vpnAuth={X}>` → `VPN_AUTH_LABEL[p]`。
   *
   * 任一跳上出现**表达式**（而不是裸标识符）就是「中途折了一次」。返回违规清单（空 = 干净），
   * 抽成函数是为了下面能拿一份合成夹具证明它**报得出**、不是恒绿。
   */
  function foldedHops(sf: ts.SourceFile): string[] {
    const bad: string[] = [];
    const nodes = (pred: (n: ts.Node) => boolean): ts.Node[] => nodesIn(sf, pred);

    // 跳 2：`vpnAuth={…}` 的实参必须是裸标识符。
    const attrs = (nodes(ts.isJsxAttribute) as ts.JsxAttribute[]).filter(
      (a) => ts.isIdentifier(a.name) && a.name.text === 'vpnAuth',
    );
    if (attrs.length === 0) bad.push('判据塌了：树上找不到 `vpnAuth={…}` 这个 JSX 属性');
    const passed: string[] = [];
    for (const a of attrs) {
      const init = a.initializer;
      const expr = init && ts.isJsxExpression(init) ? init.expression : undefined;
      if (expr && ts.isIdentifier(expr)) passed.push(expr.text);
      else bad.push(`跳2 vpnAuth={…} 传的不是裸标识符（中途折了一次）：${init?.getText(sf) ?? '（空）'}`);
    }

    // 跳 1：那个标识符的声明必须**恰好**是 `useVpnAuthState()` 这一次调用，不是包着它的表达式。
    for (const name of passed) {
      const decls = (nodes(ts.isVariableDeclaration) as ts.VariableDeclaration[]).filter(
        (d) => ts.isIdentifier(d.name) && d.name.text === name,
      );
      if (decls.length === 0) {
        bad.push(`跳1 ${name} 传进了 SettingsRoot，却找不到它的声明`);
        continue;
      }
      for (const d of decls) {
        const init = d.initializer;
        const ok =
          init !== undefined &&
          ts.isCallExpression(init) &&
          ts.isIdentifier(init.expression) &&
          init.expression.text === 'useVpnAuthState';
        if (!ok) {
          bad.push(
            `跳1 const ${name} = ${init?.getText(sf) ?? '（无初值）'} —— ` +
              '不是 useVpnAuthState() 本身，状态在到达界面之前被折了一次',
          );
        }
      }
    }

    // 跳 3：`VPN_AUTH_LABEL[…]` 的下标必须是 `SettingsRoot` 那个形参本身。
    const idx = (nodes(ts.isElementAccessExpression) as ts.ElementAccessExpression[]).filter(
      (e) => ts.isIdentifier(e.expression) && e.expression.text === 'VPN_AUTH_LABEL',
    );
    if (idx.length === 0) bad.push('判据塌了：树上找不到 `VPN_AUTH_LABEL[…]` 的查表');
    for (const e of idx) {
      if (!ts.isIdentifier(e.argumentExpression)) {
        bad.push(
          `跳3 VPN_AUTH_LABEL[${e.argumentExpression.getText(sf)}] —— ` +
            '查表下标不是裸标识符，折叠藏在查表那一刻',
        );
      }
    }
    return bad;
  }

  it('🔴 状态从 hook 到标签表一路不被折（N5：折在中途，上面那条渲染判据看不见）', () => {
    const file = join(HERE, 'MobileSettingsScreen.tsx');
    const sf = ts.parseSourceFile(file, readFileSync(file, 'utf8'));
    expect(
      foldedHops(sf),
      'VPN 授权状态在到达标签表之前被改写过 —— `unknown`（读不到）会被显示成 `denied`/`granted`，' +
        '而那正是「没有真值就不许编」要禁的事',
    ).toEqual([]);
  });

  it('反向对照：合成夹具里的一次折叠**报得出**（证明上一条不是恒绿）', () => {
    const folded = `
const VPN_AUTH_LABEL = { authorized: 'a', denied: 'b', unknown: 'c' };
function Screen() {
  const vpnAuthRaw = useVpnAuthState();
  const vpnAuth = vpnAuthRaw === 'unknown' ? 'denied' : vpnAuthRaw;
  return <SettingsRoot onOpen={() => undefined} vpnAuth={vpnAuth} />;
}
function SettingsRoot({ vpnAuth }) {
  return <span>{VPN_AUTH_LABEL[vpnAuth]}</span>;
}
`;
    const hits = foldedHops(ts.parseSourceFile('synthetic-vpn-fold.tsx', folded));
    expect(hits.length, '协调者那条 N5 变异一模一样地放进来，判据一声不吭').toBeGreaterThan(0);
    expect(hits.join('\n')).toContain('不是 useVpnAuthState() 本身');
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('② 不做 split view（收敛 §2.3 / 裁定 #6）', () => {
  it('根页只有进入行、没有设置控件；二级页只有设置控件、没有进入行', () => {
    // 两侧同时成立 ⇒ 结构上不可能出现「左栏目录 + 右栏内容」的双栏形态。
    expect(switchRows(ROOT_MARKUP), '根页出现了设置控件 —— 那是 split view 的形状').toEqual([]);
    for (const id of MOBILE_SETTINGS_PAGES) {
      expect(MARKUP[id], `${id} 页里出现了根页的进入行`).not.toContain('data-push=');
    }
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('③ 不出现「允许局域网访问管理 API」开关（裁定 #9 · §4.7 · §4.16）', () => {
  it('网络页的开关集合逐值全等（加回任何一颗都会撑大这个集合）', () => {
    expect(switchRows(MARKUP.network)).toEqual([...EXPECTED_SWITCHES.network]);
  });

  it('八页的任何一处都不出现管理面板 / 局域网 / 本地端口 / 终端代理的文案', () => {
    const surface = allMarkup() + '\n' + ROOT_MARKUP;
    for (const key of [
      'settings.network.mgmtBlock',
      'settings.network.mgmtEnable',
      'settings.network.mgmtPort',
      'settings.network.allowLan',
      'settings.advanced.localPort',
      'settings.network.mixedPort',
      'settings.network.systemProxyBlock',
    ]) {
      expect(surface, `渲染出了「${copy(key)}」（${key}）—— 该块在移动端应缺席`).not.toContain(copy(key));
    }
  });

  it('源码面不含这些能力的接线（文案没画不等于线没接）', () => {
    for (const token of [
      'singboxDashboard',
      'clashApiSecret',
      'controlPort',
      'openSingboxDashboard',
      'allowLan',
    ]) {
      expect(SOURCE_TEXT, `移动端设置屏里出现了 \`${token}\``).not.toContain(token);
    }
    // 「绕过局域网」（`bypassLAN` / `bypassLANList`）2026-09-25 从这串否定里移出（A6）：它不是
    // 「局域网访问管理 API」那一族 —— 那族是别的设备连进本机，这条是本机流量去局域网不进隧道，
    // 生成侧在 Android 上有消费方。它只该出现在 TUN 页，网络页仍不许有（系统代理那块在移动端缺席）。
    expect(MARKUP.tun, 'TUN 页没画出「绕过局域网」').toContain(copy('settings.advanced.bypassLAN'));
    expect(MARKUP.network, '网络页画出了「绕过局域网」—— 它在移动端归 TUN 页').not.toContain(
      copy('settings.advanced.bypassLAN'),
    );
    // 正面对照：网络页确实接了别的网络设置（否则上面一串否定是对空文件说的）。
    expect(SOURCE_TEXT).toContain('blockQuic');
    expect(SOURCE_TEXT).toContain('mainSessionViaProxy');
  });

  /*
   * 🔴 终端代理环境变量块（桌面 `SettingsNetwork.tsx` 的 `TerminalEnvBlock` + 两颗复制）是一条**安全裁定**的
   * 后果，不是平台做不到：那块复制走的是 `http_proxy=http://127.0.0.1:<mixedPort>`，而 Android 上
   * **按裁定不发** mixed inbound（`inbounds.rs` 的 mixed 段：共享回环 + 零认证，被排除的应用能借道）。
   * 四屏对差登记表里那四条（`f:onCopy` / `c:term-copy` / `TerminalEnvBlock` / `CopyIcon`）按
   * `adjudicated-out` 出账，锚就是本条 —— 所以本条同时钉两件事：块不出现；裁定本体还在。
   * 裁定被撤（Android 开始发 mixed）⇒ 下面的正面断言当场红 ⇒ 那四条必须重判，而不是继续躺在豁免里。
   */
  it('终端代理环境变量块不出现（安全裁定：Android 不发 mixed inbound）', () => {
    const surface = allMarkup() + '\n' + ROOT_MARKUP;
    for (const key of [
      'settings.advanced.terminalProxy',
      'settings.advanced.envVarsLabel',
      'settings.advanced.copyAllEnv',
      'settings.advanced.tipSessionOnly',
    ]) {
      expect(surface, `渲染出了「${copy(key)}」（${key}）—— 终端代理块在移动端应缺席`).not.toContain(copy(key));
    }
    for (const token of ['TerminalEnvBlock', 'splitTerminalEnvByPlatform', 'term-copy', 'http_proxy']) {
      expect(SOURCE_TEXT, `移动端设置屏里出现了 \`${token}\``).not.toContain(token);
    }
    // 正面断言：裁定本体（剥注释后的代码行）还在 —— Android 臂恒不发 mixed。
    const inbounds = maskRustComments(
      readFileSync(join(REPO, 'crates/config-engine/src/builder/inbounds.rs'), 'utf8'),
    );
    const fnAt = inbounds.indexOf('pub const fn emits_mixed_inbound(platform: Platform) -> bool');
    expect(fnAt, '`emits_mixed_inbound` 不见了 —— 裁定的单一真值换了地方，本条与登记表都要重判').toBeGreaterThan(-1);
    expect(
      inbounds.slice(fnAt, fnAt + 400),
      'Android 臂不再判「不发 mixed」—— 安全裁定被撤，终端代理块的缺席理由随之失效',
    ).toContain('Platform::Android | Platform::Ios => false,');
    // 正面对照：网络页本身渲染出来了（否则「不含」是对空串说的）。
    expect(MARKUP.network.length).toBeGreaterThan(200);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('④ `strict_route` 开关不显示（U2：承诺了却一行都不跑，比没有更坏）', () => {
  it('TUN 页的开关集合逐值全等（strict route 不在其中）', () => {
    expect(switchRows(MARKUP.tun)).toEqual([...EXPECTED_SWITCHES.tun]);
  });

  it('八页都不出现严格路由的标题与说明', () => {
    const surface = allMarkup() + '\n' + ROOT_MARKUP;
    expect(surface).not.toContain(copy('settings.tun.strictRoute'));
    expect(surface).not.toContain(copy('settings.tun.strictRouteDesc'));
  });

  it('正面对照：TUN 页本身是真的移植了（自动路由 / MTU 都在）', () => {
    expect(MARKUP.tun).toContain(copy('settings.tun.autoRoute'));
    expect(rows(MARKUP.tun).map((r) => r.id)).toContain('tun-mtu');
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('⑤ 系统「始终开启的 VPN」是跳转项，**不是开关**（裁定 #8 / W-17）', () => {
  it('通用页的开关集合逐值全等（boot-auto-connect 不在其中）', () => {
    expect(switchRows(MARKUP.general)).toEqual([...EXPECTED_SWITCHES.general]);
  });

  it('这一行确实存在，且它自己那一段里没有开关', () => {
    const row = rows(MARKUP.general).find((r) => r.id === 'boot-auto-connect');
    expect(row, '开机自动连接这一行不见了 —— 缺席不是裁定 #8 的处置').toBeDefined();
    expect(row?.chunk).not.toContain('role="switch"');
    expect(row?.chunk).toContain(copy('mobileSettings.general.bootConnectTitle'));
  });

  it('「应用自己开不了」这句话与系统设置路径都是常驻的（不是弹出、不是悬浮）', () => {
    expect(MARKUP.general).toContain(copy('mobileSettings.general.bootConnectDesc'));
    expect(MARKUP.general).toContain(copy('mobileSettings.general.bootConnectPath'));
  });

  /*
   * 🔴 W-17：跳转腿接上了，但**桥在不在决定这一行画成什么**。
   *
   * 判据面固定是 node 环境（没有原生桥）⇒ `MARKUP.general` 是「桥不在」那一档。
   * 「桥在」那一档要自己造一个假桥再渲染一次 —— 两档都必须验：
   *  · 只验「桥不在」⇒ 把跳转腿整条删掉也全绿（那正是本批之前的状态）；
   *  · 只验「桥在」⇒ 桌面调试档上会画出一个点了必然失败的跳转项，而那比一行说明更坏。
   */
  it('🔴 桥不在（桌面 / 浏览器调试档）⇒ 这一行不画成可点的', () => {
    const pushes = [...MARKUP.general.matchAll(/data-push="([^"]+)"/g)].map((m) => m[1]);
    expect(pushes, '原生桥不在却画了一个点了必然失败的跳转项').toEqual([]);
  });

  it('🔴 桥在（Android）⇒ 这一行变成可点的跳转项，且点它真的往桥上投递', () => {
    const g = globalThis as unknown as Record<string, unknown>;
    const before = g[VPN_SETTINGS_BRIDGE];
    const posted: string[] = [];
    g[VPN_SETTINGS_BRIDGE] = {
      postMessage: (m: string) => posted.push(m),
      onmessage: null,
    };
    try {
      const markup = render(pageOf('general'));
      const pushes = [...markup.matchAll(/data-push="([^"]+)"/g)].map((m) => m[1]);
      expect(pushes, '桥在场却仍然画成不可点的说明行 —— 裁定 #8 要的是跳转项').toEqual([
        'boot-auto-connect',
      ]);
      // 那一行仍然不是开关（裁定 #8 的不变量在两档下都成立）。
      const row = rows(markup).find((r) => r.id === 'boot-auto-connect');
      expect(row?.chunk).not.toContain('role="switch"');
      // 常驻路径不许因为「能跳了」就删掉：跳转只到 VPN 列表页，剩下两跳靠它（见生产侧头注）。
      expect(markup).toContain(copy('mobileSettings.general.bootConnectPath'));
      // 行为：那条腿真的会往桥上投递（而不是一个空壳）。
      void openSystemVpnSettings();
      expect(posted, '桥在场却没收到任何东西 —— 那一跳是个空壳').toEqual([VPN_SETTINGS_OPEN]);
    } finally {
      if (before === undefined) delete g[VPN_SETTINGS_BRIDGE];
      else g[VPN_SETTINGS_BRIDGE] = before;
    }
  });

  /*
   * 🔴 **生产那一跳**（复审同族形态：函数被测 ≠ 生产在用它）。
   *
   * 上一条只证明「`openSystemVpnSettings` 这个函数会往桥上投递」。把那一行的 `onOpen` 换成
   * `commit('boot-auto-connect', Promise.resolve(), …)`（跳转腿整条拔掉、行还在、点了什么都不做）
   * 时，上一条与本组其余每一条**全绿**（协调者实测）。故这里按**整次调用**对拍：
   * 行 id + 那次真的调用，两样都在同一个 `commit(` 的实参里。
   *
   * 拆成两条互不相干的子串（「有 `commit(`」+「有 `openSystemVpnSettings`」）挡不住这个变异 ——
   * 函数在文件里出现过（import 那一行就有），而实参位上是个 `Promise.resolve()`。
   */
  it('🔴 那一行的 `onOpen` 真的把跳转腿交给了 `commit`（不是一个空壳 promise）', () => {
    const src = stripComments(readFileSync(join(HERE, 'GeneralPage.tsx'), 'utf8'));
    expect(
      /commit\(\s*'boot-auto-connect'\s*,\s*openSystemVpnSettings\(\)\s*,/.test(src),
      '跳转腿没有落在 `commit(\'boot-auto-connect\', openSystemVpnSettings(), …)` 里 —— ' +
        '要么点了什么都不做，要么跳失败了没人说',
    ).toBe(true);
  });

  it('🔴 桥回执「失败」⇒ 那次跳转 reject（点了没反应必须落成红字，裁定 #14）', async () => {
    const g = globalThis as unknown as Record<string, unknown>;
    const before = g[VPN_SETTINGS_BRIDGE];
    const fake: { postMessage: (m: string) => void; onmessage: ((e: { data?: unknown }) => void) | null } = {
      postMessage: () => fake.onmessage?.({ data: VPN_SETTINGS_FAILED }),
      onmessage: null,
    };
    g[VPN_SETTINGS_BRIDGE] = fake;
    try {
      await expect(openSystemVpnSettings()).rejects.toThrow(VPN_SETTINGS_FAILED);
      // 正向对照：回执「成功」时 resolve（证明上面那条不是「恒 reject」）。
      fake.postMessage = () => fake.onmessage?.({ data: VPN_SETTINGS_OK });
      await expect(openSystemVpnSettings()).resolves.toBeUndefined();
    } finally {
      if (before === undefined) delete g[VPN_SETTINGS_BRIDGE];
      else g[VPN_SETTINGS_BRIDGE] = before;
    }
    // 桥不在 ⇒ 立刻 reject（一个「桥不在就静默成功」的实现会让那条早退失效时无声无息）。
    await expect(openSystemVpnSettings()).rejects.toThrow();
  });

  it('🔴 跨语言：Kotlin 侧那条桥的名字与本侧逐字相同，且发的是 VPN 设置那个 action', () => {
    const kt = maskRustComments(
      readFileSync(
        join(REPO, 'src-tauri/gen/android/app/src/main/java/com/polaris2/app/MainActivity.kt'),
        'utf8',
      ),
    );
    expect(kt.length, 'MainActivity.kt 读到的是空文件 —— 判据面塌了').toBeGreaterThan(400);
    const name = /private const val VPN_SETTINGS_BRIDGE\s*=\s*"([^"]+)"/.exec(kt);
    expect(name, 'Kotlin 侧找不到 `VPN_SETTINGS_BRIDGE`（改名了？）').not.toBeNull();
    expect(
      name?.[1],
      '两侧桥名漂了 —— `postMessage` 会打在 undefined 上，点了静默无反应、运行期零报错',
    ).toBe(VPN_SETTINGS_BRIDGE);
    // 收信口装在 WebView 建好那一刻（不是「整份文件里出现过」—— 后者靠定义本身就恒真）。
    const created = /fun onWebViewCreate\([\s\S]*?\n  \}/.exec(kt)?.[0] ?? '';
    expect(created, '收信口没装 —— `window.polarisVpnSettings` 永远不存在').toContain(
      'installVpnSettingsBridge(webView)',
    );
    const fn = /fun installVpnSettingsBridge\([\s\S]*?\n  \}/.exec(kt)?.[0] ?? '';
    expect(fn.length, '`installVpnSettingsBridge` 的函数体切不出来').toBeGreaterThan(80);
    /*
     * 🔴 **action 必须是 `Settings.ACTION_VPN_SETTINGS`**，不是一个手打的 intent 字符串。
     * 取值 `"android.settings.VPN_SETTINGS"` 实测自 android-34 `android.jar` 的常量池；
     * 写字面量的话拼错一个字母 = 运行期 `ActivityNotFoundException`，编译期一个字都不说。
     */
    expect(fn, '没走 `Settings.ACTION_VPN_SETTINGS` —— 手打 intent 串拼错了编译期看不见').toContain(
      'Intent(Settings.ACTION_VPN_SETTINGS)',
    );
    expect(fn, '缺 NEW_TASK：设置页会被压进本应用的任务栈').toContain('FLAG_ACTIVITY_NEW_TASK');
    // 两个分支各发一条回执（少一条 = 那一档静默）。
    expect(fn).toContain('VPN_SETTINGS_OK');
    expect(fn).toContain('VPN_SETTINGS_FAILED');
  });

  it('桌面「开机自启」「静默启动」两行的**桌面文案**不移植（§4.3）', () => {
    // 开机自启换了对象（移动端是「开机自动连接」，文案另起一套，见 ⑤b）；静默启动在移动端没有对象。
    expect(MARKUP.general).not.toContain(copy('settings.general.autoStartTitle'));
    expect(MARKUP.general).not.toContain(copy('settings.general.silentStart'));
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('⑤b 应用自己的「开机自动连接」开关（`autoStart` 的 Android 腿，2026-09-25）', () => {
  it('这一行存在、是开关，且与系统「始终开启的 VPN」那一行各自说明区别', () => {
    const row = rows(MARKUP.general).find((r) => r.id === 'auto-start');
    expect(row, '开机自动连接开关不见了').toBeDefined();
    expect(row?.chunk).toContain('role="switch"');
    expect(row?.chunk).toContain(copy('mobileSettings.general.autoStartTitle'));
    expect(row?.chunk).toContain(copy('mobileSettings.general.autoStartDesc'));
    // 两行并列：跳转项那一行仍在（反向对照：本开关没有顶掉系统那条路）。
    expect(rows(MARKUP.general).map((r) => r.id)).toContain('boot-auto-connect');
    expect(copy('mobileSettings.general.autoStartTitle')).not.toBe(
      copy('mobileSettings.general.bootConnectTitle'),
    );
  });

  /*
   * 🔴 写腿按**整次调用**对拍（同 ⑤ 那条的口径）：先写执行侧（`autoStartApi.set` → Rust
   * `auto_start_set` → Kotlin 标记文件），成功后才落配置镜像。拆成两个子串挡不住「只落配置、执行侧
   * 整条拔掉」这一形 —— 那时开关亮着，开机时什么都不会发生。
   * 2026-09-25：写完（无论成败）**重读执行侧**作显示值 —— 镜像落盘失败时开机行为已经变了，
   * 显示跟执行侧走；重读失败回到「不知道」（禁用），不猜一个值。
   */
  it('🔴 写腿 = commit(\'auto-start\', autoStartApi.set(v).then(() => update(…)).finally(重读执行侧))', () => {
    const src = stripComments(readFileSync(join(HERE, 'GeneralPage.tsx'), 'utf8'));
    expect(
      /commit\(\s*'auto-start'\s*,\s*autoStartApi\.set\(v\)\.then\(\(\)\s*=>\s*update\(\{\s*autoStart:\s*v\s*\}\)\s*\)\.finally\(\(\)\s*=>\s*autoStartApi\.getStatus\(\)\.then\(setAutoStart,\s*\(\)\s*=>\s*setAutoStart\(null\)\)\s*,?\s*\)\s*,?\s*\)/.test(
        src,
      ),
      '开机自动连接的写腿没有「先执行侧、后配置、写完重读执行侧」地挂在 commit(\'auto-start\', …) 上',
    ).toBe(true);
  });

  /*
   * 🔴 显示值的**来源**（2026-09-25）：user config 随系统备份走，恢复后 `config.autoStart` 可能与
   * Kotlin 侧 `noBackupFilesDir` 里的真值分叉 ⇒ 开关必须读执行侧，不读配置。
   * 夹具是 SSR（effect 不跑），故「未读到」这一档就是首帧：喂一份 `autoStart: true` 的配置，
   * 开关若仍按配置画就会亮成「开」—— 本条要它禁用且未勾选。
   */
  it('🔴 显示值来自 status 命令而非 config：config 说「开」、真值未读到 ⇒ 禁用 + 未勾选', () => {
    const html = render(
      createElement(GeneralPage, {
        config: { ...CONFIG, autoStart: true } as UserConfig,
        update: async () => undefined,
        commit: () => undefined,
      }),
    );
    const row = rows(html).find((r) => r.id === 'auto-start');
    expect(row, '开机自动连接开关不见了（下面两条会空跑）').toBeDefined();
    expect(row?.chunk).toContain('role="switch"');
    expect(row?.chunk, '开关按 config.autoStart 亮成了「开」—— 显示源仍是配置').toContain('aria-checked="false"');
    expect(row?.chunk, '还没读到执行侧真值时开关没禁用').toContain('disabled');
    // 反向对照：同一份渲染里，读 config 的开关（启动即连接，CONFIG.autoConnect = true）确实亮着 ——
    // 证明「未勾选」不是因为这份夹具里所有开关都画不亮。
    expect(rows(html).find((r) => r.id === 'auto-connect')?.chunk).toContain('aria-checked="true"');
  });

  it('🔴 读腿 = 挂载时经 status 命令读执行侧真值，读失败落在同一行（不静默显示成「关」）', () => {
    const src = stripComments(readFileSync(join(HERE, 'GeneralPage.tsx'), 'utf8'));
    expect(
      /commit\(\s*'auto-start'\s*,\s*autoStartApi\.getStatus\(\)\.then\(setAutoStart\)/.test(src),
      '开机自动连接开关没有从执行侧读初值，或读失败没接回显',
    ).toBe(true);
    expect(src).toContain('checked={autoStart === true}');
    expect(src).toContain('disabled={autoStart === null}');
    expect(src, '开关又从 config.autoStart 取显示值了').not.toMatch(/checked=\{[^}]*config\.autoStart/);
  });

  it('跨语言：status 命令在 Android 上读的是桥 → Kotlin 真值，不是 config', () => {
    const rs = maskRustComments(readFileSync(join(REPO, 'src-tauri/src/commands/misc/autostart.rs'), 'utf8'));
    const at = rs.indexOf('pub async fn auto_start_get_status');
    expect(at, '`auto_start_get_status` 不见了').toBeGreaterThan(-1);
    const body = rs.slice(at, at + 900);
    expect(body).toContain('#[cfg(target_os = "android")]');
    expect(body).toContain('android_bridge::boot_auto_connect()');
    expect(body, 'status 命令读了配置 —— 那份会随系统备份分叉').not.toMatch(/auto_start\b|autoStart/);
  });

  it('五种 locale 都有这一行的标题与说明', () => {
    for (const loc of ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa']) {
      const j = JSON.parse(readFileSync(join(HERE, '..', '..', 'i18n', 'locales', `${loc}.json`), 'utf8')) as {
        mobileSettings: { general: Record<string, string> };
      };
      expect(j.mobileSettings.general.autoStartTitle, `${loc} 缺 autoStartTitle`).toBeTruthy();
      expect(j.mobileSettings.general.autoStartDesc, `${loc} 缺 autoStartDesc`).toBeTruthy();
      expect(j.mobileSettings.general.autoStartReadFailed, `${loc} 缺 autoStartReadFailed`).toBeTruthy();
      expect(j.mobileSettings.general.autoStartReadFailedWithReason, `${loc} 缺 autoStartReadFailedWithReason`).toContain(
        '{{reason}}',
      );
    }
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('⑤c 「系统备份」开关（Android Auto Backup 的运行期闸门，默认关，2026-09-25）', () => {
  const PAGE = () => stripComments(readFileSync(join(HERE, 'BackupPage.tsx'), 'utf8'));

  it('这一行在备份页、是开关，标题与说明（含凭据、锁屏加密条件）都在', () => {
    const row = rows(MARKUP.backup).find((r) => r.id === 'system-backup');
    expect(row, '系统备份开关不见了').toBeDefined();
    expect(row?.chunk).toContain('role="switch"');
    expect(row?.chunk).toContain(copy('mobileSettings.backup.systemBackupTitle'));
    expect(row?.chunk).toContain(copy('mobileHelp.systemBackup'));
    expect(MARKUP.backup).toContain(copy('mobileSettings.backup.systemBackupGroup'));
  });

  it('还没读到执行侧真值时开关是禁用的、不是亮成「开」', () => {
    // 夹具是 SSR：effect 不跑 ⇒ 状态停在「未知」。未知必须画成禁用 + 未勾选，而不是某个猜的值。
    const row = rows(MARKUP.backup).find((r) => r.id === 'system-backup');
    expect(row?.chunk).toContain('disabled');
    expect(row?.chunk).toContain('aria-checked="false"');
  });

  /*
   * 🔴 写腿按**整次调用**对拍：执行侧（`api.systemBackup.set` → Rust `system_backup_set` → Kotlin
   * noBackupFilesDir 里的开关文件）成功后才改界面状态。只改界面、执行侧整条拔掉 ⇒ 开关亮着，
   * 系统照旧什么都不备份（或反过来：界面说关、实际在备份）。
   */
  it('🔴 写腿 = commit(\'system-backup\', api.systemBackup.set(v).then(() => setEnabled(v)))', () => {
    expect(
      /commit\(\s*'system-backup'\s*,\s*api\.systemBackup\.set\(v\)\.then\(\(\)\s*=>\s*setEnabled\(v\)\s*\)\s*,?\s*\)/.test(PAGE()),
      '系统备份的写腿没有「先执行侧、后界面」地挂在 commit(\'system-backup\', …) 上',
    ).toBe(true);
  });

  it('🔴 读腿 = 挂载时读执行侧真值，读失败落在同一行（不静默显示成「关」）', () => {
    expect(
      /commit\(\s*'system-backup'\s*,\s*api\.systemBackup\.getStatus\(\)\.then\(setEnabled\)/.test(PAGE()),
      '系统备份开关没有从执行侧读初值，或读失败没接回显',
    ).toBe(true);
  });

  it('真值不进 user config（config 自己随备份走，副本恢复后会与本机真值分叉）', () => {
    expect(PAGE()).not.toMatch(/update\(\s*\{[^}]*[Bb]ackup/);
    expect(PAGE()).not.toMatch(/config\.\w*[Ss]ystemBackup/);
  });

  it('桌面设置不画这一行（Android 独有能力，桌面无对应物）', () => {
    const desktopDir = join(HERE, '..', '..', 'components', 'screens', 'settings');
    for (const f of readdirSync(desktopDir).filter((n) => n.endsWith('.tsx') && !n.includes('.test.'))) {
      expect(readFileSync(join(desktopDir, f), 'utf8'), `${f} 引用了系统备份`).not.toMatch(/systemBackup|system-backup/);
    }
  });

  it('五种 locale 都有这一组的全部文案', () => {
    const keys = ['systemBackupGroup', 'systemBackupTitle', 'systemBackupDesc', 'systemBackupReadFailed', 'systemBackupReadFailedWithReason'];
    for (const loc of ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa']) {
      const j = JSON.parse(readFileSync(join(HERE, '..', '..', 'i18n', 'locales', `${loc}.json`), 'utf8')) as {
        mobileSettings: { backup: Record<string, string> };
      };
      for (const k of keys) expect(j.mobileSettings.backup[k], `${loc} 缺 ${k}`).toBeTruthy();
      expect(j.mobileSettings.backup.systemBackupReadFailedWithReason).toContain('{{reason}}');
    }
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('⑥ 关于页的许可文案按平台给对（Android 产物 = GPLv3）', () => {
  it('真值源在：仓根 `NOTICE` 里确有按平台分叉的那一节', () => {
    const notice = readFileSync(join(REPO, 'NOTICE'), 'utf8');
    expect(notice).toContain('Android');
    expect(notice).toContain('GPLv3');
    expect(notice).toContain('MIT');
  });

  it('五语种：产物那行必须说 GPLv3，且**不得**说 MIT', () => {
    for (const lang of LOCALES) {
      const value = leaf(localeJson(lang), 'mobileSettings.about.licenseArtifactValue');
      expect(typeof value, `${lang} 缺 licenseArtifactValue`).toBe('string');
      expect(value as string, `${lang}: 产物许可没说 GPLv3`).toContain('GPLv3');
      expect(value as string, `${lang}: 产物许可说成了 MIT —— 拿到 APK 的人得到的是 GPLv3`).not.toContain(
        'MIT',
      );
    }
  });

  it('五语种：源码那行必须说 MIT（两件事分开写，不是二选一）', () => {
    for (const lang of LOCALES) {
      const value = leaf(localeJson(lang), 'mobileSettings.about.licenseSourceValue');
      expect(value as string, `${lang}: 源码许可没说 MIT`).toContain('MIT');
    }
  });

  it('关于页把两行都画出来了', () => {
    expect(MARKUP.about).toContain(copy('mobileSettings.about.licenseSourceValue'));
    expect(MARKUP.about).toContain(copy('mobileSettings.about.licenseArtifactValue'));
  });

  it('完全卸载：动作缺席，但那句话不许静默丢掉（§4.9）', () => {
    expect(MARKUP.about).not.toContain(copy('settings.about.uninstallTitle'));
    expect(MARKUP.about).not.toContain(copy('settings.about.uninstallDesc'));
    expect(MARKUP.about).toContain(copy('mobileSettings.about.uninstallNote'));
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('⑦ 隐私锁：三条腿都在，且解锁只能走 `privacy_unlock`（裁定 #10 / W-16）', () => {
  /*
   * ⚠️ **这一组 2026-09-06 翻了面**。此前它守的是「整组缺席」：闲置计时只在桌面 `App.tsx`、
   * 锁屏遮罩只在桌面 `AppShell.tsx`，移动入口一条都够不着 ⇒ 拨开开关**什么都不会锁**，
   * 故整组不画、只留一句常驻说明。W-16 把三条腿都接上了，那句说明随之删掉，本组改为
   * **正面事实断言**：接线在不在、以及裁定 #10 那道闸有没有被绕过去。
   *
   * 行为级判据（闲置计时真的在计时 / 解锁真的先打 `privacy_unlock` / 遮罩不登记进 back-stack）
   * 住在 `mobile/privacy-lock.test.tsx` —— 那边能对假 IPC 与假计时器直接驱动。本组只管**这一屏**。
   */
  it('闲置计时与锁屏遮罩现在真的挂在移动入口上（缺席结论已作废，取证换成正面的）', () => {
    const app = readFileSync(join(UI_SRC, 'App.tsx'), 'utf8');
    const shell = readFileSync(join(UI_SRC, 'components/layout/AppShell.tsx'), 'utf8');
    const mobileApp = readFileSync(join(UI_SRC, 'mobile/MobileApp.tsx'), 'utf8');
    const mobileWiring = readFileSync(join(UI_SRC, 'mobile/app-wiring.ts'), 'utf8');
    // 桌面那两处仍在（取证面是活的）。
    expect(app).toContain('useIdlePrivacyLock()');
    expect(shell).toContain('<LockOverlay />');
    // 移动端有**自己的**那两条（不是 import 桌面那两个 —— 后者会把桌面层叠链拖进移动包）。
    expect(mobileWiring, '移动入口没有闲置计时 —— 拨开开关什么都不会锁').toContain(
      'armIdlePrivacyLock',
    );
    expect(mobileApp, '移动入口没有挂锁屏遮罩 —— 锁上之后界面照常可读').toContain(
      '<MobileLockOverlay',
    );
    expect(mobileApp, '把桌面遮罩引进了移动树（契约 A1：那会拖进桌面层叠链）').not.toContain(
      'LockOverlay />\n',
    );
  });

  it('通用页现在画出了隐私锁两行（开关 + 密码），且密码行只在锁开着时出现', () => {
    const ids = rows(MARKUP.general).map((r) => r.id);
    expect(ids, '自动隐私锁开关不见了').toContain('auto-privacy-mode');
    // 夹具里 `autoPrivacyMode: true` ⇒ 密码行在场。
    expect(ids, '密码行不见了（夹具里隐私锁是开着的）').toContain('privacy-password');
    expect(MARKUP.general).toContain(copy('settings.general.autoPrivacyMode'));
    expect(MARKUP.general).toContain(copy('settings.general.privacyPassword'));
    // 🔴 契约 L111：锁关着时密码行**不该**在（恒显会让人以为「设了密码就有保护」）。
    const off = render(
      createElement(GeneralPage, {
        config: { ...CONFIG, autoPrivacyMode: false } as unknown as UserConfig,
        update: async () => undefined,
        commit: () => undefined,
      }),
    );
    expect(rows(off).map((r) => r.id), '隐私锁关着时密码行还在').not.toContain('privacy-password');
    // 正向对照：那一档下开关本身仍在（证明上面那条不是「整页没渲染」）。
    expect(rows(off).map((r) => r.id)).toContain('auto-privacy-mode');
  });

  it('那句「移动端不提供隐私锁」的缺席说明已经不在了（接上了就不许再说没接）', () => {
    expect(MARKUP.general).not.toContain('data-note="privacy-lock-absent"');
    for (const lang of LOCALES) {
      expect(
        leaf(localeJson(lang), 'mobileSettings.general.privacyLockAbsent'),
        `${lang}: 那条缺席说明还留在 locale 里 —— 能力接上了，文案就是假话`,
      ).toBeUndefined();
    }
  });

  it('🔴 设置屏结构上不碰隐私态的翻转（那是遮罩与闲置计时的唯一收敛点）', () => {
    expect(SOURCE_TEXT).not.toContain('setPrivacyMode');
    // 正面对照：本屏确实碰**密码**（否则上面那条否定是对空气说的）。
    expect(SOURCE_TEXT).toContain('privacyApi.setPassword');
    expect(SOURCE_TEXT).toContain('disableLogFile');
  });

  it('一旦接生物识别，必须同时经过 `privacy.unlock`（只许当「取得凭据」的方式）', () => {
    const biometric = /biometric|BiometricPrompt|navigator\.credentials|webauthn/i.test(SOURCE_TEXT);
    if (biometric) {
      expect(
        /privacyApi\.unlock|PRIVACY_UNLOCK/.test(SOURCE_TEXT),
        '出现了生物识别，却没有走 privacy_unlock —— 直接翻标志位会绕过整条校验链',
      ).toBe(true);
    }
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('⑧ 只靠悬浮提示承载的解释都换成了常驻通道（§4.12，逐条计数）', () => {
  /**
   * 桌面把这些解释挂在 `InfoIcon` 的 `tip` 上，触屏没有 hover。机制详情已改为可点 i，关键说明仍常驻。逐条登记它落在**哪一页**，
   * 断言那句话真的出现在那一页的 DOM 里。**这是一份计数，不是印象**（§4.12 的验收口径）。
   */
  const RESIDENT: readonly [MobileSettingsPageId, string][] = [
    ['general', 'settings.general.autoConnectDesc'],
    ['general', 'settings.general.autoCheckUpdateDesc'],
    ['general', 'mobileHelp.disableLogs'],
    ['display', 'settings.display.languageDesc'],
    ['network', 'settings.network.directInterfaceDesc'],
    ['network', 'settings.network.proxyInterfaceDesc'],
    ['network', 'settings.network.blockQuicDescFull'],
    ['network', 'settings.network.webrtcLeakDesc'],
    ['network', 'settings.advanced.tlsFragmentDesc'],
    ['network', 'settings.advanced.autoSwitchNodeDesc'],
    ['network', 'settings.network.meshLoginFallbackDesc'],
    ['network', 'settings.network.interruptOnSwitchDesc'],
    ['network', 'settings.network.restartOnNodeChangeDesc'],
    ['network', 'settings.advanced.mainSessionViaProxyDesc'],
    ['network', 'settings.network.speedTestUrlDesc'],
    ['dns', 'mobileHelp.connectionResolution'],
    ['dns', 'settings.dns.fakeIpDesc'],
    ['dns', 'settings.dns.remoteDnsDesc'],
    ['dns', 'settings.dns.domesticDnsDesc'],
    ['dns', 'settings.dns.takeoverSystemDnsDesc'],
    ['dns', 'settings.advanced.optimisticCacheDesc'],
    ['dns', 'settings.advanced.dnsTimeoutDesc'],
    ['dns', 'settings.dns.raceStrategyDesc'],
    ['tun', 'settings.tun.mtuDesc'],
    ['tun', 'settings.tun.autoRouteDesc'],
    ['tun', 'settings.tun.natTypeDesc'],
    ['tun', 'mobileHelp.ipv6'],
    ['update', 'settings.update.intervalCardSub'],
    ['update', 'settings.update.subChannelDesc'],
    ['update', 'settings.update.ruleResourceAutoDesc'],
  ];

  it('自检：登记表有量级（表被清空会让下面那条恒绿）', () => {
    expect(RESIDENT.length).toBeGreaterThanOrEqual(30);
  });

  it('逐条都落在了对应页面的 DOM 上', () => {
    const missing = RESIDENT.filter(([page, key]) => !MARKUP[page].includes(copy(key)));
    expect(missing.map(([p, k]) => `${p} ← ${k}`), '这些解释在移动端没有常驻落点').toEqual([]);
  });

  it('本屏不引桌面那颗悬浮提示图标（它在触屏上没有触发路径）', () => {
    expect(SOURCE_TEXT).not.toContain('InfoIcon');
    expect(SOURCE_TEXT).not.toContain('data-tip');
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('⑨ 能力缺席：该缺的块整块不画，不画成禁用（§4.2 / §4.3 / §4.5 / §4.8）', () => {
  it('窗口 / 托盘 / 图形 / 终端代理这些桌面对象一处都不出现', () => {
    const surface = allMarkup() + '\n' + ROOT_MARKUP;
    for (const key of [
      'settings.display.groupWindowNotify',
      'settings.display.closeBehavior',
      'settings.general.rememberWindowSize',
      'settings.general.autoLightweightMode',
      'settings.general.keepTrayMenuWarm',
      'settings.general.groupGraphics',
      'settings.general.hardwareAcceleration',
      'settings.general.windowEffects',
      'settings.nav.helper',
    ]) {
      expect(surface, `渲染出了「${copy(key)}」（${key}）—— 该项在移动端应缺席`).not.toContain(copy(key));
    }
  });

  /*
   * 🔴 D8：三项「Android 上做得到、但价值低」的桌面能力，由**用户裁定**正式放弃（不是平台做不到）。
   * 可行替代各一句，留给将来重议时不必重新调研：硬件加速逃生门 = WebView `setLayerType(LAYER_TYPE_SOFTWARE)`；
   * 应用内卸载 = `Intent.ACTION_DELETE`（需 `REQUEST_DELETE_PACKAGES`）请求系统卸载；
   * 轻量模式 = 销毁 WebView 后由前台通知的 contentIntent 充当回程。
   * 四屏对差登记表里这三条按 `adjudicated-out` 出账，锚就是本条：本条被删 ⇒ 那三条豁免当场失效。
   */
  it('D8 用户裁定放弃（2026-09-25）：硬件加速逃生门 / 应用内卸载 / 轻量模式一处都不出现', () => {
    const surface = allMarkup() + '\n' + ROOT_MARKUP;
    for (const key of [
      'settings.general.hardwareAcceleration',
      'settings.general.autoLightweightMode',
      'settings.about.uninstallTitle',
      'settings.about.uninstallConfirmAgain',
    ]) {
      expect(surface, `渲染出了「${copy(key)}」（${key}）—— D8 裁定放弃的项在移动端应缺席`).not.toContain(copy(key));
    }
    for (const token of ['hardwareAcceleration', 'autoLightweightMode', 'uninstallAll']) {
      expect(SOURCE_TEXT, `移动端设置屏里出现了 \`${token}\`（D8 裁定放弃）`).not.toContain(token);
    }
    // 正面对照：取材面是活的 —— 通用页与关于页真的渲染了，关于页那句「去系统里卸载」的指路常驻。
    expect(MARKUP.general).toContain(copy('settings.general.autoConnect'));
    expect(MARKUP.about).toContain(copy('mobileSettings.about.uninstallNote'));
    expect(SOURCE_TEXT).toContain('autoConnect');
  });

  it('更新：只保留真实 APK 更新入口，不显示独立内核更新或假版本', () => {
    expect(rows(MARKUP.update).some((r) => r.id === 'core-update')).toBe(false);
    expect(MARKUP.update).not.toContain(copy('settings.dns.builtinTag'));
    expect(switchRows(MARKUP.update)).toEqual([...EXPECTED_SWITCHES.update]);
    /*
     * 版本不是编的：fixture 下 `versionApi` 的 effect 不跑 ⇒ 应用那一行的芯片只能是 `—`。
     *
     * ⚠️ 这条断言 2026-09-06（W-19）改过一次口径。此前它断言的是「显示『待接线』」，而
     * **版本号读不到是一个运行期状态，不是一条没接的腿** —— 那颗芯片把两件事说成了一件。
     * 现在与关于页同形（`AboutPage.tsx:155` 的 `—`），`mobileSettings.pending` 整条键随之删掉，
     * ⓪ 组另有一条断言钉着它不许回来。
     */
    const chip = rows(MARKUP.update).filter((r) => r.id === 'app-update');
    expect(chip.length, '应用更新那一行不见了').toBeGreaterThan(0);
    expect(chip.map((r) => r.chunk).join(''), '应用版本那一格冒出了一个假版本号').toContain('—');
    /*
     * 🔴 **下载 / 安装那一跳 2026-09-13（批 15）接上了**，这一条因此换了对象：
     * 从「不许画那两个阶段」换成「画出来的必须是 Android 真有的那条腿」。
     *
     * 桌面那四句**仍然**不许出现，但理由变了 —— 不是「没有这条腿」，而是**它们描述的是另一种
     * 落地方式**：桌面是「下载 → 写安装脚本 → 停代理 → 退出应用 → 重启装上」，Android 是
     * 「下载 → 经 FileProvider 交系统安装器 → 本进程继续活着等」。照搬「重启并安装」那句话，
     * 就是承诺一件这个平台上不会发生的事（后端 Android 分支明写不停代理、不退应用）。
     */
    for (const key of [
      'settings.update.downloadingApp',
      'settings.update.restartAndInstall',
      'settings.update.restartToInstall',
    ]) {
      expect(
        MARKUP.update,
        `更新页画出了「${copy(key)}」—— 那句话描述的是桌面「重启并安装」那条腿，Android 上不发生`,
      ).not.toContain(copy(key));
    }
    /*
     * `settings.update.downloadDone`（zh-CN「下载完成」）**只在应用更新那一行上**判：
     * 它是一个四字短串，而同屏「重新下载当前版本」那句常驻说明里就含着它
     * （「…下载完成后再由你确认安装」）—— 对整屏做子串否定判定会在一句**应该在**的文案上假红。
     * 桌面那四句描述的都是应用更新卡自己的态，故把射程收到那一行是对位的，不是把门磨钝。
     */
    const appUpdateRow = rows(MARKUP.update)
      .filter((r) => r.id === 'app-update')
      .map((r) => r.chunk)
      .join('');
    expect(appUpdateRow.length, '应用更新那一行切不出来 —— 上面那条否定判定会在空串上恒真').toBeGreaterThan(
      200,
    );
    expect(
      appUpdateRow,
      `应用更新那一行画出了「${copy('settings.update.downloadDone')}」—— 那是桌面 downloaded 态的话`,
    ).not.toContain(copy('settings.update.downloadDone'));
    /*
     * 正面断言（**否定式判据挡不住「整块没画」**）：下载那一跳的三件事各自在场。
     * 少了这一条，把整块删掉之后上面那四条否定断言照样全绿。
     */
    expect(MARKUP.update, '「重装当前版本」那颗不在').toContain(copy('settings.update.reinstallCurrent'));
    expect(MARKUP.update, '「自动下载新版本」那一格不在').toContain(copy('settings.update.autoDownloadApp'));
    expect(rows(MARKUP.update).map((r) => r.id)).toContain('auto-download-update');
  });

  it('备份：类目选择在，导出/导入两颗按钮真的可点（W-18 已接线）', () => {
    expect(switchRows(MARKUP.backup)).toEqual([...EXPECTED_SWITCHES.backup]);
    // 说明那句话改成了实话：说的是「走哪个系统面板」，不再是「这条腿没接」。
    expect(MARKUP.backup).toContain(copy('mobileSettings.backup.pickerNote'));
    expect(MARKUP.backup).toContain(copy('settings.backup.exportSelected'));
    expect(MARKUP.backup).toContain(copy('settings.backup.import'));
    /*
     * 🔴 判据是「**这两颗按钮**不再被写死禁用」，不是「整页没有 disabled 字样」——
     * 后者会在将来任何一颗按运行期状态置灰的控件上假红。夹具里 `selected` 初值是全选、
     * `busy` 是 false ⇒ 两颗都应渲染成可点。
     */
    const actions = rows(MARKUP.backup).find((r) => r.id === 'backup-actions');
    expect(actions, '导出/导入那一行必须在场（`commit` 的行内红字按它定位）').toBeDefined();
    expect(actions?.chunk).not.toContain('disabled');
  });

  it('通知换了载体但没丢掉「权限可能没给」这个新状态（§4.3）', () => {
    expect(MARKUP.display).toContain(copy('mobileHelp.notifications'));
    expect(switchRows(MARKUP.display)).toEqual([...EXPECTED_SWITCHES.display]);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
describe('⑩ 写失败必须可见：每一个写操作都有行内回显（裁定 #14）', () => {
  /** 写操作只在这六页；备份与关于不写 config。 */
  const WRITING_PAGES: readonly MobileSettingsPageId[] = [
    'general',
    'display',
    'network',
    'dns',
    'tun',
    'update',
  ];

  /**
   * 每一页上「会写 config 的行」。
   *
   * 这张表不是手抄的清单：下面第一条用**源码解析**出同一个集合再逐值对拍 —— 表与树两边互为对方的账，
   * 少登记一行、或多一行写腿没登记，两个方向都会红。
   */
  const WRITE_ROWS: Record<string, readonly string[]> = {
    // `boot-auto-connect` = 跳系统 VPN 设置页（W-17，桥在场时那一行是可点的）；
    // `auto-privacy-mode` / `privacy-password` = 隐私锁整组（W-16）。三条都是**会失败**且
    // 用户在等回执的写腿：跳不过去、密码在锁屏中被 `PRIVACY_LOCKED` 拒、开关落盘失败，
    // 每一条都必须落在这一行下面的红字上。
    general: [
      'auto-connect',
      'auto-check-update',
      'auto-privacy-mode',
      'auto-start',
      'boot-auto-connect',
      'disable-log-file',
      'privacy-password',
    ],
    display: ['theme', 'language', 'notifications'],
    network: [
      'direct-interface',
      'proxy-interface',
      'speed-test-url',
      'block-quic',
      'webrtc-leak',
      'tls-fragment',
      'auto-switch-node',
      'mesh-login-fallback',
      'interrupt-on-switch',
      'restart-on-node-change',
      'main-session-via-proxy',
    ],
    dns: [
      'connection-resolution',
      'fake-ip',
      'remote-dns',
      'domestic-dns',
      'dns-timeout',
      'takeover-system-dns',
      'optimistic-cache',
      'race-strategy',
      'node-resolver-single',
      // 竞速池三颗 + 两颗布尔开关（⑫）：写腿都在，故失败回显也必须都在。
      'race-pool-ali',
      'race-pool-dnspod',
      'race-pool-system',
      // 三张清单编辑器（2026-09-13 / 批 10 接通）。它们与上面那些开关一样**会失败且用户在等回执**
      // ——落盘失败时清单弹回上一版，不报就是「删了又回来了」。
      'race-custom-list',
      'fakeip-filter-list-editor',
      'browser-doh-list-editor',
      'fakeip-filter',
      'block-browser-doh',
      // v2 默认 DNS 策略两格（2026-09-13 / 批 17 接通，`DnsDefaultsRows`）。它们与本页其余行
      // 一样走 `commit(rowId, update(…))`：落盘失败时那一档弹回上一版，不报就是「选了又跳回去」。
      // ⚠️ 两行都只在 v2 夹具下渲染（`DNS_V2_TOP`），`dns-default-fallback` 还要 `hostsFirst` 那一档。
      'dns-default-action',
      'dns-default-fallback',
    ],
    // `tun-inbound-exclude` = 连入来源排除的网段清单（2026-09-06 接线）。它与本页其余行一样
    // 走 `commit(rowId, update(...))`，故失败回显与它们同一条通道。
    tun: [
      'tun-mtu',
      'tun-auto-route',
      'tun-nat-type',
      'tun-ipv6',
      'tun-inbound-exclude',
      // 「绕过局域网」开关与清单（2026-09-25，A6）：同走 `commit(rowId, update(...))`。
      'tun-bypass-lan',
      'tun-bypass-lan-list',
    ],
    // `app-update` = 检查更新 + 打开发布页 + 跳过此版本 + 下载 + 交系统安装器五条腿
    // （前两条 W-19，跳过 2026-09-13，后两条批 15）。
    // 它们不写一个字节的 config，但会失败、且用户正盯着那颗按钮等回执 —— 与关于页那四条外链
    // 同一类，故同样走 `commit`。三条共用一个行 id 是因为它们贴的是**同一行**：红字该落在
    // 那一行下面，用三个不同的行 id 会让其中两句渲染不出来（`SettingsRow` 按 id 自取）。
    // `update-channel` = 应用更新通道（本批接线）：它是真的写 config 的那一条。
    update: [
      'app-update',
      // `app-reinstall` = 「重装当前版本」（批 15）：一次 `includeCurrent` 检查 + 一次下载，
      // 两步都会失败且用户正盯着那颗按钮等回执。它**不与 `app-update` 共用行 id**：
      // 它是自己那一行的动作，红字该落在它下面。
      'app-reinstall',
      // `auto-download-update` = 自动下载新版本（批 15）：真的写 config 的那一条。
      'auto-download-update',
      'update-channel',
      'update-interval',
      'sub-auto-on-start',
      'sub-channel',
      'rule-resource-auto',
      'gh-proxy',
      'gh-custom-domain',
    ],
  };

  /**
   * 写腿入口**不写死清单，从源码推**。
   *
   * 一个函数是「中转」的判据：它把自己的某个**形参**原样交给 `commit`（或交给另一个已认定的中转）
   * 的首参位。从 `commit` 出发做不动点闭包 ⇒ `commitDns → patchDns → commit` 这种两跳链也认得出。
   *
   * 为什么不写死清单：写死的那一版活了三分钟就烂了 —— 加一个 `commitDns` 中转，解析面当场少两行写腿，
   * 而「登记表与源码对拍」那条会红在登记表上，把人引向改表而不是改判据。让判据自己跟着代码走。
   */
  function relaysOf(page: string): string[] {
    const src = stripComments(readFileSync(join(HERE, pageFile(page)), 'utf8'));
    const fns = [...src.matchAll(/function\s+(\w+)\s*\(([^)]*)\)/g)].map((m) => {
      const from = m.index ?? 0;
      const stop = src.indexOf('\n  }', from);
      return { name: m[1], params: m[2], body: src.slice(from, stop === -1 ? from + 1500 : stop) };
    });
    const relays = new Set(['commit']);
    for (let pass = 0; pass < 4; pass += 1) {
      for (const fn of fns) {
        if (relays.has(fn.name)) continue;
        for (const r of relays) {
          const call = new RegExp(`\\b${r}\\(\\s*([A-Za-z]\\w*)\\s*,`).exec(fn.body);
          if (call && new RegExp(`\\b${call[1]}\\b`).test(fn.params)) {
            relays.add(fn.name);
            break;
          }
        }
      }
    }
    return [...relays];
  }

  /** 页源码里所有「带行 id 的写腿」的首参字面量（入口集合由上面推出来）。 */
  const siteIdsOf = (page: string): string[] => {
    const src = stripComments(readFileSync(join(HERE, pageFile(page)), 'utf8'));
    const re = new RegExp(`\\b(?:${relaysOf(page).join('|')})\\(\\s*'([a-z0-9-]+)'`, 'g');
    return [...new Set([...src.matchAll(re)].map((m) => m[1]))].sort();
  };

  it('自检：登记表非空且有量级（表被清空会让下面几条全部空跑）', () => {
    const total = WRITING_PAGES.reduce((n, p) => n + WRITE_ROWS[p].length, 0);
    expect(total).toBeGreaterThanOrEqual(35);
  });

  it('自检：中转闭包认得出已知的那几个（认不出会让写腿少解析、登记表被引向改表）', () => {
    const all = new Set(WRITING_PAGES.flatMap((p) => relaysOf(p)));
    for (const name of ['commit', 'patchDns', 'commitDns', 'patchTun', 'setInterface', 'toggleRow']) {
      expect(all, `中转 \`${name}\` 没被认出来`).toContain(name);
    }
  });

  it('登记表 = 源码里解析出的写腿行 id（两个方向都说话）', () => {
    for (const page of WRITING_PAGES) {
      expect(siteIdsOf(page), `${page} 页的写腿与登记表对不上`).toEqual([...WRITE_ROWS[page]].sort());
    }
  });

  it('每一处 `update(` 都挂在 `commit(` 上（不许有裸写：写失败会静默）', () => {
    for (const page of WRITING_PAGES) {
      const src = stripComments(readFileSync(join(HERE, `${pageFile(page)}`), 'utf8'));
      const calls = [...src.matchAll(/\bupdate\(/g)];
      expect(calls.length, `${page} 页一个写调用都没解析到 —— 判据面塌了`).toBeGreaterThan(0);
      for (const m of calls) {
        const before = src.slice(Math.max(0, (m.index ?? 0) - 140), m.index);
        // 两种合法形态：① 直接是 `commit(…, update(…))` 的实参；
        // ② `(v) => update({ … })` 这个**闭包**，它被交给同文件里的 `toggleRow`，由那一处统一 commit。
        const wrapped = /commit\(\s*(?:'[a-z0-9-]+'|[A-Za-z]\w*)\s*,\s*$/.test(before);
        const thunk = /\(v\)\s*=>\s*$/.test(before) && src.includes('commit(id, patch(v))');
        // ③ 「先写执行侧、成功后落配置」的链：`commit('<id>', xxxApi.set(v).then(() => update(…)))` ——
        // 整条链是 commit 的实参，任一步 reject 都落到同一行红字（`auto-start` 那一行，⑤b）。
        const chained =
          /commit\(\s*'[a-z0-9-]+'\s*,\s*[A-Za-z]\w*(?:\.\w+)*\([^()]*\)\.then\(\(\)\s*=>\s*$/.test(before);
        expect(
          wrapped || thunk || chained,
          `${page} 页有一处 \`update(\` 没有接失败回显（片段：…${before.slice(-50)}update( ）`,
        ).toBe(true);
      }
    }
  });

  /*
   * 🔴 **把 `commit` 喂进子组件的那一跳**（2026-09-06 复审 major）。
   *
   * 上面那条「登记表 = 源码里解析出的写腿行 id」按 `commit('<行 id>'` 的**字面形态**扫源码，而
   * `privacy-password` 那条写腿住在子组件 `PrivacyPasswordRow` 体内、`commit` 是它的一个 **prop**。
   * 容器把这个 prop 换成惰性函数（`commit={() => undefined}`）时字面量仍在文件里，下面那条注入式
   * 渲染断言走的又是 `WriteErrorsContext` 而不是 `commit` ⇒ 整组无感（实测全量 4300 条全绿）。
   * 那一档的真机表现：保存锁屏密码的失败**整条静默** —— 后端在锁屏中以 `PRIVACY_LOCKED` 拒、
   * `setPassword` 返 `success:false`、IPC 断，三者一律不落红字，用户以为密码设好了（裁定 #14）。
   */
  const feedsCommitToChild = (src: string, child: string): boolean =>
    new RegExp(`<${child}[^>]*\\bcommit=\\{commit\\}`).test(src);

  it('🔴 带写腿的子组件真的拿到本屏那条 `commit`（不是一个惰性函数）', () => {
    // ⓪ 自检：谓词正反都判得对（否则下面那条断言是空话）。
    expect(feedsCommitToChild('<PrivacyPasswordRow commit={commit} />', 'PrivacyPasswordRow')).toBe(
      true,
    );
    expect(
      feedsCommitToChild('<PrivacyPasswordRow commit={() => undefined} />', 'PrivacyPasswordRow'),
      '把 `commit` 换成惰性函数却没被认出来 —— 下面那条断言恒绿',
    ).toBe(false);

    const src = stripComments(readFileSync(join(HERE, 'GeneralPage.tsx'), 'utf8'));
    expect(
      feedsCommitToChild(src, 'PrivacyPasswordRow'),
      '锁屏密码行拿到的不是本屏那条 `commit` —— 保存失败整条静默（后端在锁屏中会以 ' +
        '`PRIVACY_LOCKED` 拒），而 ⑩ 组其余每一条照样全绿',
    ).toBe(true);
  });

  it('注入一条失败，那一行就真的渲染出行内提示（逐行、逐页）', () => {
    const MARKER = 'WRITE-FAIL-MARKER';
    for (const page of WRITING_PAGES) {
      const ids = WRITE_ROWS[page];
      const injected = Object.fromEntries(ids.map((id) => [id, MARKER]));
      /*
       * DNS 页要**四份**夹具叠起来才盖得全（都是互斥分档，见 `dnsMarkup` 头注）：
       *  · 缺省档（单上游 + fakeIpFilter 开 + blockBrowserDoh 关）；
       *  · 竞速档 —— 池那三颗开关与自定义 DoH 清单只在这一档出现；
       *  · `blockBrowserDoh` 打开 —— 那张 DoH 端点清单只在开关打开时渲染（关着时渲染它，
       *    就是给用户一张编辑得动却不生效的名单）；
       *  · v2 默认策略档（2026-09-13 / 批 17）—— 那两格在 schema v1 下整块不画，
       *    而 `CONFIG` 固定停在 v1。缺这一份，`dns-default-*` 两行会被读成「没接失败回显」。
       * 少任何一份，那一档独有的行都会被读成「没接失败回显」。
       */
      const markup =
        render(pageOf(page), injected) +
        (page === 'dns'
          ? dnsMarkup({ resolveNodeDomainsAhead: true }, {}, injected)
            + dnsMarkup({}, { blockBrowserDoh: true }, injected)
            + dnsMarkup({}, DNS_V2_TOP, injected)
          : '');
      // 去重：DNS 那两档共有的行会各出现一次，判的是**集合**恰等，不是出现次数。
      const shown = [
        ...new Set([...markup.matchAll(/data-write-error="([^"]+)"/g)].map((m) => m[1])),
      ].sort();
      expect(shown, `${page} 页有写腿的行没能显示失败提示`).toEqual([...ids].sort());
      // 正面：提示文字真的进了 DOM，不是只有个空壳属性。
      expect(markup).toContain(MARKER);
    }
  });

  it('不注入时一条提示都不渲染（免得它变成常驻红字）', () => {
    for (const page of WRITING_PAGES) {
      expect(MARKUP[page]).not.toContain('data-write-error=');
    }
  });

  it('`commit` 在写失败时**真的记下了东西**（「有 catch 但 catch 体是空的」在这里现形）', async () => {
    let state: WriteErrors = {};
    const commit = createCommit((updater) => {
      state = updater(state);
    }, i18n.t.bind(i18n));

    commit('auto-connect', Promise.reject(new Error('端口被占用')));
    await new Promise((r) => setImmediate(r));
    expect(Object.keys(state), '写失败后错误表仍然是空的 —— 失败被吞了').toEqual(['auto-connect']);
    // 原因要带出来（只说「保存失败」等于把用户能据以行动的那半句丢了）。
    expect(state['auto-connect']).toContain('端口被占用');

    // 成功要把它清掉，否则那行红字会一直挂着。
    commit('auto-connect', Promise.resolve());
    await new Promise((r) => setImmediate(r));
    expect(state).toEqual({});
  });

  it('失败原因只取第一行且截断（后端的多行细节/调用栈不外露）', async () => {
    let state: WriteErrors = {};
    const commit = createCommit((updater) => {
      state = updater(state);
    }, i18n.t.bind(i18n));
    commit('x', Promise.reject(new Error(`第一行\n    at foo (bar.rs:1)\n${'y'.repeat(400)}`)));
    await new Promise((r) => setImmediate(r));
    expect(state['x']).toContain('第一行');
    expect(state['x']).not.toContain('at foo');
    expect(state['x'].length).toBeLessThan(160);
  });

  it('交给各页的 `update` 是**会抛的那一版**（不抛就没有失败可接）', () => {
    const screen = stripComments(readFileSync(join(HERE, 'MobileSettingsScreen.tsx'), 'utf8'));
    expect(screen).toMatch(/update\(patch,\s*\{\s*throwOnError:\s*true\s*\}\)/);
    expect(screen).toContain('update={strictUpdate}');
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ⑪ **移动端不再从 `config.proxyModeType` 取分支**（首页那颗接管方式芯片整颗移除的另一半）。
 *
 * # 为什么这是一条判据，而不是「顺手清理」
 *
 * Android 恒 TUN（三档里另外两档靠的 `mixed-in` 入站在该平台根本不发，见
 * `crates/config-engine/src/builder/inbounds.rs`），而盘上那个字段的默认值是 `systemProxy`
 * （`user_config/app_config.rs`）。首页那颗控件删掉之后，移动端**再也没有任何入口能改它** ⇒
 * 谁还在读它，谁就恒定读到 `systemProxy`：
 *
 *  · DNS 页的「关掉 FakeIP 有风险」那句话（原判 `needsFakeIpOffConfirm(false, proxyModeType)`）
 *    会在唯一需要它的平台上**永不出现**；
 *  · TUN 页的 IPv6 / FakeIP 联动提示同理；
 *  · 网络页的 `webrtcTunOnlyNote` 反过来会**恒不出现**，而那句话的措辞是「在主页把接管方式切到
 *    TUN 后可用」—— 主页已经没有那颗控件了，它指向一个不存在的操作。
 *
 * 所以这一组是「恒真/恒假的分支被拆掉了」的正面账：**夹具刻意喂 `systemProxy`**（= 全新安装的
 * 真实盘上值），TUN 专属的那几行仍然必须渲染出来。
 */
describe('⑪ 设置页不再读 `proxyModeType`：TUN 专属的行在错的值下照样渲染', () => {
  /** 夹具确实喂的是那个「错的」值 —— 它一旦被改回 `'tun'`，下面几条就全成了恒绿。 */
  it('自检：夹具喂的是全新安装的盘上值 `systemProxy`，不是 `tun`', () => {
    expect(CONFIG.proxyModeType).toBe('systemProxy');
    // 回放：桌面那条腿在这份夹具下**判否** —— 移动端若照读它，下面那两句话就一个都不会出现。
    expect(needsFakeIpOffConfirm(false, CONFIG.proxyModeType)).toBe(false);
  });

  it('源码面：八页一处都不出现 `proxyModeType`', () => {
    expect(
      SOURCE_TEXT,
      '移动端设置页又在读 `proxyModeType` —— 它在这个平台上恒为 `systemProxy` 且改不了，' +
        '读它等于把 TUN 专属的解释永久关掉',
    ).not.toContain('proxyModeType');
    // 正面对照：同一个取材面确实读着别的 config 字段（否则上面那条是对空文件说的）。
    expect(SOURCE_TEXT).toContain('enableIPv6');
    expect(SOURCE_TEXT).toContain('webrtcLeakProtection');
  });

  it('DNS：关掉 FakeIP 的风险提示照样常驻（这是它唯一的可见通道）', () => {
    const row = rows(MARKUP.dns).find((r) => r.id === 'fake-ip');
    expect(row, 'FakeIP 那一行不见了 —— 判据面塌了').toBeDefined();
    expect(row?.chunk, 'FakeIP 风险提示没渲染').toContain(copy('mobileSettings.dns.fakeIpOffRisk'));
  });

  it('TUN：IPv6 / FakeIP 联动提示照样出得来（同一份 `systemProxy` 配置）', () => {
    // 这一条要 FakeIP **关着**才成立，夹具里它是开的 ⇒ 单独配一份，其余键原样。
    const cfg = {
      ...CONFIG,
      dnsConfig: { ...CONFIG.dnsConfig, enableFakeIp: false },
    } as unknown as UserConfig;
    const html = render(
      createElement(TunPage, { config: cfg, update: async () => undefined, commit: () => undefined }),
    );
    expect(html, 'IPv6 联动提示没渲染 —— 它只在这一行有可见通道').toContain(
      copy('settings.network.ipv6NodeFakeIpHint'),
    );
    // 反向对照：FakeIP 开着时它**不**出现（证明上一条不是恒真）。
    expect(MARKUP.tun).not.toContain(copy('settings.network.ipv6NodeFakeIpHint'));
  });

  it('网络：`webrtcTunOnlyNote` 一处不出现，但 WebRTC 那一行本身还在且可切', () => {
    const surface = allMarkup() + '\n' + ROOT_MARKUP;
    expect(
      surface,
      '又渲染出了「仅 TUN 模式生效——在主页把接管方式切到 TUN 后可用」—— 主页已经没有那颗控件了',
    ).not.toContain(copy('settings.network.webrtcTunOnlyNote'));
    // 正面：整行没有被连坐删掉（那同样满足上面那条否定断言）。
    const row = rows(MARKUP.network).find((r) => r.id === 'webrtc-leak');
    expect(row, 'WebRTC 防泄露那一行不见了 —— 缺席不是这一条的处置').toBeDefined();
    expect(row?.chunk).toContain(copy('settings.network.webrtcLeakProtection'));
    expect(row?.chunk).toContain(copy('settings.network.webrtcLeakOff'));
    expect(row?.chunk).toContain(copy('settings.network.webrtcLeakBlock'));
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ⑫ DNS 的三块能力**不再整块静默缺席**（IA §3.3 规则 2 + §4 开头那句判据）。
 *
 * 缺的从来不只是三张 `ListEditor`：`fakeIpFilter`（缺省开）与 `blockBrowserDoh` 是两个**布尔字段**，
 * 桌面拨过就一直在配置里生效，而手机上此前既看不到、也改不了、页面上还没有一句话说它去哪了 ——
 * 「present in the configuration, invisible in the UI, and inert at runtime is a defect」。
 * 竞速池同理：`nodeResolverPool` 是活字段，看不见就等于那台设备上永远只能用别处设好的值。
 *
 * 本组按**渲染出来的 DOM** 判，且每条都带反向对照（缺省态与显式态各看一次），
 * 免得「什么都没渲染出来」把断言骗过去。
 */
describe('⑫ DNS：三块能力补齐，清单缺席带常驻理由（§3.3 规则 2）', () => {
  const rowOf = (markup: string, id: string): string => {
    const row = rows(markup).find((r) => r.id === id);
    expect(row, `markup 里没有行 \`${id}\` —— 判据面塌了`).toBeDefined();
    return row?.chunk ?? '';
  };

  it('自检：另一份夹具真的渲染得出来，且 race 档确实被翻到了「竞速」', () => {
    const race = dnsMarkup({ resolveNodeDomainsAhead: true });
    expect(race.length).toBeGreaterThan(400);
    // 单上游那颗下拉只在 race 关时出现；它在这份夹具里必须不在，否则下面几条测的是同一个态。
    expect(rows(race).map((r) => r.id)).not.toContain('node-resolver-single');
    expect(rows(MARKUP.dns).map((r) => r.id)).toContain('node-resolver-single');
  });

  it('开关集合逐值全等：两颗布尔开关在，且没多画别的（race 关）', () => {
    expect(switchRows(MARKUP.dns)).toEqual([...EXPECTED_SWITCHES.dns]);
  });

  it('两个默认口径按后端来：`fakeIpFilter` 缺省=开，`blockBrowserDoh` 缺省=关', () => {
    // 夹具里两个字段都没写 ⇒ 走的就是缺省那一支。
    expect(rowOf(MARKUP.dns, 'fakeip-filter')).toContain('aria-checked="true"');
    expect(rowOf(MARKUP.dns, 'block-browser-doh')).toContain('aria-checked="false"');
    // 反向对照：显式值真的读得进来（否则上面两条可能只是恒真）。
    const flipped = dnsMarkup({}, { fakeIpFilter: false, blockBrowserDoh: true });
    expect(rowOf(flipped, 'fakeip-filter')).toContain('aria-checked="false"');
    expect(rowOf(flipped, 'block-browser-doh')).toContain('aria-checked="true"');
  });

  it('竞速池三颗上游只在 race 档出现，且逐值反映 `nodeResolverPool`', () => {
    const race = dnsMarkup({ resolveNodeDomainsAhead: true });
    expect(switchRows(race)).toEqual([
      'fake-ip',
      'takeover-system-dns',
      'optimistic-cache',
      'race-pool-ali',
      'race-pool-dnspod',
      'race-pool-system',
      'fakeip-filter',
      'block-browser-doh',
    ]);
    // 缺省池 = ali + dnspod，`system` 不在里面。
    expect(rowOf(race, 'race-pool-ali')).toContain('aria-checked="true"');
    expect(rowOf(race, 'race-pool-system')).toContain('aria-checked="false"');
    // 反向对照：换一份池，三颗跟着换（不是画死的）。
    const onlySystem = dnsMarkup({ resolveNodeDomainsAhead: true, nodeResolverPool: ['system'] });
    expect(rowOf(onlySystem, 'race-pool-ali')).toContain('aria-checked="false"');
    expect(rowOf(onlySystem, 'race-pool-system')).toContain('aria-checked="true"');
    // race 关时它们整组不在（那一档没有池这个概念）。
    expect(switchRows(MARKUP.dns)).not.toContain('race-pool-ali');
  });

  it('🔴 桌面把 3 条自定义 DoH 加满时，内置那两颗按满额禁掉并说出原因（不禁 = 拨了不生效）', () => {
    const custom = ['c1', 'c2', 'c3'].map((id) => ({ id, spec: `https://10.0.0.${id.slice(1)}/dns-query` }));
    const full = dnsMarkup({
      resolveNodeDomainsAhead: true,
      nodeResolverPool: ['c1', 'c2', 'c3'],
      nodeResolverCustom: custom,
    });
    expect(rowOf(full, 'race-pool-ali')).toContain('disabled=""');
    expect(rowOf(full, 'race-pool-ali')).toContain(copy('settings.dns.raceQuotaReached'));
    // 兜底层不占额度 ⇒ 满额也不该被禁。
    expect(rowOf(full, 'race-pool-system')).not.toContain('disabled=""');
    // 反向对照：额度没满时不禁（否则上面那条对任何输入都成立）。
    const free = dnsMarkup({ resolveNodeDomainsAhead: true });
    expect(rowOf(free, 'race-pool-ali')).not.toContain('disabled=""');
    expect(free).not.toContain(copy('settings.dns.raceQuotaReached'));
  });

  /**
   * 缺席理由的**门控口径**（K5-RV05）。
   *
   * 桌面 `SettingsDns.tsx:656` 与 `:698` 两处都写死了同一条原则并给了理由：「关闭时不渲染清单：
   * 不生效的可编辑清单是误导」。移动端没有清单可画，但那两条**描述清单**的说明与「去桌面端改」
   * 的缺席理由是同一块的东西 —— 总开关关着时它们描述的是一张不生效的名单。
   *
   * 故这条判据必须在**开着**的夹具上断言「理由在」，并配一份**关着**的夹具断言「整块不挂」：
   * 只测一档会把两个方向各读反一半 —— `blockBrowserDoh` 缺省是关的，在缺省夹具上断言「理由在」
   * 等于要求全新安装的手机常驻一段描述不生效清单的说明。
   */
  it('🔴 三张清单**都编辑得了**（2026-09-13 接通，缺席理由随之撤销）', () => {
    const on = dnsMarkup(
      { resolveNodeDomainsAhead: true },
      { fakeIpFilter: true, blockBrowserDoh: true },
    );
    /*
     * 上一版这条断言钉的是「三块清单缺席都带常驻理由」。批 10 把三张清单接上了
     * （`MobileListEditor`，去重与草稿规则读 `@/domain/list-entries`，与桌面同一份），
     * 那三句理由随之作废 —— 留着它们就是三句假话。判据翻面：**编辑器在场**，理由不在场。
     */
    for (const row of ['race-custom-list', 'fakeip-filter-list-editor', 'browser-doh-list-editor']) {
      expect(rows(on).map((r) => r.id), `清单编辑器不在场：${row}`).toContain(row);
    }
    for (const note of ['fakeip-filter-list-absent', 'browser-doh-list-absent', 'race-custom-absent']) {
      expect(on, `清单已经能编辑了，那句缺席理由还挂着：${note}`).not.toContain(`data-note="${note}"`);
    }
    /* 🔴 2026-09-13（批 17）：原来这里还有一条 `not.toContain(copy('mobileRules.formUnavailable'))`。
       那条键已经从五份 locale 里删掉了（DNS 默认策略接通后它是死键），`copy()` 取不到会**抛**，
       而它抛出来的错与「文案又冒出来了」读起来是两回事。同一件事的正面判据搬到了下面
       ⑫quater 那一组：那两格是真控件、且本页一条缺席理由都不剩。 */
    // 正面：解释这三块**是什么**的那几句仍在（有了编辑器也仍要说清编辑的是什么）。
    expect(on).toContain(copy('settings.dns.fakeIpFilterHint'));
    expect(on).toContain(copy('settings.dns.browserDohFold'));
    expect(on).toContain(copy('mobileHelp.raceUpstreams'));
  });

  it('🔴 总开关关着时那两块的说明整块不挂（不生效的清单说明是误导，与桌面同口径）', () => {
    const off = dnsMarkup(
      { resolveNodeDomainsAhead: true },
      { fakeIpFilter: false, blockBrowserDoh: false },
    );
    for (const note of ['fakeip-filter-list', 'browser-doh-list']) {
      expect(off, `总开关关着却还挂着清单说明：${note}`).not.toContain(`data-note="${note}"`);
    }
    /* 清单**编辑器**同理跟着总开关走：一张编辑得动却不生效的名单比一段说明更误导。 */
    for (const row of ['fakeip-filter-list-editor', 'browser-doh-list-editor']) {
      expect(rows(off).map((r) => r.id), `总开关关着却还画着清单编辑器：${row}`).not.toContain(row);
    }
    // 正面：开关行本身**不许**跟着消失 —— 那就退回 K5-04 修掉的那个「整块静默缺席」了。
    expect(rows(off).map((r) => r.id)).toContain('fakeip-filter');
    expect(rows(off).map((r) => r.id)).toContain('block-browser-doh');
    expect(rowOf(off, 'fakeip-filter')).toContain('aria-checked="false"');
    // 反向对照：竞速那块与这两颗开关无关，它的清单编辑器照在 —— 否则上面几条可能只是「什么都没渲染」。
    expect(rows(off).map((r) => r.id)).toContain('race-custom-list');
  });

  it('🔴 缺省安装上不常驻一段描述不生效清单的说明（`blockBrowserDoh` 缺省是关的）', () => {
    // 缺省夹具：`fakeIpFilter` 未设 = 开、`blockBrowserDoh` 未设 = 关。
    expect(MARKUP.dns).toContain('data-note="fakeip-filter-list"');
    expect(MARKUP.dns).not.toContain('data-note="browser-doh-list"');
    /* 编辑器同样跟着走：缺省关的那一块连编辑器一起不画。 */
    expect(rows(MARKUP.dns).map((r) => r.id)).toContain('fakeip-filter-list-editor');
    expect(rows(MARKUP.dns).map((r) => r.id)).not.toContain('browser-doh-list-editor');
  });

  /**
   * 复用桌面文案时，**祈使句不能跟着搬过来**（K5-RV04）。
   *
   * 承诺的真值在**用户可见文案**里，不从机制反推：`settings.dns.browserDohHint` 是写给能编辑清单的
   * 客户端的，以「遇到漏网的**自行添加**」收尾。本屏没有添加入口，而紧接着的缺席理由又说
   * 「请在桌面端修改后同步」—— 一句让你去做、下一句告诉你做不了。故它换成只**命名**那张清单的
   * `browserDohFold`（无动作指令）。
   *
   * 对照留在这里当口径：`settings.dns.fakeIpFilterHint` 是纯描述、不含祈使 ⇒ 原样保留；
   * `settings.dns.raceUpstreamsDesc` 陈述的是配置本身的容量与额度上限（且那个「最多 3 个」在别处
   * 没有第二个落点），也不是祈使 ⇒ 保留。这张表登记的只是**含动作指令**的那几句。
   */
  /*
   * 🔴 **这张表 2026-09-13 清空了，`browserDohHint` 那条禁令随之撤销。**
   *
   * 那条禁令的前提是「本屏没有添加入口」，而批 10 把清单编辑器接上了 ⇒ 那句祈使
   * （「遇到漏网的**自行添加**」）在这台设备上**兑现得了**，它不再是一句做不到的承诺。
   * 承诺的真值在用户可见文案里，而现在文案与能力对得上。
   *
   * 表留着不删：它是这条口径的落点 —— 下一次复用一句**含动作指令**的桌面文案而移动端做不到时，
   * 往这里加一行，下面那条否定断言就把它钉住。空表时下面的自检要求它**证明自己没塌**。
   */
  const DESKTOP_ONLY_COPY: readonly [string, string][] = [];

  it('自检：这张表空着也不许让下面那条恒绿（谓词与取材面都要证明自己是活的）', () => {
    /* 谓词是活的：拿一句**已知含祈使**的桌面文案喂它，必须认得出来。 */
    expect(copy('settings.dns.browserDohHint')).toContain('自行添加');
    /* 取材面是活的：那一块真的在屏上（否则下面那条否定断言只是在说「什么都没渲染出来」）。 */
    const on = dnsMarkup({}, { blockBrowserDoh: true });
    expect(on).toContain(copy('settings.dns.browserDohTitle'));
    expect(on).toContain('data-note="browser-doh-list"');
    /* 且那句祈使现在**确实**渲染着 —— 它与「本屏能添加」是一对，任何一半没了都要回来重判。 */
    expect(on, '那句祈使不在屏上了 ⇒ 上面撤销禁令的前提要重核').toContain(
      copy('mobileHelp.browserDohList'),
    );
    expect(rows(on).map((r) => r.id), '添加入口不在了，而祈使还挂着').toContain(
      'browser-doh-list-editor',
    );
  });

  it('🔴 承诺了移动端做不到的动作的那几句一处都不渲染', () => {
    const surface = [
      allMarkup(),
      ROOT_MARKUP,
      dnsMarkup({ resolveNodeDomainsAhead: true }, { fakeIpFilter: true, blockBrowserDoh: true }),
    ].join('\n');
    const leaked = DESKTOP_ONLY_COPY.filter(([key]) => surface.includes(copy(key))).map(
      ([key, why]) => `${key}（${why}）`,
    );
    expect(leaked, '这几句桌面文案漏进了移动端').toEqual([]);
  });

  /**
   * 额度用满的那句话**不是错误态**（K5-RV06）。
   *
   * 桌面把同一句放在 `Switch` 的 `tip` 里（`SettingsDns.tsx:564/576`），非错误。移动端此前走
   * `SettingsRow` 的 `problem` 槽 ⇒ 渲染成 `hsl(var(--err))` 红字，与同页「DNS 地址非法」
   * 「超时值越界」同一视觉等级：桌面加满 3 条自定义 DoH 的用户打开手机会以为配置坏了。
   */
  it('🔴 满额提示走非错误色（与「DNS 地址非法」不是同一视觉等级）', () => {
    const custom = ['c1', 'c2', 'c3'].map((id) => ({ id, spec: `https://10.0.0.${id.slice(1)}/dns-query` }));
    const full = dnsMarkup({
      resolveNodeDomainsAhead: true,
      nodeResolverPool: ['c1', 'c2', 'c3'],
      nodeResolverCustom: custom,
    });
    const quota = copy('settings.dns.raceQuotaReached');
    const span = /<span data-hint="race-pool-ali" style="([^"]*)">([^<]*)<\/span>/.exec(full);
    expect(span, '满额提示不在 `hint` 槽里 —— 它要么没渲染，要么又回到了 `problem`').not.toBeNull();
    expect(span?.[2]).toBe(quota);
    expect(span?.[1], '满额提示被画成了错误色').not.toContain('--err');
    expect(span?.[1]).toContain('color:hsl(var(--fg-dim))');
    // 正面对照：`problem` 槽本身仍然画红（同页的 FakeIP 风险提示走的就是它）——
    // 否则上面那条只证明了「这一页压根没有红字」。
    expect(MARKUP.dns).toContain('color:hsl(var(--err))');
    expect(MARKUP.dns).toContain(copy('mobileSettings.dns.fakeIpOffRisk'));
    // 反向对照：额度没满时这一格整个不渲染。
    expect(dnsMarkup({ resolveNodeDomainsAhead: true })).not.toContain('data-hint="race-pool-ali"');
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ⑬ 开关的**可点区** = `--tap-min`（48），视觉一格不动。
 *
 * IA §3.1 #11 的原话是「48 × 48 hit area around an unchanged visual」，`core.json` 又把 touch target
 * 列进「不随 Dynamic Type 缩放」并给了理由「48 是手指，不是字母」。此前按钮自身就是 44×26 的轨道 ⇒
 * 命中盒只有规范的 54% 高，而行是 `<div>`、点空不响应，用户会以为开关卡住而反复点。
 *
 * 判据钉两头：命中盒必须引 `--tap-min`（引 token 而不是写死 48），轨道必须仍是 44×26（扩的是可点区，
 * 不是把开关画大）。
 */
describe('⑫bis DNS 上游预设下拉：两端读**同一张表**（2026-09-06 接线）', () => {
  /**
   * 上一版不移植的理由是「桌面那张表是组件私有函数，抄一份就是第二份真值源」。本批把表搬进
   * `settings-dns-logic.ts`（两端已经共用的纯逻辑模块），于是这一组要守两件事：
   *  ① 下拉真的画出来了、选中态跟着配置走、哨兵档永远不落库；
   *  ② 那张表在仓里**只有一份**——「抄一份」那条路必须继续堵着，否则移植等于把缺陷换了个形状。
   */
  const dnsSource = SOURCES.find((f) => f.file === 'DnsPage.tsx')?.text ?? '';

  it('自检：取材面在（否则下面的源码级断言是对空气说话）', () => {
    expect(dnsSource.length, 'DnsPage.tsx 没进取材面').toBeGreaterThan(2000);
  });

  it('两条地址行各有一颗预设下拉，四档预设 + 一档「自定义…」齐全', () => {
    for (const rowId of ['remote-dns', 'domestic-dns']) {
      const row = rows(MARKUP.dns).find((r) => r.id === rowId)?.chunk ?? '';
      expect(row, `${rowId} 那一行不见了`).not.toBe('');
      expect(row, `${rowId} 没有预设下拉`).toContain('<select');
      const options = [...row.matchAll(/<option value="([^"]*)"/g)].map((m) => m[1]);
      expect(options.length, `${rowId} 的档位数不对`).toBe(5);
      expect(options[options.length - 1], '最后一档必须是「自定义…」哨兵').toBe('__custom__');
    }
  });

  it('两端读的是同一张表：桌面那份预设逐值出现在移动端下拉里', () => {
    const remote = rows(MARKUP.dns).find((r) => r.id === 'remote-dns')?.chunk ?? '';
    const domestic = rows(MARKUP.dns).find((r) => r.id === 'domestic-dns')?.chunk ?? '';
    // 值取自共享模块本身（不是这里抄一份字面量 —— 那又是一份真值源）。
    for (const preset of remotePresets((k) => k)) expect(remote).toContain(preset.value);
    for (const preset of domesticPresets((k) => k)) expect(domestic).toContain(preset.value);
    // 两张表不许混：远程那颗里不该出现国内的上游。
    expect(remote).not.toContain('doh.pub');
    expect(domestic).not.toContain('cloudflare-dns.com');
  });

  it('选中态跟着配置走：命中预设 ⇒ 选中它；不在表里 ⇒ 停在「自定义…」', () => {
    // 夹具：foreignDns 是表内的 Cloudflare，domesticDns 是裸 IP（不在表内）。
    const remote = rows(MARKUP.dns).find((r) => r.id === 'remote-dns')?.chunk ?? '';
    const domestic = rows(MARKUP.dns).find((r) => r.id === 'domestic-dns')?.chunk ?? '';
    expect(remote).toContain('<option value="https://1.1.1.1/dns-query" selected="">');
    expect(domestic, '不在表内的值没落到「自定义…」档').toContain(
      '<option value="__custom__" selected="">',
    );
    // 正向对照：换成表内的值，哨兵档必须让位 —— 否则上一条只是「哨兵恒选中」。
    const inTable = dnsMarkup({ domesticDns: 'https://doh.pub/dns-query' });
    const flipped = rows(inTable).find((r) => r.id === 'domestic-dns')?.chunk ?? '';
    expect(flipped).toContain('<option value="https://doh.pub/dns-query" selected="">');
    expect(flipped).not.toContain('<option value="__custom__" selected="">');
    // 手输框仍显示同一个值（下拉与输入框是同一个真值，不是两份草稿）。
    expect(flipped).toContain('value="https://doh.pub/dns-query"');
  });

  it('哨兵永不落库：`__custom__` 那一档在写路径上被显式挡掉', () => {
    // 挡不住的话 `__custom__` 会被写进 dnsConfig，而 `parseDnsServerSpec` 解不出它 ⇒
    // DNS 段整块回落到兜底上游，界面上却还显示着「自定义」。
    expect(dnsSource).toMatch(/if\s*\(\s*value\s*===\s*DNS_PRESET_CUSTOM\s*\)\s*return;/);
    // 反向对照：切掉那一支，谓词必须转 false（证明它认的是这一行，不是随便一段文本）。
    const bypassed = dnsSource.replace(
      /if\s*\(\s*value\s*===\s*DNS_PRESET_CUSTOM\s*\)\s*return;/,
      '',
    );
    expect(bypassed).not.toBe(dnsSource);
    expect(/if\s*\(\s*value\s*===\s*DNS_PRESET_CUSTOM\s*\)\s*return;/.test(bypassed)).toBe(false);
    // 且哨兵字面量本身只在共享模块里定义一次，两个消费点都引常量而不是各写一遍。
    expect(dnsSource, '移动端自己又写了一遍哨兵字面量').not.toContain("'__custom__'");
  });

  it('🔴 表在仓里只有一份：任何生产文件里再出现第二处预设地址都算分叉', () => {
    // 取材面：`ui/src` 全部非测试 `.ts/.tsx`。判据用两条表里各挑一条**只可能出现在表里**的地址。
    const walk = (dir: string): string[] =>
      readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
        e.isDirectory() ? walk(join(dir, e.name)) : [join(dir, e.name)],
      );
    const production = walk(UI_SRC).filter(
      (f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f) && !/test-support/.test(f),
    );
    expect(production.length, '取材面塌了').toBeGreaterThan(100);
    for (const needle of ['cloudflare-dns.com/dns-query', 'dns.alidns.com/dns-query']) {
      const holders = production
        .filter((f) => stripComments(readFileSync(f, 'utf8')).includes(needle))
        .map((f) => f.slice(UI_SRC.length).replace(/^\//, ''));
      expect(
        holders,
        `「${needle}」出现在不止一个生产文件里 —— 预设表又分叉了（这正是上一版拒绝移植的那个理由）`,
      ).toEqual(['components/screens/settings/settings-dns-logic.ts']);
    }
    // 反向对照：谓词认得出「多了一处」——拿一条真的出现在两个文件里的字面量比一次。
    const twoPlaces = production.filter((f) =>
      stripComments(readFileSync(f, 'utf8')).includes('DNS_PRESET_CUSTOM'),
    );
    expect(
      twoPlaces.length,
      '连 `DNS_PRESET_CUSTOM` 都只在一个文件里 ⇒ 上面那条「恰好一个文件」可能只是走查没走到',
    ).toBeGreaterThan(1);
  });
});

describe('⑫ter 连入来源排除：配置里在、界面上也看得见了（2026-09-06 接线）', () => {
  /**
   * 这一块守的是 §4 开头点名的那个缺陷形态：**配置里在、界面上看不见、运行时照常生效**。
   * `tunConfig.inboundExcludeCidrs` 在 Android 上有活的消费方（`inbounds.rs:406` 只对 linux 短路，
   * 其余平台走 `:418` → `:614 tun.route_exclude_address`，承载侧 `PolarisVpnService.kt:131`
   * 逐条 `excludeRouteTolerantly`），而上一版移动端一处都改不到。
   *
   * ⚠️ 本组是**前端**判据：它证明「这一格画出来了、改得动、空表删键」，不证明真机上那条路由
   * 真的被排除了 —— 那归真机验收。
   */
  const tunWith = (cidrs?: string[]): string =>
    render(
      createElement(TunPage, {
        config: {
          ...CONFIG,
          tunConfig: {
            ...(CONFIG as unknown as { tunConfig: object }).tunConfig,
            ...(cidrs === undefined ? {} : { inboundExcludeCidrs: cidrs }),
          },
        } as unknown as UserConfig,
        update: async () => undefined,
        commit: () => undefined,
      }),
    );

  it('这一块在场：有自己的分组标题与一行清单编辑器', () => {
    expect(MARKUP.tun, '连入来源排除那一块没画出来').toContain(copy('settings.tun.inboundExcludeBlock'));
    expect(rows(MARKUP.tun).map((r) => r.id)).toContain('tun-inbound-exclude');
    expect(MARKUP.tun, '清单编辑器的锚点不在').toContain('data-list-editor="tun-inbound-exclude"');
  });

  it('桌面那句 `tip` 落成常驻说明（§4.12：触屏没有 hover）', () => {
    expect(MARKUP.tun).toContain(copy('settings.tun.inboundExcludeHint'));
  });

  it('清单逐条画出来，且是**可编辑**的输入框而不是只读文字', () => {
    const html = tunWith(['100.64.0.0/10', '10.9.0.0/16']);
    for (const cidr of ['100.64.0.0/10', '10.9.0.0/16']) {
      expect(html, `${cidr} 没画出来`).toContain(`value="${cidr}"`);
    }
    const editor = /data-list-editor="tun-inbound-exclude"([\s\S]*?)<\/div><\/div><\/div>/.exec(html);
    expect(editor, '清单编辑器切不出来').not.toBeNull();
    expect((html.match(/aria-label="CIDR"/g) ?? []).length, '两条网段该有两个输入框').toBe(2);
    // 每条各有一颗删除按钮，且带可读名（图标按钮没有可访问名 = 读屏读不出来）。
    expect((html.match(new RegExp(`aria-label="${copy('common.delete')}"`, 'g')) ?? []).length).toBe(2);
  });

  it('空表：不画输入框，画一句「还没加」，且那句话**不是**缺席声明', () => {
    const html = tunWith(undefined);
    expect(html).toContain(copy('settings.tun.inboundExcludeEmpty'));
    expect(html.match(/aria-label="CIDR"/g) ?? []).toEqual([]);
    // 添加与批量导入两颗按钮照常可用 —— 空表不是「这个功能没有」。
    expect(html).toContain(copy('settings.tun.addCidr'));
    expect(html).toContain(copy('common.bulkImport'));
    expect(html, '两颗按钮被禁用了 ⇒ 空表变成了死界面').not.toMatch(
      /<button[^>]*\sdisabled[^>]*>[^<]*(?:添加网段|批量导入)/,
    );
  });

  it('🔴 MTU 占位符只说「内核默认」、不写任何数字（Polaris 不持有平台 → MTU 表）', () => {
    /*
     * 用户留空 = 生成期不下发 `mtu`，由内核按运行环境取默认（上游 `inbound.go` 的 `MTU == 0` 分支）。
     * 占位符写任何具体数都是在替内核报一个值 —— 平台一不同（Android / iOS / 桌面）就是错的。
     */
    const row = rows(MARKUP.tun).find((r) => r.id === 'tun-mtu');
    expect(row, 'MTU 那一行不见了 —— 下面的否定断言会对空气说话').toBeDefined();
    const want = copy('settings.tun.mtuAutoPlaceholder');
    expect(row?.chunk, '占位符没渲染成「自动（内核默认）」').toContain(`placeholder="${want}"`);
    const hasNumber = (text: string): boolean => /\d/.test(text);
    expect(hasNumber(want), 'zh-CN 占位符文案里有数字').toBe(false);
    // 五种 locale 都不带插值、不带数字。
    for (const lang of ['zh-CN', 'zh-TW', 'en-US', 'ru', 'fa']) {
      const v = ((localeJson(lang).settings as Record<string, Record<string, string>>).tun ?? {})
        .mtuAutoPlaceholder;
      expect(typeof v, `${lang} 缺 mtuAutoPlaceholder`).toBe('string');
      expect(hasNumber(v) || v.includes('{{'), `${lang} 的占位符带数字或插值：${v}`).toBe(false);
    }
    // 反向对照：谓词对「自动（65535）」必须说话。
    expect(hasNumber('自动（65535）')).toBe(true);
  });

  it('🔴 空表**删键**而不是写 `[]`（两者在配置生成侧不是同一件事）', () => {
    const src = SOURCES.find((f) => f.file === 'TunPage.tsx')?.text ?? '';
    expect(src.length, 'TunPage.tsx 没进取材面').toBeGreaterThan(1000);
    expect(src, '清空清单时没有删键').toMatch(/delete\s+nextTun\.inboundExcludeCidrs;/);
    // 反向对照：切掉那一行，谓词必须转 false。
    const bypassed = src.replace(/delete\s+nextTun\.inboundExcludeCidrs;/, '');
    expect(bypassed).not.toBe(src);
    expect(/delete\s+nextTun\.inboundExcludeCidrs;/.test(bypassed)).toBe(false);
    // 空串条目要在提交前滤掉：点「添加」留下的空行落进配置就是一条空网段。
    expect(src).toMatch(/filter\(\(s\)\s*=>\s*s\s*!==\s*''\)/);
  });

  it('🔴 去重与草稿规则**不重写**：两端读同一份纯逻辑（`@/domain/list-entries`）', () => {
    const chrome = SOURCES.find((f) => f.file === 'SettingsChrome.tsx')?.text ?? '';
    expect(chrome, '移动端清单编辑器没有复用共享的纯逻辑').toContain(
      "from '@/domain/list-entries'",
    );
    for (const fn of ['parseBulkEntries', 'nextDraft', 'sameEntries']) {
      expect(chrome, `${fn} 没被用上 —— 大概率是又抄了一份`).toContain(fn);
    }
    // 且它**没有** import 桌面那条组件链（契约 A1 的理由，也是不直接复用 ListEditor 的理由）。
    expect(chrome).not.toContain('screens/settings/ListEditor');
    expect(chrome).not.toContain('screens/settings/Primitives');
  });

  it('🔴 两端都不许种幻影默认：缺席就画空表，不画一段并未生效的排除段', () => {
    /*
     * 引擎侧的真话：`crates/config-engine/src/builder/inbounds.rs:401-405` 对
     * `inbound_exclude_cidrs` 缺席与 `[]` 都是 `unwrap_or(&[])` ⇒ **什么都不排除**；
     * `crates/store/src/store.rs:290 default_config()` 里连 `tunConfig` 这一键都没有 ⇒
     * 全新安装就是缺席态。
     *
     * 桌面 `SettingsTun.tsx:96` 曾写 `?? ['100.64.0.0/10']`：`Fold` 打出 `count=1`、清单里画着
     * 一条**并未生效**的排除段，而移动端同一格画的是「尚未添加网段」——同一份配置两端说法相反，
     * 且在桌面点一次「添加网段」就把这条幻影当成用户清单落了库。2026-09-06 桌面改成 `?? []`。
     * 这条门钉的是「两端同答」，取材面是两个文件的源码。
     */
    const mobileSrc = SOURCES.find((f) => f.file === 'TunPage.tsx')?.text ?? '';
    const desktopSrc = stripComments(
      readFileSync(join(UI_SRC, 'components/screens/settings/SettingsTun.tsx'), 'utf8'),
    );
    expect(mobileSrc.length, 'TunPage.tsx 没进取材面').toBeGreaterThan(1000);
    expect(desktopSrc.length, 'SettingsTun.tsx 没进取材面').toBeGreaterThan(1000);

    /** 这一格的兜底里出现任何 CIDR 字面量 = 种了一段并未生效的排除。 */
    const seedsAPhantom = (src: string): boolean =>
      /inboundExcludeCidrs\s*\?\?\s*\[\s*'[^']/.test(src);
    expect(seedsAPhantom(desktopSrc), '桌面又给这一格种了默认段').toBe(false);
    expect(seedsAPhantom(mobileSrc), '移动端又给这一格种了默认段').toBe(false);
    /* 正面：两端都真的读了这个字段，且缺席时落到**空表**（只写「不许出现 X」会被
       「这一格整个没了」骗过）。
       **两端今天都走 `injectedList`**（2026-09-12 起）：缺席判成边界故障 —— 报错 + 空清单，
       比 `?? []` 强一档，值仍是空表，另外还留下一条「边界没注入」的记录。那条更强的纪律由
       `crates/config-engine/tests/frontend_sot_guard.rs` 对**注入字段**强制，本门不重复它。
       本门要钉的只是「缺席 ⇒ 空表，不是一段并未生效的排除段」，故 `?? []` 这种更弱但仍正确的
       写法也认 —— 认它不是为了给谁开口子，是因为本门的命题本来就不含「必须用哪种读法」。 */
    const fallsBackToEmpty = (src: string): boolean =>
      /inboundExcludeCidrs\s*\?\?\s*\[\s*\]/.test(src) ||
      /injectedList\(\s*tun\.inboundExcludeCidrs\s*,/.test(src);
    for (const [name, src] of [
      ['桌面', desktopSrc],
      ['移动端', mobileSrc],
    ] as const) {
      expect(
        fallsBackToEmpty(src),
        `${name}这一格没有以空表兜底 —— 要么字段没读、要么兜底又变成别的东西`,
      ).toBe(true);
    }
    /* 反向对照之二：谓词不许被「读了这个字段」本身喂绿 —— 只出现字段名、没有任何兜底形态时必须为假。
       少这一条，上面那个 `||` 就可能被一句光秃秃的 `tun.inboundExcludeCidrs` 骗过。 */
    expect(fallsBackToEmpty('const x = tun.inboundExcludeCidrs;')).toBe(false);
    expect(fallsBackToEmpty("const x = tun.inboundExcludeCidrs ?? ['100.64.0.0/10'];")).toBe(false);
    // 反向对照：谓词认得出「种了一段」这种形态。
    expect(seedsAPhantom("const x = tun.inboundExcludeCidrs ?? ['100.64.0.0/10'];")).toBe(true);
    // `100.64.0.0/10` 仍可以当**建议**出现在 placeholder 里，那不是既成事实。
    expect(mobileSrc).toContain('placeholder="100.64.0.0/10"');
  });

  it('Linux 专属那句提示**不移植**（在 Android 上它是一句与本机无关的话）', () => {
    expect(MARKUP.tun).not.toContain(copy('settings.tun.inboundLinuxNote'));
    // 正向对照：这条键确实还在 locale 里（桌面仍在用），不是因为它没了才「找不到」。
    expect(copy('settings.tun.inboundLinuxNote').length).toBeGreaterThan(5);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ⑫quater v2 **默认 DNS 策略**：配置里在、界面上也改得动了（2026-09-13 / 批 17 接线）。
 *
 * 此前这两格（`dnsDefaults.unmatchedAction` + Hosts 档的兜底服务器）在移动端只读不可写，页面上
 * 是一句 `SettingsNote` 缺席理由。本组钉的是**能力**，不是文案：
 *  · 两格是真控件，且写腿真的落在 `dnsDefaults` 上（不是只改 legacy 镜像）；
 *  · `dnsConfig` 那一半同写（漏了它，v1/v2 两份配置会各说各话）；
 *  · 候选表是**分组**的，且组头**可折叠**（`Csel.tsx|f:header` 在本屏的落点）；
 *  · 那句缺席理由一处都不剩。
 */
describe('⑫quater v2 默认 DNS 策略：两格真的改得动（批 17 接线）', () => {
  const src = stripComments(readFileSync(join(HERE, 'DnsPage.tsx'), 'utf8'));
  /* 渲染要等顶层那个 `beforeAll` 把 i18n 实例建起来 —— describe 体在 collect 阶段就跑，
     那时 `i18n` 还是 undefined，写成模块级常量会当场炸在收集期（错误读起来像判据文件坏了）。 */
  let v2 = '';
  /** 未命中动作取 `fakeIp` 的那一档：兜底行必须**整行不画**（不是画成禁用）。 */
  let v2NoHosts = '';
  beforeAll(() => {
    v2 = dnsMarkup({}, DNS_V2_TOP);
    v2NoHosts = dnsMarkup({}, {
      ...DNS_V2_TOP,
      dnsDefaults: {
        ...(DNS_V2_TOP.dnsDefaults as object),
        unmatchedAction: { type: 'fakeIp' },
      },
    });
  });

  it('自检：v2 夹具真的翻到了那一档（翻不到 ⇒ 下面每条都是对空气说话）', () => {
    expect(v2, 'v2 夹具没渲染出默认策略行 —— 夹具或门控坏了').toContain('data-setting="dns-default-action"');
    // 反向对照：schema v1 那一档一行都不画（`MARKUP.dns` 是 v1）。
    expect(MARKUP.dns).not.toContain('data-setting="dns-default-action"');
    expect(MARKUP.dns).not.toContain('data-setting="dns-default-fallback"');
  });

  it('🔴 两格都是**可点的控件**，不是只读文字（缺席理由的那句话一处都不剩）', () => {
    // 触发器是按钮 + `aria-haspopup="listbox"`：收起来的下拉必须说得出自己是干什么的。
    expect(v2).toContain('aria-haspopup="listbox"');
    expect(v2).toContain(`aria-label="${copy('settings.dns.defaultUnmatched')}"`);
    expect(v2).toContain(`aria-label="${copy('rules.dnsHostsFallback')}"`);
    // 本页一条 `SettingsNote` 形态的缺席理由都不该再有「默认策略」那一条。
    expect(v2).not.toContain('data-note="dns-defaults-absent"');
  });

  it('🔴 当前档显示的是**那一档的名字**，不是内部编码（`server:builtin-domestic` 之类）', () => {
    expect(v2, 'Hosts 档的名字没显示出来').toContain('Hosts');
    expect(v2, '触发器上印出了内部编码').not.toContain('server:builtin-domestic');
    expect(v2).not.toContain('hosts:sys-hosts');
  });

  it('🔴 兜底那一行只在 `hostsFirst` 档出现（别的档整行不画，不画成禁用）', () => {
    expect(v2).toContain('data-setting="dns-default-fallback"');
    expect(v2NoHosts, '非 Hosts 档也画了兜底行').not.toContain('data-setting="dns-default-fallback"');
    // 正面对照：那一档主行还在（不是整块没渲染出来）。
    expect(v2NoHosts).toContain('data-setting="dns-default-action"');
  });

  it('🔴 写腿真的落在 `dnsDefaults` 上，且 `dnsConfig` 那一半同写（只改一半 = 两份配置各说各话）', () => {
    const at = src.indexOf('function commitDefaultAction');
    expect(at, '写腿不见了 —— 判据面塌了').toBeGreaterThan(-1);
    const body = src.slice(at, at + 600);
    expect(body, '没写 `dnsDefaults`').toContain('dnsDefaults: { ...defaults, unmatchedAction }');
    expect(body, 'legacy 镜像 `dnsConfig` 没同写').toContain('dnsConfig: { ...dns,');
    // 两个键的补丁复用本页那颗 v1 FakeIP 开关的同一个函数，不在这里重写一遍。
    expect(body).toContain('fakeIpTogglePatch(unmatchedAction.type === ');
    // 两格共用这条写腿，行 id 逐字由调用点给（拼出来的 id 判据面扫不到）。
    expect(src).toContain("commitDefaultAction('dns-default-action'");
    expect(src).toContain("commitDefaultAction('dns-default-fallback', dnsActionFromChoice(");
  });

  it('🔴 判据不重写：候选表 / 当前档 / 档位反解全部从共享纯模块取', () => {
    expect(src).toContain("from '@/components/dialogs/dns-action-options'");
    for (const fn of [
      'buildDnsActionGroups',
      'dnsDefaultActionChoice',
      'dnsDefaultFallbackChoice',
      'dnsActionFromChoice',
    ]) {
      expect(src, `${fn} 没被用上 —— 那一格多半被本页重写了一份`).toContain(fn);
    }
    // 两端读同一份：桌面那两处也走同一对函数（搬家前它们是行内表达式）。
    const desktop = stripComments(
      readFileSync(join(UI_SRC, 'components/screens/rules/DnsPolicyWorkspace.tsx'), 'utf8'),
    );
    expect(desktop, '桌面又把当前档算回了行内表达式 —— 两份实现会各自漂移').toContain(
      'dnsDefaultActionChoice(dnsDefaults)',
    );
    expect(desktop).toContain('dnsDefaultFallbackChoice(dnsDefaults)');
  });

  /*
   * 组头这一维要**两条判据成对交**才盖得住，缺任何一条都留一条缝：
   *  · 面板是按 `open` 开合的，而 `open` 由本页的 `useState` 持有 ⇒ `renderToStaticMarkup`
   *    （全仓唯一的渲染方式，无 testing-library）永远只到得了「面板关着」那一帧。光看这一帧
   *    只能证明触发器在，证不了组头长什么样。
   *  · 反过来，只渲染一次面板也只证明「那颗组件会画组头」—— 它在规则屏早就被证过了，
   *    本屏要证的是**这一页真的把可折叠的那份数据交给了它**。
   * 故：①源码级钉接线（`groups={actionGroups}` ← `collapsibleGroups(buildDnsActionGroups(…))`），
   * ②渲染级喂**同一条数据流的产物**给面板，证明它真的画出可折叠组头与默认展开集。
   */
  it('🔴 ① 接线：面板吃的就是「可折叠化」之后的那份候选表', () => {
    // 组头的渲染实现一份都没新写：复用规则屏那颗面板，行摊平走全仓同一个 `buildCselRows`。
    expect(src).toContain("from '../screens/rules/Primitives'");
    expect(src).toContain('SelectSheetPanel');
    // 「折不折」的唯一开关是 `CselGroup.id`：本页给每组配了稳定键。
    expect(src).toContain('groups.map((group) => ({ ...group, id: group.label }))');
    // 两张面板的 `groups` 实参都取自 `collapsibleGroups(...)` 的产物，不是 `buildDnsActionGroups` 的裸返回。
    expect(src).toContain('const actionGroups = collapsibleGroups(');
    expect(src).toContain('const fallbackGroups = collapsibleGroups(');
    expect(src).toContain('groups={actionGroups}');
    expect(src).toContain('groups={fallbackGroups}');
    // 默认展开集也接上了（少这一格，打开面板时用户选中的那一档藏在折起的组里）。
    expect(src).toContain('openGroupIds={openGroupOf(actionGroups, actionValue)}');
    /*
     * 🔴 **触发器与面板的开合键必须逐字成对**，这是本组唯一堵得住的那条缝：面板按 `open` 开合，
     * 而 `renderToStaticMarkup` 只到得了「关着」那一帧 ⇒ 把面板的 `open` 写成恒 `false`（或两边
     * 用了不同的键）时，上面每一条源码断言与下面每一条渲染断言**都不会红**，而用户点下去什么
     * 都不发生。
     *
     * 🔴 判据按**切片**取，不按整份源码 `toContain`：上一版就是整份 `toContain`，而
     * `open={sheet === 'action'}` 在触发器上也有一份 ⇒ 把**面板**那一处改成 `open={false}`
     * 时判据照绿（实测过）。计数不是位置。
     */
    const panels = src.split('<SelectSheetPanel').slice(1);
    const triggers = src.split('<SelectSheetTrigger').slice(1);
    // 切片自检：切不出块（或块数变了）时下面的 `some` 会恒假/恒真，先让它自曝。
    expect(panels.length, '切不出两张面板 —— 切片面塌了，或多了一张没登记的').toBe(2);
    expect(triggers.length, '切不出两颗触发器').toBe(2);
    for (const key of ['action', 'fallback']) {
      expect(
        triggers.some(
          (block) =>
            block.includes(`onOpen={() => setSheet('${key}')}`) &&
            block.includes(`open={sheet === '${key}'}`),
        ),
        `没有哪颗触发器把 sheet 置成 '${key}' 并按同一个键回显展开态（aria-expanded）`,
      ).toBe(true);
      expect(
        panels.some((block) => block.includes(`open={sheet === '${key}'}`)),
        `'${key}' 那张面板的 open 没跟触发器对上 —— 点了不会打开`,
      ).toBe(true);
    }
  });

  /** 本页真正交给面板的那份候选表（逐字同 `DnsDefaultsRows` 的构造，含 `responses` 裁剪）。 */
  const liveGroups = (currentValue: string): CselGroup[] =>
    collapsibleGroups(
      buildDnsActionGroups({
        servers: DNS_V2_TOP.dnsServers as DnsServerResource[],
        groups: DNS_V2_TOP.dnsServerGroups as DnsServerGroup[],
        nodes: [],
        t: i18n.t.bind(i18n) as unknown as Parameters<typeof buildDnsActionGroups>[0]['t'],
        currentValue,
        responses: ['fakeIp', 'reject'],
      }),
    );

  const panelHtml = (currentValue: string): string => {
    const groups = liveGroups(currentValue);
    return render(
      createElement(SelectSheetPanel, {
        label: 'probe',
        groups,
        value: currentValue,
        openGroupIds: openGroupOf(groups, currentValue),
        open: true,
        onClose: () => undefined,
        onSelect: () => undefined,
        closeLabel: 'close',
      }),
    );
  };

  it('🔴 ② 渲染：同一条数据流真的画出**可折叠**的组头（不是纯视觉分隔）', () => {
    const html = panelHtml('hosts:sys-hosts');
    // `.mr-sel-grp-t` + `aria-expanded` 是可折叠那一支；不带 id 的组走 `role="presentation"` 的 div。
    expect(html, '面板里一个可折叠组头都没有').toContain('mr-sel-grp-t');
    expect(html, '组头没有展开态 ⇒ 它不是可折叠的那一支').toContain('aria-expanded=');
    expect(html, '组标题没画出来').toContain(copy('rules.dnsActionServerHeading'));
    expect(html).toContain(copy('rules.dnsActionGroupHeading'));
    // 反向对照：不「可折叠化」时同一份数据只得到纯视觉分隔 —— 证明上面几条不是恒绿。
    const flat = render(
      createElement(SelectSheetPanel, {
        label: 'probe',
        groups: buildDnsActionGroups({
          servers: DNS_V2_TOP.dnsServers as DnsServerResource[],
          groups: DNS_V2_TOP.dnsServerGroups as DnsServerGroup[],
          nodes: [],
          t: i18n.t.bind(i18n) as unknown as Parameters<typeof buildDnsActionGroups>[0]['t'],
          currentValue: 'hosts:sys-hosts',
          responses: ['fakeIp', 'reject'],
        }),
        value: 'hosts:sys-hosts',
        open: true,
        onClose: () => undefined,
        onSelect: () => undefined,
        closeLabel: 'close',
      }),
    );
    expect(flat, '没配 id 也画出了可折叠组头 —— 那条判据量不出差别').not.toContain('mr-sel-grp-t');
    expect(flat, '正向对照：组标题本身仍在（只是折不起来）').toContain(copy('rules.dnsActionServerHeading'));
  });

  it('🔴 ③ 含当前档的那一组默认展开（不猜第一组：选中的档必须一打开就看得见）', () => {
    /* 折叠组的选项**不渲染**（`buildCselRows` 的判据）⇒ 用「那一组的成员名在不在 DOM 上」两个
       方向各证一次。当前档落在 Hosts 组时，服务器组那条（`竞速组`）应当折着。 */
    const onHosts = panelHtml('hosts:sys-hosts');
    expect(onHosts, 'Hosts 那一组没展开 —— 用户看不到自己选的是哪一档').toContain('Hosts');
    expect(onHosts, '折起来的那一组把选项也渲染出来了 —— 折叠没生效').not.toContain('竞速组');
    // 反向对照：当前档落在服务器组里时，那一组就该展开（证明上一条不是「恒不渲染」）。
    expect(panelHtml('group:grp-race'), '当前档所在的组没展开').toContain('竞速组');
  });
});

describe('⑬ 开关：命中盒 48，视觉仍是 44×26（§3.1 #11）', () => {
  const switchChunk = (): string => {
    const row = rows(MARKUP.general).find((r) => r.id === 'auto-connect');
    expect(row, '通用页那颗开关不见了 —— 判据面塌了').toBeDefined();
    expect(row?.chunk, '取到的行里没有开关').toContain('role="switch"');
    return row?.chunk ?? '';
  };

  it('命中盒两边都引 `--tap-min`（写死 48 也算过，但那就不跟着 token 走了）', () => {
    const chunk = switchChunk();
    expect(chunk).toContain('min-width:var(--tap-min)');
    expect(chunk).toContain('min-height:var(--tap-min)');
  });

  it('视觉轨道一格不动：44×26 + 18 圆钮，且负外边距把布局占位收回去', () => {
    const chunk = switchChunk();
    expect(chunk).toContain('width:44px');
    expect(chunk).toContain('height:26px');
    expect(chunk).toContain('width:18px');
    // 收回布局占位的那两笔：不收的话行会被 48 撑高，开关看着就变大了。
    expect(chunk).toContain('calc((26px - var(--tap-min)) / 2)');
    expect(chunk).toContain('calc((44px - var(--tap-min)) / 2)');
  });

  /**
   * 开关元素的**开标签**（含 `style`）。
   *
   * 取材面必须收窄到开关本身。早先那一版数的是整页里 `min-height:var(--tap-min)` 的**出现次数**，
   * 而 `FIELD`（:297）/ `BUTTON`（:380）/ 页头返回键（:568）写的是同一行字符串：实测八页里
   * 21 个命中来自非开关元素、29 颗才是开关 ⇒ 判据自带 21 单位的松弛，可以有多达 21 颗开关丢掉
   * 命中盒而它照样绿。**计数不是覆盖**：要判「每一颗」就得逐颗取出来点名。
   *
   * 不写死 `<button`：将来有人内联手画一颗 `<div role="switch">`（本仓内联样式的写法下门槛很低），
   * 这里必须照样抓得到 —— 那正是这条判据要防的复发形态。
   */
  const switchTags = (markup: string): string[] =>
    [...markup.matchAll(/<[a-z][a-z0-9]*[^>]*\srole="switch"[^>]*>/g)].map((m) => m[0]);

  /** 没拿到 48 命中盒的开关，按 `aria-label` 点名（报错时看得出是哪一颗）。 */
  const hitBoxOffenders = (markup: string): string[] =>
    switchTags(markup)
      .filter(
        (tag) =>
          !tag.includes('min-width:var(--tap-min)') || !tag.includes('min-height:var(--tap-min)'),
      )
      .map((tag) => /aria-label="([^"]*)"/.exec(tag)?.[1] ?? tag.slice(0, 60));

  it('八页的每一颗开关都走同一条腿（漏一颗 = 那一颗仍然只有 26 高）', () => {
    // 取材面含 DNS 页**两档**（见 `DNS_RACE_MARKUP` 头注）：竞速池那三颗开关只在 race 开时渲染。
    const all = allMarkup() + '\n' + ROOT_MARKUP + '\n' + DNS_RACE_MARKUP;
    const tags = switchTags(all);
    expect(tags.length, '八页一颗开关都没扫到 —— 判据面塌了').toBeGreaterThanOrEqual(15);
    // 扩面自检：第二份夹具确实带进了**新的**开关（否则这次扩面是个静默的空操作）。
    const raceOnly = switchTags(DNS_RACE_MARKUP).filter((t) => !switchTags(MARKUP.dns).includes(t));
    expect(
      raceOnly.length,
      'DNS 第二份夹具没带进任何新开关 —— 互斥分档没生效，这次扩面等于没做',
    ).toBeGreaterThanOrEqual(3);
    // 取材面自检：抓到的开标签数必须**等于** `role="switch"` 的出现次数。少一个就说明有一颗开关的
    // 写法逃出了上面那条正则，此后它不再被下面这条判据管着 —— 而门看起来还是绿的。
    expect(tags.length, '有 `role="switch"` 没被开标签正则抓到（写法逃出了取材面）').toBe(
      [...all.matchAll(/role="switch"/g)].length,
    );
    expect(hitBoxOffenders(all), '这几颗开关没拿到 48 的命中盒').toEqual([]);
  });

  it('自检：手画一颗没有命中盒的开关会被抓住（旧的计数式写法在这里恒绿）', () => {
    const handRolled =
      '<div data-setting="x">' +
      '<div role="switch" aria-label="手画的" style="min-height:26px"></div>' +
      '<input style="min-height:var(--tap-min)"/></div>';
    // 先证明旧形态确实抓不到它：同一段里有个**非开关**元素拿着 48，`hits >= switches` 当场成立。
    expect(
      [...handRolled.matchAll(/min-height:var\(--tap-min\)/g)].length,
      '这个反例没能复现旧判据的松弛',
    ).toBeGreaterThanOrEqual([...handRolled.matchAll(/role="switch"/g)].length);
    // 新形态点名抓住它。
    expect(hitBoxOffenders(handRolled)).toEqual(['手画的']);
    // 反向对照：合规的那一颗不进点名单（否则这条判据恒红，等于没判）。
    expect(hitBoxOffenders(MARKUP.general)).toEqual([]);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ⑭ 屏头**固定**（IA §2.4「Fixed and scrolling」第一条：status bar / screen header / bottom navigation）。
 *
 * 外壳没有页头槽位（`MobileShell` 把 children 整个交给滚动区）⇒ 与首页同法，屏内 `sticky` 兑现同一
 * 语义。这条在设置屏比首页更承重：Android 系统返回键虽已接上（`back-stack.ts` → `MobileApp` 三档），
 * 但那是**看不见**的一条回退路径 —— 可发现性整个压在页头那颗返回按钮上，跟着内容滚走的话
 * 用户得先滚回顶部才找得到它。理由换了一半，判据不变。
 */
describe('⑭ 二级页页头固定，返回按钮不随内容滚走（§2.4 Fixed）', () => {
  const header = (): string =>
    render(
      createElement(SettingsHeader, {
        title: 'DNS',
        backLabel: copy('mobileSettings.back'),
        onBack: () => undefined,
      }),
    );

  it('页头是 sticky，且是不透明实底 + 发丝线收边（卡片要从它下面滚过去）', () => {
    const html = header();
    expect(html).toContain('position:sticky');
    expect(html).toMatch(/top:0(px)?/);
    expect(html).toContain('background:hsl(var(--bg))');
    expect(html).toContain('border-bottom:1px solid hsl(var(--hair))');
  });

  it('自检 + 正面：这份 markup 里确实有那颗返回按钮（它才是被钉住的东西）', () => {
    const html = header();
    expect(html).toContain('data-back="settings"');
    expect(html).toContain(copy('mobileSettings.back'));
    // 反向对照：根页（无 `onBack`）不画返回键，但页头照样是固定的。
    const root = render(
      createElement(SettingsHeader, { title: '设置', backLabel: copy('mobileSettings.back') }),
    );
    expect(root).not.toContain('data-back=');
    expect(root).toContain('position:sticky');
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ⑮ 压栈的二级页上**不重复画底部导航**（`mobile-screen-shell.md#Variants` / IA §2.4 Fixed）。
 *
 * 这一组判的是**接线**，不是机制：渲染的是整棵生产树（`MobileShell` 套真的 `MobileSettingsScreen`），
 * 栈由外壳那张表驱动。故「屏把栈写回屏内 `useState`」「外壳读了别的目的地那一格」这类断线
 * 都会在这里现形 —— 而这正是本项目反复撞的那道缝：机制有门、接线没门。
 *
 * 能这么判的前提是压栈状态在 **render 期**可读（`MobileShell` 的 `usePushedPage` 走
 * `useSyncExternalStore` 且 server snapshot 就是 live 读法）。换成 effect 上报或 zustand，
 * 这四条会全部恒绿 —— 理由逐字写在那个文件里。
 */
describe('⑮ 压栈页不带底部导航（spec Variants / §2.4）', () => {
  const shell = (child: ReactElement): string =>
    render(
      createElement(MobileShell, {
        active: 'settings',
        onSelect: () => undefined,
        children: child,
      }),
    );

  const screenTree = (): string => {
    useAppStore.setState({ config: CONFIG as unknown as UserConfig });
    return shell(createElement(MobileSettingsScreen));
  };

  const reset = (): void => {
    setPushedPage('settings', null);
    setPushedPage('rules', null);
  };

  it('自检 + 正面对照：根页上导航真的画得出来（否则下面每条都恒绿）', () => {
    reset();
    const html = screenTree();
    expect(html).toContain('class="m-nav"');
    // 正面：渲染出的确实是设置屏根页那张列表，不是加载态空壳。
    expect(html).toContain('data-push="dns"');
  });

  it('🔴 栈写成 `dns` 之后：同一棵生产树上导航消失、DNS 页出现', () => {
    reset();
    setPushedPage('settings', 'dns');
    const html = screenTree();
    expect(html, '压栈页上仍然画着底部导航').not.toContain('class="m-nav"');
    // 正面：屏读的就是外壳那一格 —— 压栈页真的换成了 DNS 页。
    expect(html).toContain('data-setting="connection-resolution"');
    expect(html).not.toContain('data-push="dns"');
    // 停靠区本体留着（`--safe-b` 那段手势条留白由它给），只是不再画成一根条。
    expect(html).toContain('class="m-dock"');
    expect(html).toContain('data-slot="pending-changes"');
    reset();
  });

  it('回到根页导航就回来（不是一去不返）', () => {
    reset();
    setPushedPage('settings', 'dns');
    expect(screenTree()).not.toContain('class="m-nav"');
    setPushedPage('settings', null);
    expect(screenTree()).toContain('class="m-nav"');
  });

  it('反向对照：别的目的地压着栈不影响本目的地的导航（外壳读的是当前那一格）', () => {
    reset();
    setPushedPage('rules', 'dns-detail');
    expect(screenTree()).toContain('class="m-nav"');
    reset();
  });

  it('栈里塞一个不认识的页 id 时当作根页处理（不渲染一页空白）', () => {
    reset();
    setPushedPage('settings', 'not-a-page');
    const html = screenTree();
    expect(html).toContain('data-push="dns"');
    reset();
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ⑯ 行高 / 卡角 / 内核版本串：上一轮**落了地却一条判据都没有**的三条（K5-05 / K5-06 / K5-07）。
 *
 * 复核时把 `ROW` 的 `minHeight` 改回 `var(--tap-min)`、删掉 `boxSizing`、把 `CARD` 的
 * `borderRadius` 改回 `var(--r)`、删掉 `AboutPage` 那行 `coreVersion` —— 四处一起改之后
 * `tsc` rc=0、全套 `vitest` 逐数不变。也就是说这三条今天可以被任何人静默改回去，diff review
 * 是唯一防线。
 *
 * 判据**不是三条并排的字面量比对**：三者的失败形态各不相同，形状也就不同。
 *
 *  · **行高**（K5-05）失败在「声明值与渲染值脱钩」：`min-height` 落在默认 `content-box` 上时，
 *    54 会被上下各 8px 内边距撑成 71 —— 今天那个 48 实际渲染成 65 就是这条。故判据钉的是**一对
 *    不变量**：凡是设置行，`min-height` 与 `box-sizing:border-box` 必须同时在，且那个下限引的是
 *    `--row-min` 而不是触控下限 `--tap-min`（48 与 54 恰好不是同一个数，两者都合法）。
 *  · **卡角**（K5-06）失败在「引错了别名」：`--r` 是 11、`--r-lg` 是 14，都存在、都合法、
 *    没有任何编译期区别，只差 3px。故判据钉的是**令牌身份**，不是几何值。
 *  · **内核版本串**（K5-07）失败在「那一行被删掉」，而它的值经 `useEffect` → IPC → `setState`
 *    到达，`renderToStaticMarkup` 跑不到 ⇒ 渲染面上永远是 `—`。故拆成两半：读入处的窄化守卫按
 *    **行为**判（⑯ 最后一条），拼串那一行按**源码**判。
 *
 * ⚠️ **如实登记没量到的那一段**：`useEffect` → `versionApi.getInfo()` → `setInfo` 这条接线本身
 * 没有判据（node 环境不跑 effect、本仓不装 jsdom、IPC 也没有夹具）。下面钉住的是它的两个端点
 * ——「守卫按契约收窄」与「页面读的就是这个守卫」——中间那一跳要真机看。
 */
describe('⑯ 行高 / 卡角 / 内核版本串（K5-05 / K5-06 / K5-07 的补门）', () => {
  /**
   * 设置行的**开标签**。注意不能用 `rows()`：切片从 `data-setting` 那个标记处起算，而 `style`
   * 写在它**前面** ⇒ 行自己的几何声明根本不在 chunk 里。
   */
  const rowTags = (markup: string): string[] =>
    [...markup.matchAll(/<[a-z][a-z0-9]*[^>]*\sdata-(?:setting|push)="[^"]*"[^>]*>/g)].map((m) => m[0]);

  /** 判据面含 DNS 页**两档**：竞速池那三行只在 race 开时渲染（见 `DNS_RACE_MARKUP` 头注）。 */
  const surfaces = (): string[] => [
    ...MOBILE_SETTINGS_PAGES.map((id) => MARKUP[id]),
    ROOT_MARKUP,
    DNS_RACE_MARKUP,
  ];
  const allRowTags = (): string[] => surfaces().flatMap(rowTags);
  const rowName = (tag: string): string =>
    /data-(?:setting|push)="([^"]+)"/.exec(tag)?.[1] ?? tag.slice(0, 60);

  it('自检：设置行的开标签真的抓得到，且一个都没漏', () => {
    const tags = allRowTags();
    expect(tags.length, '八页 + 根页 + DNS 第二档一个设置行都没抓到 —— 判据面塌了').toBeGreaterThanOrEqual(
      40,
    );
    // 与 `rows()` 的标记数对拍：少一个就说明有一种行的写法逃出了正则，而门看起来还是绿的。
    // 两边同源同面 ⇒ 扩面之后基数一起变大，这条对拍本身不受扩面影响。
    const marked = surfaces().flatMap((m) => [...m.matchAll(/data-(?:setting|push)="[^"]*"/g)]).length;
    expect(tags.length, '有设置行的开标签没被正则抓到').toBe(marked);
    /* 扩面自检：DNS 第二档确实带进了**新的**设置行，否则扩面是空操作。
       竞速池那三颗开关 + 自定义 DoH 那张清单编辑器（2026-09-13 接通）—— 四行都只在 race 档出现，
       另一档（单上游）没有池这个概念，也就没有「自定义上游占额度」这件事。 */
    const raceOnly = rowTags(DNS_RACE_MARKUP).filter((t) => !rowTags(MARKUP.dns).includes(t));
    expect(
      raceOnly.map(rowName).sort(),
      'DNS 第二份夹具没带进竞速档那几行 —— 互斥分档没生效',
    ).toEqual(['race-custom-list', 'race-pool-ali', 'race-pool-dnspod', 'race-pool-system']);
  });

  it('设置行保持 48px 命中下限，长说明自然增高且不把内边距算到行高外', () => {
    const offenders = allRowTags()
      .filter(
        (tag) =>
          !tag.includes('min-height:var(--tap-min)') || !tag.includes('box-sizing:border-box'),
      )
      .map(rowName);
    expect(
      offenders,
      '有设置行丢失 48px 命中下限或 border-box，内边距可能把空行撑大',
    ).toEqual([]);
  });

  it('自检：`box-sizing` 与行高任一项不对，上面那条都会红（不是恒绿）', () => {
    const cases: readonly [string, string][] = [
      ['x', '<div style="min-height:var(--tap-min);padding:6px 0" data-setting="x"></div>'],
      ['y', '<div style="min-height:54px;box-sizing:border-box" data-setting="y"></div>'],
      ['z', '<div style="min-height:48px;box-sizing:border-box" data-setting="z"></div>'],
    ];
    for (const [id, markup] of cases) {
      const bad = rowTags(markup)
        .filter(
          (tag) =>
            !tag.includes('min-height:var(--tap-min)') ||
            !tag.includes('box-sizing:border-box'),
        )
        .map(rowName);
      expect(bad, `\`${id}\` 这种写法没被抓住`).toEqual([id]);
    }
  });

  it('🔴 K5-06 分组卡引 `--r-lg`（14），整屏一处 `var(--r)`（11）都不许有', () => {
    const surface = surfaces().join('\n');
    const cards = [...surface.matchAll(/<section[^>]*>/g)].map((m) => m[0]);
    expect(cards.length, '一张分组卡都没抓到 —— 判据面塌了').toBeGreaterThanOrEqual(10);
    expect(
      cards.filter((tag) => !tag.includes('border-radius:var(--r-lg)')).length,
      '有分组卡没引 `--r-lg` —— 卡角是 `radius.cardMobile`（=alias:radius.lg=14），不是 `--r`',
    ).toBe(0);
    expect(surface, '出现了 `border-radius:var(--r)` —— 卡角退回 11px，违反契约 D-3').not.toContain(
      'border-radius:var(--r)',
    );
    // 自检：上面那条否定式判据真的分得清 `--r` 与 `--r-sm` / `--r-xs`（同屏都在用它们）。
    expect('border-radius:var(--r-sm)').not.toContain('border-radius:var(--r)');
    expect('border-radius:var(--r-xs)').not.toContain('border-radius:var(--r)');
    expect('border-radius:var(--r)').toContain('border-radius:var(--r)');
    expect(surface, '`--r-sm` 不在屏上 ⇒ 上面那条自检对着一个不存在的形态').toContain(
      'border-radius:var(--r-sm)',
    );
  });

  it('🔴 K5-07 内核版本串那一行还在（源码级：它的值经 effect→IPC 到达，渲染面上量不到）', () => {
    const about = SOURCES.find((s) => s.file === 'AboutPage.tsx');
    expect(about, 'AboutPage.tsx 不在取材面里 —— 判据面塌了').toBeDefined();
    const text = about?.text ?? '';
    expect(text, '版本行不再读 `coreVersion` —— 关于页只剩应用版本').toContain('info?.coreVersion');
    expect(text, '`sing-box <版本>` 那半截拼串没了').toContain('sing-box ${');
    // 如实登记：这一格在**渲染面**上今天恒为待定占位 —— 上面两条是源码级的，够不着运行时。
    expect(MARKUP.about, 'node 环境跑不到 effect ⇒ 这一格必须是待定占位').toContain('—');
    // 取的是拼串那一半（`· sing-box <版本>`）而不是裸 `sing-box`：`settings.about.intro` 那句
    // 「基于 sing-box 的现代网络代理工具」也在这一页上，用裸串会把这条判据变成恒红。
    expect(MARKUP.about, '渲染面上出现了内核版本串 —— 说明它不是经 effect 来的').not.toContain(
      '· sing-box',
    );
    expect(MARKUP.about, '自检：那句 intro 确实在这一页上（上面那条切点是为它收窄的）').toContain(
      'sing-box',
    );
  });

  it('🔴 K5-RV07 版本载荷按**运行时**收窄，不靠 `as unknown as` 把类型关系切断', () => {
    // 后端真实载荷（`src-tauri/src/commands/updater/app_update.rs:428-436`）逐字段进来。
    expect(
      narrowVersionInfo({ appVersion: '1.2.3', coreVersion: '1.11.4', coreBaseline: '1.11.0' }),
    ).toEqual({ appVersion: '1.2.3', coreVersion: '1.11.4', coreBaseline: '1.11.0' });
    // 后端改名 / 改成 null ⇒ 整段判为读不到（页面回落 `—`），而不是渲染一个 `undefined`。
    expect(narrowVersionInfo({ appVersion: '1.2.3', coreCurrentVersion: '1.11.4' })).toBeNull();
    expect(narrowVersionInfo({ appVersion: '1.2.3', coreVersion: null })).toBeNull();
    expect(narrowVersionInfo(null)).toBeNull();
    expect(narrowVersionInfo('1.2.3')).toBeNull();
    // 基线缺席只影响展示位，不该把整段版本号一起打掉。
    expect(narrowVersionInfo({ appVersion: '1.2.3', coreVersion: '1.11.4' })?.coreBaseline).toBe('');
    // 接线：页面读的就是这个守卫（函数被测 ≠ 生产在用它 —— 抽函数会造出新的缝）。
    expect(
      SOURCES.find((s) => s.file === 'AboutPage.tsx')?.text,
      '关于页没走这个守卫 —— 上面几条就成了对一个死函数说话',
    ).toContain('narrowVersionInfo(payload)');
    // 双重断言不许回来。取材面是**剥了注释**的产品代码，故 AboutPage 头注里解释「为什么不是
    // `as unknown as`」的那两句不会把这条判据自己污染成恒红。
    expect(SOURCE_TEXT, '本屏又出现了 `as unknown as` —— 编译期保护被整个关掉').not.toContain(
      'as unknown as',
    );
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ⑰ 密度守卫：设置行的可点高度与视觉间距分别治理。
 * 宽屏不补一档只为填空白的 64px 行高；长说明仍由内容自然撑高。
 */
describe('⑰ 设置行 48px 命中与只读状态', () => {
  it('触控尺寸由已定义的 --tap-min 提供，卡片内距独立收紧', () => {
    expect(context('mobile').files.length).toBeGreaterThanOrEqual(6);
    expect(declaringSites('--tap-min', { ctx: 'mobile', where: ALL })).not.toEqual([]);
    const chrome = SOURCES.find((source) => source.file === 'SettingsChrome.tsx')?.text ?? '';
    expect(chrome).toContain("minHeight: 'var(--tap-min)'");
    expect(chrome).toContain("padding: 'var(--sp-3)'");
    expect(chrome).not.toContain('var(--row-min');
  });

  it('VPN 授权是只读状态，不画成另一颗可按按钮', () => {
    const status = /<output[^>]*data-status="vpn-authorization"[^>]*>/.exec(ROOT_MARKUP)?.[0];
    expect(status).toBeDefined();
    expect(status).not.toContain('border:');
    expect(status).not.toContain('cursor:pointer');
  });
});

// ═══════════════════════════════════════════════════════════════════════════
/**
 * ⑱ 应用更新：检查 + 显示 + 打开发布页（W-19），以及根页那行入口（W-20）。
 *
 * # 🔴 判据里**一个真实请求都不发**
 *
 * 检查那条腿打的是 GitHub。故 `runAppUpdateCheck` 吃一个 `check` 函数而不是直接 import
 * `updateApi` —— 本组全程喂假的，并在最后一条里**正面断言**「本组没有发出任何真实请求」
 * （生产那一跳 `checkAppUpdateViaIpc` 只被断言它绑到了真的那条 IPC 上，从不调用它）。
 *
 * # 三条腿必须成对
 *
 *  ① 折算：`appUpdateHint` / `appUpdateEntryVersion` 四态逐支（纯函数，直接驱动）；
 *  ② 会话态：`runAppUpdateCheck` 真的把结果落进那份共享快照，失败不折成「已是最新」；
 *  ③ 生产那一跳：更新页真的把 `checkAppUpdateViaIpc` 喂进去、根页真的把折算结果画出来。
 *    少 ③ 会被「函数都对、但容器恒传一个假的 / 恒传 null」骗过。
 */
describe('⑱ 应用更新：检查 + 显示 + 打开发布页（W-19 / W-20）', () => {
  /** 记一次调用；`spy.calls` 就是「本组发出去的全部请求」。 */
  const fakeChecker = (
    result: { hasUpdate: boolean; updateInfo?: { version: string }; error?: string } | Error,
  ): { fn: typeof checkAppUpdateViaIpc; calls: { includePrerelease: boolean }[] } => {
    const calls: { includePrerelease: boolean }[] = [];
    return {
      calls,
      fn: async (options) => {
        calls.push(options);
        if (result instanceof Error) throw result;
        return result;
      },
    };
  };

  /*
   * 🔴 **真实请求探针**。
   *
   * 「测试里的 IPC 一律打桩」这句话本身要有证据，否则它只是一句自述。做法：把生产那条绑定
   * 底下的 `updateApi.check` 换成一个**只计数、只抛错、绝不放行**的探针，然后
   *  · 先用一条正向对照证明它真的截得住（否则下面那个 0 是空话）；
   *  · 复位计数，最后一条断言本组其余用例一次都没碰过它。
   * 探针装在本组的 `beforeAll` 上、`afterAll` 还原：出了这一组它不该改变别人的行为。
   */
  let realCheckCalls = 0;
  const realCheck = updateApi.check;
  beforeAll(() => {
    updateApi.check = async () => {
      realCheckCalls += 1;
      throw new Error('probe: real update check must not run in tests');
    };
  });
  afterAll(() => {
    updateApi.check = realCheck;
  });

  beforeEach(() => {
    resetAppUpdateCheck();
  });

  it('自检：真实请求探针截得住生产那条绑定（否则本组末尾那个 0 是空话）', async () => {
    const before = realCheckCalls;
    await expect(runAppUpdateCheck(checkAppUpdateViaIpc, CONFIG)).rejects.toThrow('probe');
    expect(realCheckCalls, '探针没数到 —— 生产那条绑定绕过了它，末尾那条断言恒绿').toBe(before + 1);
    realCheckCalls = before; // 复位：末尾那条量的是**其余**用例
  });

  it('自检：会话态复位得掉（模块态跨用例存活，不复位会让下面互相污染）', () => {
    expect(readAppUpdateCheck()).toEqual({
      phase: 'idle',
      snapshot: null,
      target: null,
      reason: '',
    });
  });

  it('查到新版本 ⇒ 落进共享快照，且提示行说的是那个版本号', async () => {
    const spy = fakeChecker({ hasUpdate: true, updateInfo: { version: '9.9.9' } });
    await runAppUpdateCheck(spy.fn, CONFIG);
    expect(readAppUpdateCheck().phase).toBe('done');
    expect(readAppUpdateCheck().snapshot).toEqual({ hasUpdate: true, version: '9.9.9' });
    expect(appUpdateHint(readAppUpdateCheck())).toEqual({
      key: 'settings.update.bannerTitle',
      version: '9.9.9',
    });
  });

  it('没有新版本 ⇒ 说「已是最新」，且根页那一行**不**画', async () => {
    const spy = fakeChecker({ hasUpdate: false });
    await runAppUpdateCheck(spy.fn, CONFIG);
    expect(appUpdateHint(readAppUpdateCheck())).toEqual({ key: 'settings.update.upToDate' });
    expect(appUpdateEntryVersion(readAppUpdateCheck(), new Set(), new Set())).toBeNull();
  });

  it('🔴 查失败 ⇒ 绝不折成「已是最新」（用户会据此认为自己在最新版上）', async () => {
    const spy = fakeChecker(new Error('network down'));
    await expect(runAppUpdateCheck(spy.fn, CONFIG)).rejects.toThrow('network down');
    expect(readAppUpdateCheck().phase).toBe('error');
    expect(appUpdateHint(readAppUpdateCheck()), '失败被说成了「已是最新」或「发现新版本」').toBeNull();
    expect(appUpdateEntryVersion(readAppUpdateCheck(), new Set(), new Set())).toBeNull();
  });

  it('🔴 后端把失败放在 `error` 字段里（不抛）时同样算失败', async () => {
    const spy = fakeChecker({ hasUpdate: false, error: 'rate limited' });
    await expect(runAppUpdateCheck(spy.fn, CONFIG)).rejects.toThrow('rate limited');
    expect(readAppUpdateCheck().phase, '`error` 字段被无视了 ⇒ 一次失败显示成「已是最新」').toBe(
      'error',
    );
  });

  it('`hasUpdate:true` 却缺版本号（后端契约破损）⇒ 只说「发现新版本」，根页那一行不画', async () => {
    const spy = fakeChecker({ hasUpdate: true });
    await runAppUpdateCheck(spy.fn, CONFIG);
    expect(appUpdateHint(readAppUpdateCheck())).toEqual({ key: 'settings.update.foundNew' });
    expect(
      appUpdateEntryVersion(readAppUpdateCheck(), new Set(), new Set()),
      '画了一行「发现新版本 undefined」',
    ).toBeNull();
  });

  it('连点不发第二次请求（`checking` 期间直接返回）', async () => {
    const calls: { includePrerelease: boolean }[] = [];
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const first = runAppUpdateCheck(async (options) => {
      calls.push(options);
      await gate;
      return { hasUpdate: false };
    }, CONFIG);
    await runAppUpdateCheck(async (options) => {
      calls.push(options);
      return { hasUpdate: false };
    }, CONFIG);
    release();
    await first;
    expect(calls.length, '「检查中」时又发了一次请求').toBe(1);
  });

  it('通道走共享判定（正式版 / 测试版两档各喂一次，不在移动端另写一遍）', async () => {
    const stable = fakeChecker({ hasUpdate: false });
    await runAppUpdateCheck(stable.fn, { ...CONFIG, appUpdateChannel: 'stable' } as unknown as UserConfig);
    expect(stable.calls).toEqual([{ includePrerelease: false }]);
    resetAppUpdateCheck();
    const pre = fakeChecker({ hasUpdate: false });
    await runAppUpdateCheck(pre.fn, {
      ...CONFIG,
      appUpdateChannel: 'prerelease',
    } as unknown as UserConfig);
    expect(pre.calls, '测试版通道没有把预发布纳入候选').toEqual([{ includePrerelease: true }]);
  });

  it('已跳过 / 已关掉的版本不再画那一行（借用桌面那四道闸，不重写）', async () => {
    const spy = fakeChecker({ hasUpdate: true, updateInfo: { version: '9.9.9' } });
    await runAppUpdateCheck(spy.fn, CONFIG);
    const state = readAppUpdateCheck();
    expect(appUpdateEntryVersion(state, new Set(), new Set()), '正向对照：没跳过时它是画的').toBe(
      '9.9.9',
    );
    expect(appUpdateEntryVersion(state, new Set(['9.9.9']), new Set())).toBeNull();
    expect(appUpdateEntryVersion(state, new Set(), new Set(['9.9.9']))).toBeNull();
    // 按**版本**判：跳过的是别的版本时这一行仍该在。
    expect(appUpdateEntryVersion(state, new Set(['1.0.0']), new Set())).toBe('9.9.9');
  });

  it('🔴 更新页真的把生产那条 IPC 喂进去了（不是恒传一个假的 / 一个空函数）', () => {
    const src = stripComments(readFileSync(join(HERE, 'UpdatePage.tsx'), 'utf8'));
    expect(
      /runAppUpdateCheck\(\s*checkAppUpdateViaIpc\s*,\s*config\s*\)/.test(src),
      '更新页那颗「检查更新」按钮没有把真的那条 IPC（与当下配置）喂进去 —— ' +
        '上面每一条行为断言照样全绿，而真机上点它什么都不会发生',
    ).toBe(true);
    // 生产那一跳绑的是真的 `updateApi.check`（**只断言它绑对了，绝不调用它**）。
    expect(typeof checkAppUpdateViaIpc).toBe('function');
    expect(
      /checkAppUpdateViaIpc[\s\S]{0,120}updateApi\.check\(/.test(
        stripComments(readFileSync(join(HERE, 'app-update-check.ts'), 'utf8')),
      ),
      '生产那一跳没有绑到 `updateApi.check` 上',
    ).toBe(true);
  });

  it('🔴 更新页那一行画出了检查按钮；查到新版本时才多出「打开发布页」', async () => {
    /*
     * 🔴 按**按钮元素**数，不按文案子串数。
     *
     * 那句 desc（「…再打开发布页下载安装 APK」）本身就含着按钮标签这个子串 ——
     * 直接 `not.toContain(copy('openReleases'))` 会被自己的说明文案判红（首跑实测）。
     * 故切出这一行的 `<button>` 标签，按「标签文本恰等于那句按钮文案」计数。
     */
    const releaseButtons = (markup: string): number =>
      [...markup.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].filter(
        (m) => m[1] === copy('mobileSettings.update.openReleases'),
      ).length;

    // 默认（idle）：只有检查按钮，没有发布页按钮 —— 恒显一颗「去 GitHub」等于对每个人都说
    // 「这里有新版本」，而多数时候那不成立。
    expect(MARKUP.update).toContain(copy('mobileSettings.update.checkAction'));
    expect(releaseButtons(MARKUP.update), '还没查就画了「打开发布页」').toBe(0);

    // 查到之后重渲一次：两颗都在，且提示行说的是那个版本。
    const spy = fakeChecker({ hasUpdate: true, updateInfo: { version: '9.9.9' } });
    await runAppUpdateCheck(spy.fn, CONFIG);
    const markup = render(pageOf('update'));
    expect(releaseButtons(markup), '查到了新版本却没有「打开发布页」的出口').toBe(1);
    expect(markup).toContain('9.9.9');

    // 反向对照：查到「已是最新」时那颗按钮又收回去（证明上面那个 1 不是恒 1）。
    resetAppUpdateCheck();
    await runAppUpdateCheck(fakeChecker({ hasUpdate: false }).fn, CONFIG);
    expect(releaseButtons(render(pageOf('update'))), '已是最新却还画着发布页按钮').toBe(0);
  });

  it('🔴 「打开发布页」那颗按钮真的去开外链（不是一个空壳 promise）', () => {
    /*
     * 与 W-17 那条同族：上面那条只证明「查到新版本时那颗按钮画出来了」。把它的实参换成
     * `Promise.resolve()`（外链腿整条拔掉、按钮还在、点了什么都不做）时，本组每一条全绿
     * （协调者实测）。故按整次调用对拍。
     */
    const src = stripComments(readFileSync(join(HERE, 'UpdatePage.tsx'), 'utf8'));
    expect(
      /commit\(\s*'app-update'\s*,\s*systemApi\.openExternal\(RELEASES_URL\)\s*,/.test(src),
      '「打开发布页」点了什么都不做 —— 那是本批唯一交给用户的下载出口',
    ).toBe(true);
  });

  it('🔴 发布页地址与关于页那条逐字相同（两处各写一份，迁仓时一处改了另一处不改 = 404）', () => {
    const pick = (file: string): string | undefined =>
      /https:\/\/github\.com\/[\w-]+\/[\w-]+\/releases/.exec(
        stripComments(readFileSync(join(HERE, file), 'utf8')),
      )?.[0];
    const about = pick('AboutPage.tsx');
    expect(about, '关于页那条发布页地址不见了 —— 对拍面塌了').toBeDefined();
    expect(pick('UpdatePage.tsx'), '两处发布页地址漂了').toBe(about);
  });

  /* ── 「跳过此版本」与更新通道（2026-09-13 接线）───────────────────────────── */

  /** 按**按钮元素**数，不按文案子串数（同上一条：desc 里可能含同一个词）。 */
  const buttonsLabelled = (markup: string, label: string): number =>
    [...markup.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].filter((m) => m[1] === label).length;

  it('🔴 「跳过此版本」：查到新版本才出现，按下之后它与「打开发布页」一起收声', async () => {
    // 用一个独一无二的版本号：会话级跳过集合是模块态、只加不减，撞上别的用例会互相污染。
    const version = '9.9.9-skip-probe';
    const skip = copy('settings.update.bannerSkip');
    const release = copy('mobileSettings.update.openReleases');

    expect(buttonsLabelled(MARKUP.update, skip), '还没查就画了「跳过此版本」').toBe(0);

    await runAppUpdateCheck(fakeChecker({ hasUpdate: true, updateInfo: { version } }).fn, CONFIG);
    const found = render(pageOf('update'));
    expect(buttonsLabelled(found, skip), '查到了新版本却没有「跳过此版本」的出口').toBe(1);
    expect(buttonsLabelled(found, release)).toBe(1);

    /* 会话那一半：后端持久化的 `skippedVersion` 只让**下一次**检查不再报这个版本，
       已经查到的这一份结果要立刻收声，否则用户刚说「这个版本不用管」、界面还在推他去下载。 */
    markAppVersionSkipped(version);
    const after = render(pageOf('update'));
    expect(buttonsLabelled(after, skip), '跳过之后那颗按钮还在').toBe(0);
    expect(buttonsLabelled(after, release), '跳过之后还在推「打开发布页」').toBe(0);
  });

  it('🔴 「跳过此版本」真的走后端那条腿（只改会话态 = 重启后它又回来了）', () => {
    /*
     * 与「打开发布页」那条同族：上一条只证明按钮画出来了。把 `updateApi.skip(...)` 换成
     * `Promise.resolve()`（持久化整条拔掉、按钮还在、这次会话看着像成功了）时，上一条照样全绿。
     * 故按整次调用对拍，并单独钉住「`success:false` 折成 reject」那一格 ——
     * 后端这条回的是 `{success}` 信封、不抛，漏判它会让一次失败安静地长成一次成功。
     */
    const src = stripComments(readFileSync(join(HERE, 'UpdatePage.tsx'), 'utf8'));
    expect(
      /commit\(\s*'app-update'\s*,\s*updateApi\.skip\(foundVersion\)/.test(src),
      '「跳过此版本」没有把版本号交给后端 —— 下次检查它还会再报一遍',
    ).toBe(true);
    expect(
      /if \(!result\.success\) throw new Error\(\);/.test(src),
      '后端回 `success:false` 时没有折成一次 reject —— 跳过失败会显示成跳过成功',
    ).toBe(true);
    expect(src).toContain('markAppVersionSkipped(foundVersion)');
  });

  it('🔴 更新通道那一行真的写 `appUpdateChannel`（此前它配置里在、界面上看不见）', () => {
    // 这一格的读侧早就在：`appUpdateIncludePrerelease(config)` 被那颗检查按钮用着
    //（见本组「通道走共享判定」一条），缺的一直是写侧。
    expect(MARKUP.update, '更新页没有通道那一行').toContain('data-setting="update-channel"');
    expect(MARKUP.update).toContain(copy('settings.update.appChannelStable'));
    expect(MARKUP.update).toContain(copy('settings.update.appChannelPrerelease'));
    const src = stripComments(readFileSync(join(HERE, 'UpdatePage.tsx'), 'utf8'));
    expect(
      /commit\(\s*'update-channel'\s*,\s*update\(\{\s*appUpdateChannel:/.test(src),
      '通道那颗 `<select>` 没有写回配置 —— 换一档什么都不会变',
    ).toBe(true);
  });

  it('🔴 根页：`appUpdate` 为空 ⇒ 那一行不画；非空 ⇒ 画出来且点它进更新页', () => {
    expect(ROOT_MARKUP, '没有新版本时根页却画了那一行').not.toContain('data-push="app-update-available"');
    const opened: string[] = [];
    const markup = render(
      createElement(SettingsRoot, {
        onOpen: (id: MobileSettingsPageId) => opened.push(id),
        vpnAuth: 'unknown',
        appUpdate: '9.9.9',
      }),
    );
    expect(markup, '有新版本却没有那一行入口').toContain('data-push="app-update-available"');
    expect(markup).toContain('9.9.9');
    // 它**不是**第九个二级页：八行进入行的集合一格不动（① 组按那八项逐值对拍根页）。
    const pushes = [...markup.matchAll(/data-push="([^"]+)"/g)].map((m) => m[1]);
    expect(pushes.filter((id) => id !== 'app-update-available')).toEqual([...MOBILE_SETTINGS_PAGES]);
    // 那一行的落点是更新页（源码级：`renderToStaticMarkup` 点不动按钮）。
    expect(
      /onOpen=\{\(\) => onOpen\('update'\)\}/.test(
        stripComments(readFileSync(join(HERE, 'MobileSettingsScreen.tsx'), 'utf8')),
      ),
      '那一行的落点不是更新页 —— 点了会去别的地方或什么都不做',
    ).toBe(true);
    expect(opened, '渲染期就触发了导航').toEqual([]);
  });

  /*
   * 🔴 **屏组件把折算结果喂给根页的那一跳**（复审同族形态）。
   *
   * 上一条是拿 `appUpdate: '9.9.9'` 直接驱动 `SettingsRoot` 的行为断言 —— 它证明「那份列表是好的」，
   * 拦不住屏组件恒传 `null`：那样根页那一行永远不出现，而本组每一条照样全绿（协调者实测）。
   * 故这里判那个 JSX 属性的**表达式本身**：必须是从 `useAppUpdateEntry` 拿到的那个标识符。
   */
  it('🔴 那一行的「忽略」真的记了一笔（不然点了它下一次渲染又回来）', () => {
    const src = stripComments(readFileSync(join(HERE, 'MobileSettingsScreen.tsx'), 'utf8'));
    expect(
      /onClick=\{\(\) => markAppVersionDismissed\(appUpdate\)\}/.test(src),
      '「忽略」点了什么都不做 —— 它与「关不掉的横幅」是同一个观感',
    ).toBe(true);
    // 行为：那颗按钮记的是**本会话**的忽略集合，记完之后那一行就不该再画。
    const version = '8.8.8';
    const dismissed = new Set<string>();
    // `target: null` = 这次查到的 release 没有可下载的资产（Android 上真实存在的一档）。
    // 根页那一行的折算**不看**它：那一行回答的是「有没有新版本」，不是「下不下得了」。
    const state = {
      phase: 'done' as const,
      snapshot: { hasUpdate: true, version },
      target: null,
      reason: '',
    };
    expect(appUpdateEntryVersion(state, new Set(), dismissed)).toBe(version);
    dismissed.add(version);
    expect(appUpdateEntryVersion(state, new Set(), dismissed), '忽略之后那一行还在').toBeNull();
  });

  it('🔴 屏组件把**真的**折算结果喂进根页（不是恒传 null）', () => {
    const src = stripComments(readFileSync(join(HERE, 'MobileSettingsScreen.tsx'), 'utf8'));
    expect(
      /<SettingsRoot[^>]*\bappUpdate=\{appUpdate\}/.test(src),
      '根页拿到的不是那个折算结果 —— 恒传 null 时那一行永远不出现，而本组其余每一条照样全绿',
    ).toBe(true);
    expect(
      /const\s+appUpdate\s*=\s*useAppUpdateEntry\(config\)/.test(src),
      '`appUpdate` 不是从 `useAppUpdateEntry(config)` 来的 —— 它可能只是一个恒 null 的局部常量',
    ).toBe(true);
  });

  it('🔴 根页那条检查腿受 `autoCheckUpdate` 门控（关掉了就不该偷偷发一次请求）', () => {
    const src = stripComments(readFileSync(join(HERE, 'MobileSettingsScreen.tsx'), 'utf8'));
    expect(
      /if\s*\(!shouldBannerCheckUpdate\(config\)\)\s*return;/.test(src),
      '根页那条自动检查没有门控 —— 用户关掉「启动时检查更新」后照样被偷偷发一次 GitHub 请求',
    ).toBe(true);
    /*
     * 🔴 **闸门与依赖数组两格分开钉**（2026-09-06 复审）。
     *
     * ① 上一版只断言源码里出现过 `checkedRef.current` 这个字符串 —— 而那把闸装在组件的 `useRef`
     *    上，本屏随底部导航切换整个卸载重挂（`Screens.tsx` 的五个目的地是五个不同的组件函数）
     *    ⇒ 实际语义是「每次进设置查一次」。判据量不到「一次」这个语义，闸门在但没牙。
     *    现在闸住在 `app-update-check.ts` 的模块态里，由下面那条行为断言逐次数。
     * ② 依赖数组此前不在任何判据的射程内。实测把 `}, [config]);` 改成 `}, []);` ⇒ 全量全绿，
     *    而挂载时 `config` 为 null ⇒ `shouldBannerCheckUpdate(null)` 假 ⇒ 那条自动检查永不发生、
     *    根页「有新版本」入口行永远不出现。故按**整段**（含收尾那一行）对拍。
     */
    const wired =
      /if \(!shouldBannerCheckUpdate\(config\)\) return;[\s\S]*?runAutoAppUpdateCheck\(checkAppUpdateViaIpc, config\)[\s\S]*?\n  \}, \[config\]\);/;
    // ⓪ 自检：谓词分得清「依赖齐」与「依赖空」（合成源码正反各一份）。
    const synth = (deps: string): string =>
      `  useEffect(() => {\n    if (!shouldBannerCheckUpdate(config)) return;\n` +
      `    void runAutoAppUpdateCheck(checkAppUpdateViaIpc, config).catch(() => undefined);\n` +
      `  }, ${deps});\n`;
    expect(wired.test(synth('[config]'))).toBe(true);
    expect(wired.test(synth('[]')), '依赖空了却没被认出来 —— 下面那条断言恒绿').toBe(false);

    expect(
      wired.test(src),
      '根页那条自动检查不再是「门控 → 每会话一次的模块闸 → 跟着 config 重跑」那一段 —— ' +
        '依赖空了它只在挂载那一帧跑一次，而那一帧 config 还是 null，于是那一行永远不出现',
    ).toBe(true);
  });

  it('🔴 「每会话一次」是真的一次：连着两次自动检查只发一个请求（手动那颗按钮不受限）', async () => {
    const auto = fakeChecker({ hasUpdate: true, updateInfo: { version: '9.9.9' } });
    expect(await runAutoAppUpdateCheck(auto.fn, CONFIG), '第一次自动检查没发出去').toBe(true);
    // 第二次 = 用户切走再回来（本屏卸载重挂）。组件级 ref 在这里已经复位，模块级闸不会。
    expect(
      await runAutoAppUpdateCheck(auto.fn, CONFIG),
      '第二次进设置又发了一次 GitHub 请求 —— 未认证 API 限额 60 次/小时/IP，' +
        '且每次重查开头会把已查到的结果抹掉',
    ).toBe(false);
    expect(auto.calls.length, '本会话发出去的自动检查不止一次').toBe(1);

    // 正向对照 ①：复位之后它又发得出去（证明上面那个 false 不是「这条腿整个坏了」）。
    resetAppUpdateCheck();
    expect(await runAutoAppUpdateCheck(auto.fn, CONFIG)).toBe(true);
    expect(auto.calls.length).toBe(2);

    // 正向对照 ②：手动那颗按钮走的是 `runAppUpdateCheck`，**不受**这把闸限制 ——
    // 否则「检查更新」第二次点下去什么都不做。
    const manual = fakeChecker({ hasUpdate: false });
    await runAppUpdateCheck(manual.fn, CONFIG);
    await runAppUpdateCheck(manual.fn, CONFIG);
    expect(manual.calls.length, '手动检查被会话闸误伤了 —— 那颗按钮第二次点下去没反应').toBe(2);
  });

  /*
   * 🔴 **跨语言那半边**（2026-09-06 复审 blocker）。
   *
   * 本组其余每一条都止步于 `updateApi.check` 这一侧（喂假 checker / 装探针），后端那半边一条判据
   * 都没有 —— 而缺陷正好在那里：`update_check` 此前在取平台那一步就
   * `let Some(platform) = AssetPlatform::from_os(std::env::consts::OS) else { return hasUpdate:false }`，
   * 而 `from_os("android")` 恒返 `None`（本仓不出 APK 资产）⇒ Android 上「检查更新」**结构性恒答
   * 「已是最新」**，连一次请求都不发：界面上「打开发布页」按钮（`check.snapshot?.hasUpdate === true`）
   * 永不出现，根页那行入口也永不出现，整批更新腿在目标平台上零功能。
   * 而 `app-update-check.ts` 自己的头注写着「把失败折成 done 会把一次失败显示成『已是最新』，
   * 那是本模块最不能犯的错」——后端在 Android 上做的正是这件事。
   */
  it('🔴 跨语言：后端在**无安装包目标的平台**上不再把「有新版本」答成「已是最新」', () => {
    const rs = maskRustComments(
      readFileSync(join(REPO, 'src-tauri/src/commands/updater/app_update.rs'), 'utf8'),
    );
    expect(rs.length, 'app_update.rs 读到的是空文件 —— 判据面塌了').toBeGreaterThan(400);
    const body = /pub async fn update_check\([\s\S]*?\n\}/.exec(rs)?.[0] ?? '';
    expect(body.length, '`update_check` 的函数体切不出来').toBeGreaterThan(400);

    // ① 那条早退**不得**回来（它是缺陷本体：还没发请求就说「已是最新」）。
    const earlyReturn = /AssetPlatform::from_os\([^)]*\)\s*else\s*\{[\s\S]{0,120}?"hasUpdate":\s*false/;
    expect(earlyReturn.test('let Some(p) = AssetPlatform::from_os(x) else { return Ok(ApiResponse::ok(json!({ "hasUpdate": false }))); };')).toBe(true); // ⓪ 自检：谓词认得出那个形态
    expect(
      earlyReturn.test(body),
      'Android 又回到「取平台失败就报已是最新」——用户手上有 9.9.9 也会看到「已是最新」，' +
        '而「打开发布页」那颗按钮永不出现',
    ).toBe(false);

    // ② 正面：先取回 releases，再在「选不到资产」那一档走只比版本的检查腿。
    //
    // 🔴 2026-09-13（批 15）这条判据换了对象但**没有放松**：`AssetPlatform::Android` 落地之后，
    //    `from_os("android")` 不再返 `None`，那条 `let Some(platform) = platform else` 分支
    //    只剩「本仓根本不为它发包的平台」。Android 的同一个风险搬到了 match 里的那条臂上 ——
    //    资产腿说 `NoUpdate` 时必须再问一次「到底有没有新版本」，否则「有新版本但这个 release
    //    没发 APK」（2026-09-13 之前的每一个 release）又会被答成「已是最新」。
    //    两处兜底都调同一个 `release_only_response`（各写一份必然漂），故这里数它出现两次。
    const at = (needle: string): number => body.indexOf(needle);
    for (const needle of [
      'let platform = AssetPlatform::from_os(std::env::consts::OS);',
      'fetch_releases_json(',
      'let Some(platform) = platform else {',
      'if !include_current && platform == AssetPlatform::Android =>',
    ]) {
      expect(at(needle), `\`update_check\` 里找不到 \`${needle}\``).toBeGreaterThanOrEqual(0);
    }
    expect(
      (body.match(/release_only_response\(/g) ?? []).length,
      '「只比版本」那条兜底应恰好挂在两处（不发包的平台 / Android 选不到资产），' +
        '少一处就有一档会把「有新版本」答成「已是最新」',
    ).toBe(2);
    expect(
      at('fetch_releases_json(') < at('let Some(platform) = platform else {'),
      '平台分叉排在拉取之前 —— 那一档又变成「不发请求就下结论」',
    ).toBe(true);
    expect(
      at('let Some(platform) = platform else {') < at('release_only_response('),
      '只比版本那条腿没有挂在「不发包的平台」那一档上',
    ).toBe(true);
    // 🔴 臂序承重：`includeCurrent`（重装当前版本）必须排在 Android 兜底**之前**，
    //    否则那条腿在 Android 上永远拿不到目标（match 取第一条命中的臂）。
    expect(
      at('if !include_current && platform == AssetPlatform::Android =>') <
        at('if include_current => match resolve_current_app_release('),
      '两条臂的守卫在 Android 上同时成立，顺序反了「重装当前版本」就没有对象了',
    ).toBe(true);
    // 同一份源码里的第二条跨语言事实：内核腿在 Android 上**零网络**早退
    //（`AssetPlatform` 多了一态之后，少这一条就会开始真的去打 SagerNet 的 releases API）。
    const core = maskRustComments(
      readFileSync(join(REPO, 'src-tauri/src/commands/updater/core_update.rs'), 'utf8'),
    );
    expect(
      core.includes('Some(AssetPlatform::Android) | None => {'),
      '内核更新检查在 Android 上不再早退 —— 那是一次纯浪费的网络请求，结果恒为「没有适配资产」',
    ).toBe(true);
  });

  /** 收尾：本组除了那条正向对照，一次都没碰过真的 `updateApi.check`（见探针那段头注）。 */
  it('🔴 本组一次真实的更新检查请求都没发过（全程喂的是假 checker）', () => {
    expect(realCheckCalls, '有用例把真的那条 IPC 放出去了 —— 判据在打 GitHub').toBe(0);
  });
});
