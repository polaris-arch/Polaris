/**
 * 设置 → DNS（移动端）。桌面对照：`components/screens/settings/SettingsDns.tsx`（`section="runtime"`）。
 *
 * IA §2.4 对本页的处置是「除系统 DNS 接管开关（iOS 隐藏）外全部保留」。本批落地的是**解析器**整块
 * 与节点解析器的竞速档；三处**刻意不移植**，各自有理由（不是排期）：
 *
 *  · **国内/国外 DNS 的预设下拉（2026-09-06 接线）**：上一版不移植，理由是「桌面那张预设表是
 *    `SettingsDns.tsx` 里的**私有**函数（`remotePresets` / `domesticPresets`，未导出），
 *    移动端抄一份就是第二份真值源」。那个理由指向的处置不是「不做」，是「把表挪到共用处」——
 *    本批把两个函数搬进 `settings-dns-logic.ts`（本页已经在 import 的那个纯逻辑模块），
 *    两端**读同一张表**，桌面侧改成 import。抄一份那条路因此不用走，下拉也就没有理由缺席。
 *    ⚠️ 那张表现在是**共享面**：改它要同时跑桌面与移动端两侧的门。
 *  · **三张清单**（自定义 DoH 上游 / FakeIP 例外域名 / DoH 端点）：`ListEditor` 是一整个交互组件
 *    （增删改 + 批量导入），移动端形态该是 `action-sheet` 而不是把桌面清单塞进 375 宽 ——
 *    登记为后续线，见设计文档的「未移植面」。
 *
 *    ⚠️ **但「清单不移植」不等于整块不移植**（IA §3.3 规则 2）。上一版把 DoH 竞速池、FakeIP 过滤、
 *    浏览器 DoH 阻断三块整块拿掉了，连**开关本身**一起 —— 而 `fakeIpFilter`（默认开）与
 *    `blockBrowserDoh` 是两个布尔字段，值还在配置里照常生效（桌面改过就一直生效），手机上却既
 *    看不到也改不了，页面上也没有一句话说它去哪了。那正是 §4 开头点名的形态：「present in the
 *    configuration, invisible in the UI, and inert at runtime is a defect」。本轮把**开关与状态**
 *    补齐，清单仍不移植，但缺席改为**带常驻理由**（每块一条 `SettingsNote`）。
 *
 *    缺席理由**跟着各自的总开关走**（桌面 `SettingsDns.tsx:656/698`：「关闭时不渲染清单：不生效的
 *    可编辑清单是误导」）：总开关关着时那张清单整体不生效，描述它的说明挂在那儿会让人以为有一份
 *    正在生效的名单。故本页的 `fakeip-filter-list*` 与 `browser-doh-list*` 四条都在开关打开时才画。
 *
 *    ⚠️ 上面这四条 2026-09-13（批 10）**已经接通**：三张清单现在真的改得动，走 `MobileListEditor`。
 *
 *    ⚠️ 2026-09-13（批 17）起**本页一条缺席理由都不剩**：最后那一处（v2 配置下的 DNS 默认策略，
 *    引 `mobileRules.formUnavailable`）随 [`DnsDefaultsRows`] 落地而消失 —— 那两格现在真的改得动。
 *    那条键因此失去全仓最后一个渲染点，已从五份 locale 里删除（`i18n-coverage` 的 G6b 死键档
 *    是零容忍：接上了却留着译文，等于留一句没人会看见的假话）。
 *  · **FakeIP 关闭确认弹窗**：`dialog-store` 的弹窗层挂在桌面 `AppShell` 上，移动入口没有它 ⇒
 *    照搬会让弹窗**永不出现**、开关也就永不提交（静默失效）。风险文案改为**常驻**在开关下方
 *    （TUN 且当前开着时显示），这与 §4.12 是同一条处置：解释必须有常驻通道。
 *    文案另起一条键而不是复用桌面那句：桌面那句以「确认关闭？」收尾，它是弹窗的问句，
 *    常驻在页面上读起来像在问一个没人回答的问题。
 */

import { useEffect, useRef, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import type { DnsConfig, DnsPolicyAction, UserConfig } from '@/contracts/types';
import type { CselGroup } from '@/components/dialogs/csel-logic';
import {
  buildDnsActionGroups,
  dnsActionFromChoice,
  dnsDefaultActionChoice,
  dnsDefaultFallbackChoice,
  type DnsDefaults,
} from '@/components/dialogs/dns-action-options';
import {
  dnsConnectionResolutionPatch,
  effectiveDnsConnectionResolution,
  type DnsConnectionResolution,
} from '@/domain/dns-connection-resolution';
import {
  DNS_FALLBACK,
  DNS_PRESET_CUSTOM,
  domesticPresets,
  fakeIpTogglePatch,
  MAX_DOH_RACE_UPSTREAMS,
  nextRacePool,
  isPureIpDnsSpec,
  normalizeDnsTimeoutInput,
  parseDnsServerSpec,
  reconcileCustomUpstreams,
  remotePresets,
  type DnsUpstreamPreset,
} from '@/components/screens/settings/settings-dns-logic';
import { DEFAULT_BROWSER_DOH_SUFFIXES } from '@/contracts/browser-doh';
import { SelectSheetPanel, SelectSheetTrigger } from '../screens/rules/Primitives';
/* 那对原语的外观住在规则屏那份 CSS 里。再 import 一次**不改变**进包 CSS 的集合与次序
   （`styles/css-oracle.test.ts ⑤` 的有序清单里它早在第 4 位，设置屏那份在第 5 位），
   只保证「从设置屏进来时这套类也在」，不靠「规则屏一定先渲染过」这种时序巧合 ——
   与 `mobile/forms/RuleFormPanel.tsx` 那一行同一条理由。 */
import '../screens/rules/rules-screen.css';
import {
  MobileListEditor,
  MobileSelect,
  MobileSwitch,
  MobileTextInput,
  SettingsGroup,
  SettingsNote,
  SettingsRow,
} from './SettingsChrome';
import type { MobileSettingsPageProps } from './settings-pages';

/**
 * 默认策略候选的组头**可折叠**化。
 *
 * `buildDnsActionGroups` 出的组不带 `id` ⇒ 在 `SelectSheetPanel` 里恒展开（桌面 `Csel` 的同款
 * 形态）。移动端这一格必须折得起来：一台装了几十条 DNS 服务器的机器，390 宽的屏上全展开等于
 * 把用户扔进一条读不完的滚动条 —— 这正是 `Csel.tsx|f:header` 那条登记在移动端的落点。
 *
 * 键取组标签本身：折叠态**每次打开面板就重置**（`SelectSheetPanel` 的 effect），故它只需要在
 * 一次渲染内唯一，而四条组标题（服务器组 / 服务器 / Hosts / 响应动作）逐条不同。
 * 不改 `buildDnsActionGroups` 给组塞 id：那会让桌面那三处下拉一起变成可折叠的，
 * 而「折不折」是屏宽决定的呈现取舍，不是候选表的性质。
 */
export function collapsibleGroups(groups: readonly CselGroup[]): CselGroup[] {
  return groups.map((group) => ({ ...group, id: group.label }));
}

/** 含当前档的那一组默认展开 —— **不猜第一组**：选中的那一档必须一打开就看得见。 */
export function openGroupOf(groups: readonly CselGroup[], value: string): ReadonlySet<string> {
  const hit = groups.find((group) => group.options.some((option) => option.value === value));
  return new Set(hit === undefined ? [] : [hit.label]);
}

/** 当前档在候选表里的显示名（触发器上那行字）。找不到就回落到值本身，不编一个好看的。 */
function labelOf(groups: readonly CselGroup[], value: string): string {
  for (const group of groups) {
    const hit = group.options.find((option) => option.value === value);
    if (hit) return hit.label;
  }
  return value;
}

export function DnsPage({ config, update, commit }: MobileSettingsPageProps): ReactElement {
  const { t } = useTranslation();
  const dns: DnsConfig = config.dnsConfig ?? {
    domesticDns: DNS_FALLBACK.domesticDns,
    foreignDns: DNS_FALLBACK.foreignDns,
    enableFakeIp: true,
  };
  const hasDnsPolicyV2 = (config.configSchemaVersion ?? 0) >= 2 && config.dnsDefaults != null;

  /** 单上游：只放行内置三档；陈旧/自定义 id 回显 `ali`，与后端「未知 single 走 ali 基线」一致。 */
  const singleUpstream = ['ali', 'dnspod', 'system'].includes(dns.nodeResolverSingle ?? '')
    ? (dns.nodeResolverSingle as string)
    : 'ali';

  /** 写一段 DNS 配置。**必须带行 id**：写失败要贴在动手的那一行下面，不是页面顶上飘一条。 */
  function patchDns(rowId: string, patch: Partial<DnsConfig>): void {
    commit(rowId, update({ dnsConfig: { ...dns, ...patch } }));
  }

  /* 三个文本输入：草稿 + 失焦提交（桌面同款理由 —— 逐键写盘会让中间态真的落进配置，且代理运行时
     每个字符触发一次整核重启评估）。种子快照守住「外部重拉不打断正在输入的用户」。 */
  const [remote, setRemote] = useState(dns.foreignDns);
  const [domestic, setDomestic] = useState(dns.domesticDns);
  const [timeout, setTimeoutDraft] = useState(dns.dnsTimeoutMs != null ? String(dns.dnsTimeoutMs) : '');
  const [invalid, setInvalid] = useState<{ remote?: boolean; domestic?: boolean; timeout?: boolean }>({});
  const seeded = useRef({
    remote: dns.foreignDns,
    domestic: dns.domesticDns,
    timeout: dns.dnsTimeoutMs != null ? String(dns.dnsTimeoutMs) : '',
  });
  useEffect(() => {
    const snap = {
      remote: dns.foreignDns,
      domestic: dns.domesticDns,
      timeout: dns.dnsTimeoutMs != null ? String(dns.dnsTimeoutMs) : '',
    };
    const prev = seeded.current;
    setRemote((cur) => (cur !== prev.remote ? cur : snap.remote));
    setDomestic((cur) => (cur !== prev.domestic ? cur : snap.domestic));
    setTimeoutDraft((cur) => (cur !== prev.timeout ? cur : snap.timeout));
    seeded.current = snap;
  }, [dns.foreignDns, dns.domesticDns, dns.dnsTimeoutMs]);

  /** 行 id 逐字由调用点给，不用三元拼：拼出来的 id 判据面扫不到。 */
  function commitDns(rowId: string, key: 'foreignDns' | 'domesticDns', raw: string): void {
    const v = raw.trim();
    const slot = key === 'foreignDns' ? 'remote' : 'domestic';
    if (v && !parseDnsServerSpec(v)) {
      setInvalid((p) => ({ ...p, [slot]: true }));
      return;
    }
    setInvalid((p) => ({ ...p, [slot]: false }));
    const next = v || DNS_FALLBACK[key];
    if (key === 'foreignDns') setRemote(next);
    else setDomestic(next);
    if (next === dns[key]) return;
    patchDns(rowId, { [key]: next });
  }

  function commitTimeout(): void {
    const parsed = normalizeDnsTimeoutInput(timeout);
    if (!parsed) {
      setInvalid((p) => ({ ...p, timeout: true }));
      return;
    }
    setInvalid((p) => ({ ...p, timeout: false }));
    setTimeoutDraft(parsed.value != null ? String(parsed.value) : '');
    if (parsed.value === dns.dnsTimeoutMs) return;
    patchDns('dns-timeout', { dnsTimeoutMs: parsed.value });
  }

  /** FakeIP：v2 策略下这颗开关不出现（未命中默认动作已迁到「DNS 规则」工作区），与桌面同门控。 */
  function toggleFakeIp(next: boolean): void {
    patchDns('fake-ip', fakeIpTogglePatch(next));
  }

  /*
   * v2 默认 DNS 策略。`hasDnsPolicyV2` 这个布尔**窄化不了类型**，而下面那块要逐字段读它 ⇒
   * 单取一格 `policyDefaults`，由它自己的 `!= null` 去开那一支。
   */
  const policyDefaults = hasDnsPolicyV2 ? config.dnsDefaults : undefined;

  /*
   * 两块清单的总开关。**缺席说明跟着总开关走**，与桌面同一条原则：`SettingsDns.tsx:656/698` 两处
   * 都写死了「关闭时不渲染清单：不生效的可编辑清单是误导」，故那边是 `fakeIpFilterOn && <Fold>`。
   * 移动端没有清单可渲染，但那两条**描述清单**的说明（「这些域名解析真实 IP」「DoH 端点清单」）
   * 与「去桌面端改」的缺席理由是同一块的东西：总开关关着时它们描述的是一张不生效的名单，
   * 挂在那儿会让人以为有一份正在生效的名单在起作用 —— 与桌面那句理由是同一个失败形态。
   *
   * `blockBrowserDoh` 缺省是**关**（`=== true` 判定）⇒ 全新安装的手机上，浏览器 DoH 那一块只剩
   * 开关本身与它的 `desc`，不再常驻一段「描述一张不生效清单」的说明。
   */
  const fakeIpFilterOn = config.fakeIpFilter !== false;
  const browserDohOn = config.blockBrowserDoh === true;

  /*
   * 竞速池（`nodeResolverPool`）与它的额度。三式逐条取自桌面 `SettingsDns.tsx:265-277`：
   * `system` 属兜底层不占额度；自定义条目被删后残留在 pool 里的孤儿 id 既不计数也不下发。
   *
   * 🔴 自定义 DoH 在移动端不可编辑（清单要 `ListEditor`），但它**仍占额度**：桌面已经加满 3 条时，
   * 这里必须把内置那两颗按「满额」禁掉并说明原因 —— 不禁的话 `nextRacePool` 原样返回，
   * 用户拨了开关什么都不会发生，又是一颗「拨了不生效」的开关。
   */
  const racePool = dns.nodeResolverPool ?? ['ali', 'dnspod'];
  const validRaceIds = new Set(['ali', 'dnspod', ...(dns.nodeResolverCustom ?? []).map((u) => u.id)]);
  const activeRacePool = racePool.filter((id) => id === 'system' || validRaceIds.has(id));
  const raceQuotaFull =
    new Set(activeRacePool.filter((id) => id !== 'system')).size >= MAX_DOH_RACE_UPSTREAMS;

  /**
   * 自定义 DoH 上游库存的写腿（`nodeResolverCustom` + `nodeResolverPool` 的孤儿清理）。
   *
   * 逐条同桌面 `SettingsDns.tsx#setCustomUpstreams`：
   *  · **非法项只标红不落盘** —— 落盘等于让用户在看到错误提示的同时，坏配置已经触发了一次整核
   *    重启评估；判据用共享的 `isPureIpDnsSpec`（自定义上游必须是纯 IP：它自己不能再要解析）；
   *  · id 由 `reconcileCustomUpstreams` 生成并复用，**删掉一条时同步把 pool 里那个 id 摘掉**，
   *    否则留下一个指向不存在上游的孤儿启用项，它既不计数也不下发，而界面上看不出来。
   */
  function commitCustomUpstreams(next: string[]): void {
    if (next.some((spec) => spec.trim() !== '' && !isPureIpDnsSpec(spec))) return;
    const current = dns.nodeResolverCustom ?? [];
    const nextCustom = reconcileCustomUpstreams(current, next, () => `doh-${crypto.randomUUID()}`);
    const nextIds = new Set(nextCustom.map((item) => item.id));
    const previousIds = new Set(current.map((item) => item.id));
    patchDns('race-custom-list', {
      nodeResolverCustom: nextCustom,
      nodeResolverPool: racePool.filter((id) => !previousIds.has(id) || nextIds.has(id)),
    });
  }

  /*
   * 上游预设下拉（两端同一张表，见文件头注第一条）。
   *
   * 与桌面 `SettingsDns.tsx:350-367` 同一条口径：
   *  · 当前值不在表里 ⇒ 停在「自定义…」那一档，**不改写用户填的地址**；
   *  · 选中「自定义…」这个哨兵**什么都不做** —— 它是一个显示态，不是一个可落库的值。
   *    漏掉这一支就会把字面量 `__custom__` 写进 `dnsConfig`，而 `parseDnsServerSpec` 解不出它，
   *    DNS 那一段整块回落到兜底上游而界面上还显示着「自定义」。
   *  · 选中一档预设 ⇒ **立刻落库**（不等失焦）：下拉没有「输入中」这个中间态，
   *    等失焦等于选了没反应。同时把草稿改成新值，否则输入框还显示旧地址。
   */
  function pickPreset(rowId: string, key: 'foreignDns' | 'domesticDns', value: string): void {
    if (value === DNS_PRESET_CUSTOM) return;
    const slot = key === 'foreignDns' ? 'remote' : 'domestic';
    if (key === 'foreignDns') setRemote(value);
    else setDomestic(value);
    setInvalid((p) => ({ ...p, [slot]: false }));
    if (value === dns[key]) return;
    patchDns(rowId, { [key]: value });
  }

  /** 下拉的当前档：值不在表里就停在哨兵上（`<MobileSelect>` 表达不了「不在表内」）。 */
  function presetValue(presets: readonly DnsUpstreamPreset[], draft: string): string {
    return presets.some((p) => p.value === draft) ? draft : DNS_PRESET_CUSTOM;
  }

  const remoteUpstreamPresets = remotePresets(t);
  const domesticUpstreamPresets = domesticPresets(t);

  /** 一条 DNS 地址行的控件簇：预设下拉 + 手输框（纵向排，390 宽放不下并排）。 */
  function dnsAddressControl(
    rowId: string,
    key: 'foreignDns' | 'domesticDns',
    presets: readonly DnsUpstreamPreset[],
    draft: string,
    setDraft: (next: string) => void,
    slot: 'remote' | 'domestic',
    label: string,
  ): ReactElement {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', width: '100%' }}>
        {/*
          🔴 `ariaLabel` 是**控件名**，不是它的某一档。上一版写的是
          `${label} · ${t('common.customEllipsis')}`（「远程 DNS · 自定义…」）——
          「自定义…」是这颗下拉最后那个**哨兵档**的名字，拿它当消歧词会把「只能选四档预设的下拉」
          念成「填自定义地址的地方」，而真正填地址的那颗输入框（下面 `MobileTextInput`，
          `ariaLabel={label}`）反倒没有区别性标识，两者语义正好颠倒。
          桌面同一颗 select 的可访问名是行名本身（`SettingsDns.tsx:335`），输入框另有 `<label>` 关联；
          移动端两颗控件同处一行、都要有名字，故下拉取「<行名> · 上游预设」。
        */}
        <MobileSelect
          value={presetValue(presets, draft)}
          ariaLabel={`${label} · ${t('settings.dns.upstreamPreset')}`}
          onChange={(v) => pickPreset(rowId, key, v)}
        >
          {presets.map((preset) => (
            <option key={preset.value} value={preset.value}>
              {preset.label}
            </option>
          ))}
          <option value={DNS_PRESET_CUSTOM}>{t('common.customEllipsis')}</option>
        </MobileSelect>
        <MobileTextInput
          mono
          value={draft}
          invalid={invalid[slot]}
          ariaLabel={label}
          onChange={(v) => {
            setDraft(v);
            if (invalid[slot]) setInvalid((p) => ({ ...p, [slot]: false }));
          }}
          onCommit={() => commitDns(rowId, key, draft)}
        />
      </div>
    );
  }

  /** 竞速池里的一个上游。`tier1` 决定它占不占那 3 个额度（`system` 不占）。 */
  function raceUpstreamRow(
    rowId: string,
    id: string,
    label: string,
    desc: string,
    tier1: boolean,
  ): ReactElement {
    const on = activeRacePool.includes(id);
    const blocked = tier1 && !on && raceQuotaFull;
    return (
      <SettingsRow
        id={rowId}
        label={label}
        desc={desc}
        /* `hint` 而不是 `problem`：额度用满是**配置的正常状态**（用户在桌面端加满了 3 条自定义
           DoH），不是这一屏出了错。走 `problem` 会把它画成 `--err` 红字，与同页「DNS 地址非法」
           「超时值越界」同一视觉等级 —— 桌面把同一句放在 `Switch` 的 `tip` 里，非错误态。 */
        hint={blocked ? t('settings.dns.raceQuotaReached') : undefined}
        control={
          <MobileSwitch
            checked={on}
            disabled={blocked}
            ariaLabel={label}
            onChange={(v) => patchDns(rowId, { nodeResolverPool: nextRacePool(activeRacePool, id, v) })}
          />
        }
      />
    );
  }

  return (
    <>
      <SettingsGroup header={t('settings.dns.resolverBlock')}>
        <SettingsRow
          first
          stacked
          id="connection-resolution"
          label={t('settings.dns.connectionResolution')}
          desc={t('mobileHelp.connectionResolution')}
          descDetails={t('settings.dns.connectionResolutionHint')}
          control={
            <MobileSelect
              value={effectiveDnsConnectionResolution(config)}
              ariaLabel={t('settings.dns.connectionResolution')}
              onChange={(v) =>
                commit(
                  'connection-resolution',
                  update(dnsConnectionResolutionPatch(config, v as DnsConnectionResolution)),
                )
              }
            >
              <option value="preserveDomain">{t('settings.dns.connectionPreserve')}</option>
              <option value="dnsRules">{t('settings.dns.connectionLocal')}</option>
            </MobileSelect>
          }
        />
        {policyDefaults != null && (
          <DnsDefaultsRows
            config={config}
            defaults={policyDefaults}
            dns={dns}
            update={update}
            commit={commit}
          />
        )}
        {!hasDnsPolicyV2 && (
          <SettingsRow
            id="fake-ip"
            label="FakeIP"
            desc={t('settings.dns.fakeIpDesc')}
            /* 关掉 FakeIP 后节点会收到真实 IP，部分机场按反滥用策略直接拒连 —— 客户端缓解不了。
               桌面把这句话放在确认弹窗里；移动端没有弹窗层，故它常驻在这一行下面。
               桌面那条腿（`needsFakeIpOffConfirm`）还要先判「接管方式是 TUN」，移动端**不判**：
               Android 恒 TUN（见 `mobile/home/MobileHomeScreen.tsx` 的 `MOBILE_TAKEOVER`），而盘上的
               `proxyModeType` 默认值是 `systemProxy` 且移动端已经没有控件能改它 ⇒ 照读那个字段会让
               这句风险提示在唯一需要它的平台上永不出现。 */
            problem={dns.enableFakeIp ? t('mobileSettings.dns.fakeIpOffRisk') : undefined}
            control={
              <MobileSwitch checked={dns.enableFakeIp} ariaLabel="FakeIP" onChange={toggleFakeIp} />
            }
          />
        )}
        {!hasDnsPolicyV2 && (
          <SettingsRow
            stacked
            id="remote-dns"
            label={t('settings.dns.remoteDns')}
            desc={t('settings.dns.remoteDnsDesc')}
            problem={invalid.remote ? t('settings.advanced.dnsInvalid') : undefined}
            control={dnsAddressControl(
              'remote-dns',
              'foreignDns',
              remoteUpstreamPresets,
              remote,
              setRemote,
              'remote',
              t('settings.dns.remoteDns'),
            )}
          />
        )}
        {!hasDnsPolicyV2 && (
          <SettingsRow
            stacked
            id="domestic-dns"
            label={t('settings.dns.domesticDns')}
            desc={t('settings.dns.domesticDnsDesc')}
            problem={invalid.domestic ? t('settings.advanced.dnsInvalid') : undefined}
            control={dnsAddressControl(
              'domestic-dns',
              'domesticDns',
              domesticUpstreamPresets,
              domestic,
              setDomestic,
              'domestic',
              t('settings.dns.domesticDns'),
            )}
          />
        )}
        <SettingsRow
          id="takeover-system-dns"
          label={t('settings.advanced.takeoverSystemDns')}
          desc={t('settings.dns.takeoverSystemDnsDesc')}
          control={
            <MobileSwitch
              checked={dns.takeoverSystemDns !== false}
              ariaLabel={t('settings.advanced.takeoverSystemDns')}
              onChange={(v) => patchDns('takeover-system-dns', { takeoverSystemDns: v })}
            />
          }
        />
        <SettingsRow
          id="optimistic-cache"
          label={t('settings.advanced.optimisticCache')}
          desc={t('settings.advanced.optimisticCacheDesc')}
          control={
            <MobileSwitch
              checked={dns.optimisticCache === true}
              ariaLabel={t('settings.advanced.optimisticCache')}
              onChange={(v) => patchDns('optimistic-cache', { optimisticCache: v })}
            />
          }
        />
        <SettingsRow
          stacked
          id="dns-timeout"
          label={t('settings.advanced.dnsTimeout')}
          desc={t('settings.advanced.dnsTimeoutDesc')}
          problem={invalid.timeout ? t('settings.advanced.dnsTimeoutRange') : undefined}
          control={
            <MobileTextInput
              mono
              inputMode="numeric"
              value={timeout}
              invalid={invalid.timeout}
              placeholder={t('settings.advanced.dnsTimeoutPlaceholder')}
              ariaLabel={t('settings.advanced.dnsTimeout')}
              onChange={(v) => {
                setTimeoutDraft(v);
                if (invalid.timeout) setInvalid((p) => ({ ...p, timeout: false }));
              }}
              onCommit={commitTimeout}
            />
          }
        />
      </SettingsGroup>

      <SettingsGroup header={t('settings.dns.nodeResolverBlock')}>
        <SettingsRow
          first
          stacked
          id="race-strategy"
          label={t('settings.dns.raceStrategy')}
          desc={t('settings.dns.raceStrategyDesc')}
          control={
            <MobileSelect
              value={dns.resolveNodeDomainsAhead === false ? 'single' : 'race'}
              ariaLabel={t('settings.dns.raceStrategy')}
              onChange={(v) => patchDns('race-strategy', { resolveNodeDomainsAhead: v === 'race' })}
            >
              <option value="race">{t('settings.dns.raceMode')}</option>
              <option value="single">{t('settings.dns.singleMode')}</option>
            </MobileSelect>
          }
        />
        {/* race 关：单上游选择器（写 `nodeResolverSingle`，与 pool 各存各的、切档互不覆盖）。
            没有它，选了「单上游」就没有任何地方能指定那个上游。 */}
        {dns.resolveNodeDomainsAhead === false && (
          <SettingsRow
            stacked
            id="node-resolver-single"
            label={t('settings.advanced.nodeResolverSingleLabel')}
            desc={t('settings.advanced.nodeResolverSingleHint')}
            control={
              <MobileSelect
                value={singleUpstream}
                ariaLabel={t('settings.advanced.nodeResolverSingleLabel')}
                onChange={(v) => patchDns('node-resolver-single', { nodeResolverSingle: v })}
              >
                <option value="ali">{t('settings.advanced.nodeResolverAli')}</option>
                <option value="dnspod">{t('settings.advanced.nodeResolverDnspod')}</option>
                <option value="system">{t('settings.advanced.nodeResolverSystem')}</option>
              </MobileSelect>
            }
          />
        )}
        {/* race 开：竞速池的三个上游。桌面在这里还有一张自定义 DoH 的 `ListEditor`，本屏不移植 ——
            但**开关不能跟着一起消失**：池是配置里活着的字段，看不见就改不了。 */}
        {dns.resolveNodeDomainsAhead !== false && (
          <>
            {raceUpstreamRow(
              'race-pool-ali',
              'ali',
              `${t('settings.dns.brandAli')} DoH`,
              '223.5.5.5',
              true,
            )}
            {raceUpstreamRow('race-pool-dnspod', 'dnspod', 'DNSPod DoH', '1.12.12.12', true)}
            {raceUpstreamRow(
              'race-pool-system',
              'system',
              t('settings.advanced.nodeResolverSystem'),
              t('settings.dns.noQuotaTag'),
              false,
            )}
            <SettingsNote id="race-upstreams-quota" title={t('settings.dns.raceStrategy')} summary={t('mobileHelp.raceUpstreams')}>{t('settings.dns.raceUpstreamsDesc')}</SettingsNote>
            {/* 自定义 DoH 上游库存。桌面是一整块 `ListEditor`，这里是它的移动端对位
                （`MobileListEditor`，去重与草稿规则读同一份 `@/domain/list-entries`）。
                它**必须在场**：自定义条目照样占竞速额度，看不见就解释不了内置那两颗为什么被禁。 */}
            <SettingsRow
              stacked
              id="race-custom-list"
              label={t('settings.dns.addCustomDoh')}
              desc={t('settings.dns.raceCustomHint')}
              control={
                <MobileListEditor
                  id="race-custom-entries"
                  value={(dns.nodeResolverCustom ?? []).map((item) => item.spec)}
                  onChange={commitCustomUpstreams}
                  placeholder="https://1.1.1.1/dns-query"
                  ariaLabel={t('settings.dns.addCustomDoh')}
                  addLabel={t('settings.dns.addCustomDoh')}
                  importLabel={t('common.bulkImport')}
                  importHint={t('settings.listImportHint')}
                  removeLabel={t('common.delete')}
                  confirmLabel={t('common.confirm')}
                  cancelLabel={t('common.cancel')}
                  emptyLabel={t('settings.dns.raceCustomEmpty')}
                />
              }
            />
          </>
        )}
      </SettingsGroup>

      {/* FakeIP 例外域名。总开关缺省/`true` 都是开，仅显式 `false` 关 —— 与后端
          `fake_ip_filter != Some(false)`（`builder/dns.rs`）及桌面 `SettingsDns.tsx:255` 同口径。 */}
      <SettingsGroup header={t('settings.advanced.fakeIpFilter')}>
        <SettingsRow
          first
          id="fakeip-filter"
          label={t('settings.advanced.fakeIpFilter')}
          desc={t('settings.advanced.fakeIpFilterDesc')}
          control={
            <MobileSwitch
              checked={fakeIpFilterOn}
              ariaLabel={t('settings.advanced.fakeIpFilter')}
              onChange={(v) => commit('fakeip-filter', update({ fakeIpFilter: v }))}
            />
          }
        />
        {fakeIpFilterOn && (
          <>
            <SettingsNote id="fakeip-filter-list">{t('settings.dns.fakeIpFilterHint')}</SettingsNote>
            {/* 例外域名清单。缺省值与桌面 `SettingsDns.tsx:261` 逐值同 —— 缺省不同会让同一份配置
                在两端显示成两张不同的名单，而这张名单决定哪些域名拿到真实 IP。 */}
            <SettingsRow
              stacked
              id="fakeip-filter-list-editor"
              label={t('settings.dns.fakeIpFilterFold')}
              control={
                <MobileListEditor
                  id="fakeip-filter-entries"
                  value={config.fakeIpFilterList ?? ['time.*.com', 'stun.*.*', 'captive.apple.com']}
                  onChange={(next) => commit('fakeip-filter-list-editor', update({ fakeIpFilterList: next }))}
                  placeholder="example.com"
                  ariaLabel={t('settings.dns.fakeIpFilterFold')}
                  addLabel={t('settings.dns.addDomain')}
                  importLabel={t('common.bulkImport')}
                  importHint={t('settings.listImportHint')}
                  removeLabel={t('common.delete')}
                  confirmLabel={t('common.confirm')}
                  cancelLabel={t('common.cancel')}
                  emptyLabel={t('settings.dns.fakeIpFilterEmpty')}
                />
              }
            />
          </>
        )}
      </SettingsGroup>

      {/* 浏览器内置 DoH 阻断。与上面相反：**默认关**（`=== true` 判定，`undefined` 不算开）——
          它是新增能力，不替用户做决定，同桌面 `SettingsDns.tsx:262`。 */}
      <SettingsGroup header={t('settings.dns.browserDohTitle')}>
        <SettingsRow
          first
          id="block-browser-doh"
          label={t('settings.dns.browserDohTitle')}
          desc={t('mobileHelp.browserDoh')}
          descDetails={t('settings.dns.browserDohDesc')}
          control={
            <MobileSwitch
              checked={browserDohOn}
              ariaLabel={t('settings.dns.browserDohTitle')}
              onChange={(v) => commit('block-browser-doh', update({ blockBrowserDoh: v }))}
            />
          }
        />
        {browserDohOn && (
          <>
            {/* 这块的清单说明取 `browserDohFold`（「DoH 端点清单」）而**不是** `browserDohHint`。
                后者是写给能编辑清单的客户端的：它以「遇到漏网的**自行添加**」收尾，而本屏没有
                添加入口，下一行紧接着还说「请在桌面端修改后同步」—— 一句让你去做、下一句告诉你
                做不了。承诺的真值在用户可见文案里，不从机制反推：那句祈使在移动端兑现不了，
                就不该出现在移动端。`browserDohFold` 只**命名**这块缺席的清单是什么，不含动作指令，
                与下面那条缺席理由合起来正好是 IA §3.3 规则 2 要的「缺什么 + 为什么缺」。
                （对照：`fakeIpFilterHint` 是纯描述、无祈使，故上面那块原样保留。） */}
            {/* 🔴 说明取回桌面那句 `browserDohHint`（含「遇到漏网的**自行添加**」）——
                上一版刻意换成纯命名的 `browserDohFold`，理由是「那句祈使在移动端兑现不了」。
                本批清单编辑器接上之后，那个前提不再成立：添加入口就在下面这一格。
                承诺的真值在用户可见文案里，而现在这句祈使是**能兑现**的。 */}
            <SettingsNote id="browser-doh-list" title={t('settings.dns.browserDohFold')} summary={t('mobileHelp.browserDohList')}>{t('settings.dns.browserDohHint')}</SettingsNote>
            <SettingsRow
              stacked
              id="browser-doh-list-editor"
              label={t('settings.dns.browserDohFold')}
              control={
                <MobileListEditor
                  id="browser-doh-entries"
                  value={config.browserDohList ?? [...DEFAULT_BROWSER_DOH_SUFFIXES]}
                  onChange={(next) => commit('browser-doh-list-editor', update({ browserDohList: next }))}
                  placeholder="dns.example.com"
                  ariaLabel={t('settings.dns.browserDohFold')}
                  addLabel={t('settings.dns.addDomain')}
                  importLabel={t('common.bulkImport')}
                  importHint={t('settings.listImportHint')}
                  removeLabel={t('common.delete')}
                  confirmLabel={t('common.confirm')}
                  cancelLabel={t('common.cancel')}
                  emptyLabel={t('settings.dns.browserDohEmpty')}
                />
              }
            />
          </>
        )}
      </SettingsGroup>
    </>
  );
}

/**
 * v2 的**默认 DNS 策略**：未命中默认动作 + Hosts 档的兜底服务器。
 *
 * # 为什么这两格落在设置页，而不是规则屏的 DNS 分段
 *
 * 桌面把它们画在 `DnsPolicyWorkspace.tsx` 的 `view === 'rules'` 那一支（规则列表顶上一张卡），
 * 因为桌面那一屏本来就是「DNS 规则 + 它的默认」一整块。移动端不同：
 *  · 写这一格的同一次 `update` 还要连带改写 `dnsConfig.enableFakeIp` / `fakeIpTunAutoEnable`
 *    （共用 `fakeIpTogglePatch`，见下面 `commitDefaultAction`）—— 而 legacy 那颗 FakeIP 开关
 *    就在本卡下面几行。两者分居两屏时，v2 盘上的用户看到的是「FakeIP 开关没了，也没说去哪」，
 *    正是 §4 开头点名的那个形态；
 *  · 本页已经持有 `hasDnsPolicyV2` 这条分叉（远程/国内 DNS 三行也跟着它走），把默认策略接在
 *    同一条分叉的另一侧，v1↔v2 两档在同一张卡里读起来是同一件事的两种形态。
 *
 * # 复用与不复用
 *
 * · 候选表、当前档、档位→动作的反解，三样全部走共享纯模块 `dialogs/dns-action-options`
 *   （`buildDnsActionGroups` / `dnsDefaultActionChoice` / `dnsDefaultFallbackChoice` /
 *   `dnsActionFromChoice`）。后两个是本批从桌面那份 `.tsx` 搬出来的 —— 它们含两个**缺省约定**，
 *   两端各写一份时漂了不会有任何门红。
 * · 不复用的只有桌面组件：`Csel` 是锚在触发器上的浮动菜单，触屏没有 hover 去开、没有光标去锚。
 *   这里换成 `select-sheet`（IA §3.3），组件直接复用移动端规则屏那一份 `SelectSheetPanel`
 *   —— 见 `collapsibleGroups` 头注。
 *
 * # 写腿
 *
 * 两格共用 `commitDefaultAction`：行 id 由调用点逐字给，写失败的红字贴在动手的那一行下面
 * （`SettingsRow` 按 id 自取），与本页其余写腿同一条通道。
 */
function DnsDefaultsRows({
  config,
  defaults,
  dns,
  update,
  commit,
}: {
  config: UserConfig;
  defaults: DnsDefaults;
  dns: DnsConfig;
  update: MobileSettingsPageProps['update'];
  commit: MobileSettingsPageProps['commit'];
}): ReactElement {
  const { t } = useTranslation();
  /* 同屏只许开一个面板：两个一起开是误触面（同 `forms/RuleFormPanel.tsx` 那条）。 */
  const [sheet, setSheet] = useState<'action' | 'fallback' | null>(null);

  const actionValue = dnsDefaultActionChoice(defaults);
  const fallbackValue = dnsDefaultFallbackChoice(defaults);
  const resources = {
    servers: config.dnsServers ?? [],
    groups: config.dnsServerGroups ?? [],
    nodes: config.servers ?? [],
    t,
    /* 逐字同桌面：未命中默认没有「预定义记录」的编辑器，故不把 `predefined` 摆进来 ——
       一个只能选中、选中之后没有地方填内容的档位，与控件坏了在用户眼里一样。 */
    responses: ['fakeIp', 'reject'] as const,
  };
  const actionGroups = collapsibleGroups(
    buildDnsActionGroups({ ...resources, currentValue: actionValue }),
  );
  /* Hosts 的兜底里不再列 Hosts（同桌面 `includeHosts: false`）：Hosts 未命中之后再兜一个 Hosts
     是一条走不通的链。 */
  const fallbackGroups = collapsibleGroups(
    buildDnsActionGroups({ ...resources, currentValue: fallbackValue, includeHosts: false }),
  );
  const hostsFirst = defaults.unmatchedAction?.type === 'hostsFirst';

  /**
   * 默认策略的唯一写腿。
   *
   * 🔴 `dnsConfig` 那一半**必须一起写**（逐字同桌面 `DnsPolicyWorkspace.tsx#commitDefaultAction`）：
   * v1 的 `enableFakeIp` 是同一件事的 legacy 镜像，只改 v2 那一半会让两份配置各说各话 ——
   * 界面按 v2 显示「已改成直连」，而按 legacy 生成的那一段仍在发 FakeIP。补丁本体共用
   * `fakeIpTogglePatch`（与本页那颗 v1 开关同一个函数），不在这里重写两个键。
   */
  function commitDefaultAction(rowId: string, unmatchedAction: DnsPolicyAction): void {
    commit(
      rowId,
      update({
        dnsConfig: { ...dns, ...fakeIpTogglePatch(unmatchedAction.type === 'fakeIp') },
        dnsDefaults: { ...defaults, unmatchedAction },
      }),
    );
  }

  return (
    <>
      <SettingsRow
        stacked
        id="dns-default-action"
        label={t('settings.dns.defaultUnmatched')}
        desc={t('settings.dns.defaults')}
        control={
          <SelectSheetTrigger
            label={t('settings.dns.defaultUnmatched')}
            options={[]}
            value={actionValue}
            /* 当前档的名字是**运行期数据**（用户自建的 DNS 服务器 / 组都在候选里），
               选项表放不下它 ⇒ 走 `valueLabel` 这一格，与应用分流的「指定节点」同一条。 */
            valueLabel={labelOf(actionGroups, actionValue)}
            open={sheet === 'action'}
            onOpen={() => setSheet('action')}
          />
        }
      />
      {/* Hosts 优先这一档才有兜底可言（同桌面：非 hostsFirst 时整行不画，不画成禁用）。 */}
      {hostsFirst && (
        <SettingsRow
          stacked
          id="dns-default-fallback"
          label={t('rules.dnsHostsFallback')}
          control={
            <SelectSheetTrigger
              label={t('rules.dnsHostsFallback')}
              options={[]}
              value={fallbackValue}
              valueLabel={labelOf(fallbackGroups, fallbackValue)}
              open={sheet === 'fallback'}
              onOpen={() => setSheet('fallback')}
            />
          }
        />
      )}
      {/* 两张面板一律渲染在行之外（`position: fixed` 的层不该嵌在行的盒里）。 */}
      <SelectSheetPanel
        label={t('settings.dns.defaultUnmatched')}
        groups={actionGroups}
        value={actionValue}
        openGroupIds={openGroupOf(actionGroups, actionValue)}
        open={sheet === 'action'}
        onClose={() => setSheet(null)}
        onSelect={(next) => {
          setSheet(null);
          /* 第二个实参是**当前**的 fallback：选中「Hosts 优先」那一下若不把它带上，
             `dnsActionFromChoice` 会给出一个 fallback 恒为 FakeIP 的动作 —— 用户的兜底被悄悄换掉。 */
          commitDefaultAction('dns-default-action', dnsActionFromChoice(next, fallbackValue));
        }}
        closeLabel={t('common.close')}
      />
      <SelectSheetPanel
        label={t('rules.dnsHostsFallback')}
        groups={fallbackGroups}
        value={fallbackValue}
        openGroupIds={openGroupOf(fallbackGroups, fallbackValue)}
        open={sheet === 'fallback'}
        onClose={() => setSheet(null)}
        onSelect={(next) => {
          setSheet(null);
          if (defaults.unmatchedAction?.type !== 'hostsFirst') return;
          /* 改的是 fallback，主动作原样重拼（同桌面 `setDefaultFallback`）。
             行 id 与写腿同处一行：`MobileSettings.test.tsx ⑩` 的写腿解析器按
             `<中转>('<行 id>'` 取，拆成多行虽仍扫得到，但那条判据的可读性靠的就是这一行。 */
          const hostsChoice = `hosts:${defaults.unmatchedAction.hostsServerId}`;
          commitDefaultAction('dns-default-fallback', dnsActionFromChoice(hostsChoice, next));
        }}
        closeLabel={t('common.close')}
      />
    </>
  );
}
