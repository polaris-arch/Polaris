/**
 * 设置 → TUN（移动端）。桌面对照：`components/screens/settings/SettingsTun.tsx`（四块）。
 *
 * # 🔴 `strict_route` 开关**不显示**（U2，主会话裁定，模拟器已实证）
 *
 * 实证：起隧道前后 `iptables -S` / `ip6tables -S` **md5 逐字节相同**（各 67 行）⇒ Android 内核侧
 * 一条防绕行规则都没装。正对照是同时刻 `ip rule` 由 22 行涨到 36 行、新增 14 条全是 netd 签名 ⇒
 * 观测手段确实看得见「规则被装上」这件事，前一条的相等不是量错了。
 *
 * 判据：**一个承诺了「严格锁入 TUN 防止绕行泄漏」却一行都不跑的开关，比没有这个开关更坏** ——
 * 用户会照着它行动（以为泄漏被堵住了）。故 UI 上不出现。
 *
 * ⚠️ **发射面不改**：配置生成侧照发 `strict_route`。改生成侧只是把空转藏起来，而承诺仍挂在 UI 上；
 * 本批只动 UI 这一侧，内核侧那条债由 U2 归口。
 *
 * # 其余去留（IA §2.4 / §4.16）
 *
 *  ✔ MTU / 自动路由 / NAT 类型 / IPv6（含 FakeIP 联动提示）
 *  ✔ **绕过局域网（`bypassLAN` / `bypassLANList`，2026-09-25 接线）**。上一版在这里判「真平台缺席」，
 *    依据是 `crates/config-engine/src/builder/inbounds.rs` 的 android 臂恒空 + 那条
 *    「`VpnService.Builder.excludeRoute` 拒收 `127.0.0.0/8`」的模拟器实证 —— 那条实证只覆盖**回环**，
 *    不覆盖 RFC1918；恒空是本仓的选择，不是平台限制。现在 android / ios 臂在开关开着时发清单的网段
 *    子集（回环在生成侧剔掉，TUN 自身网段与生效组网段挖掉，见 `mobile_bypass_lan_exclude`）。
 *    开关与清单复用桌面同两个字段（桌面 `SettingsNetwork.tsx`），开关显示态复用
 *    `settings-logic.ts#bypassLanState`（关掉时隐藏清单 —— 清单此时一条都不生效）。
 *    ⚠️ 只有网段条目在本机生效：域名条目在桌面喂系统代理旁路，移动端没有那个对象，行内 desc 如实说明。
 *
 *  ✔ **「连入来源排除」（2026-09-06 接线）**。它与上一条**不是**同一件事，此前被并成了一条。
 *    它是**用户显式声明**的 `tunConfig.inboundExcludeCidrs`，在 Android 上**有活的消费方**：
 *    `inbounds.rs:406` 只对 `platform == "linux"` 短路告警，其余平台（含 android）走
 *    `:418 compute_user_tun_exclude(...)` → `:484 exclude_addr.extend(user_exclude.extra)`
 *    → `:614-615 tun.route_exclude_address = Some(...)`；承载侧 `PolarisVpnService.kt:131-132`
 *    逐条 `excludeRouteTolerantly` 真发。⇒ 这个字段在这台设备上会真的发射并生效，
 *    而上一版 UI 上**没有任何地方能设置它**（桌面在 `SettingsTun.tsx:389-402`）——
 *    那正是本仓 §4 自己点名的缺陷形态「配置里在、界面上看不见」。本批把它移植过来。
 *    清单编辑器用 `SettingsChrome` 的 `MobileListEditor`（桌面 `ListEditor` import 的是
 *    `screens/settings/Primitives` → `dialogs/Csel`，整条桌面层叠链，契约 A1 不许进移动入口）；
 *    **去重与草稿规则不重写**，三条纯函数在 `@/domain/list-entries`，两端同一份。
 *    ⚠️ **不移植** `settings.tun.inboundLinuxNote` 那条提示：它说的是「Linux 下此项不生效」，
 *    在 Android 上是一句与本机无关的话。
 *
 *    另：上一版这条引的 `inbounds.rs:234` **行号已失效** —— 那一行今天是 `udp_nat_behaviors`，
 *    与排除网段无关。
 *  ✘ **局域网网关（邻居短名解析 / MAC 过滤）**：内核侧只有 Linux/macOS 有实现，移动端发射即无效。
 *  ✘ **三平台机制说明的 `<details>`**：说的是 macOS/Windows/Linux 三家的路由机制，移动端一条都不适用。
 */

import { useEffect, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import type { TunModeConfig, UdpNatType } from '@/contracts/types';
import type { EndpointForceRouteReport } from '@/contracts/endpoint-force-route-report';
import { MTU_MAX, MTU_MIN, parseMtuInput } from '@/domain/tun-mtu';
import { injectedList, injectedRecord } from '@/domain/effective-config';
import { bypassLanState } from '@/components/screens/settings/settings-logic';
import { api } from '@/ipc/api-client';
import {
  MobileListEditor,
  MobileSelect,
  MobileSwitch,
  MobileTextInput,
  SettingsGroup,
  SettingsRow,
} from './SettingsChrome';
import { MobileEndpointForceRouteBlock } from './TunReports';
import type { MobileSettingsPageProps } from './settings-pages';

/** `'default'` 是只存在于控件里的哨兵，落库时删键（Select 的 value 表达不了 undefined）。 */
const NAT_TYPE_OPTIONS: readonly { value: UdpNatType | 'default'; key: string }[] = [
  { value: 'default', key: 'settings.tun.natTypeDefault' },
  { value: 'fullCone', key: 'settings.tun.natTypeFullCone' },
  { value: 'restrictedCone', key: 'settings.tun.natTypeRestricted' },
  { value: 'portRestrictedCone', key: 'settings.tun.natTypePortRestricted' },
];

export function TunPage({ config, update, commit }: MobileSettingsPageProps): ReactElement {
  const { t } = useTranslation();
  /* 生效值由 `config:get` 边界注入（Rust `user_config::effective_view::ensure_effective_config`）。
     这里**不许**写 `?? {…}` 兜底：那份兜底就是与内核默认分叉的第二真值源 —— `bypassLANList` /
     `inboundExcludeCidrs` 两次都栽在同一个形态上（UI 与内核各说一套 + 用户按下第一个键就把
     兜底当成自己的清单落了库）。缺席 ⇒ 报错 + 空对象，界面看起来就是坏的，而不是显示一份
     看似正常、实则与内核不符的配置。判据在 `crates/config-engine/tests/frontend_sot_guard.rs`。 */
  const tun = injectedRecord<TunModeConfig>(config.tunConfig, 'tunConfig');

  /** 写一段 TUN 配置。**必须带行 id**：写失败要贴在动手的那一行下面。 */
  function patchTun(rowId: string, patch: Partial<TunModeConfig>): void {
    commit(rowId, update({ tunConfig: { ...tun, ...patch } }));
  }

  /**
   * 连入来源排除的网段清单。
   *
   * **空表要删键**，不写 `[]`。
   *
   * ⚠️ **理由 2026-09-06 更正**（上一版给的因果链是假的）：原文写的是「配置生成侧
   * `inbounds.rs:418 compute_user_tun_exclude` 判的是有没有这一项」——引擎侧**不存在**这条判据。
   * `crates/config-engine/src/builder/inbounds.rs:401-405` 是
   * `.and_then(|t| t.inbound_exclude_cidrs.as_deref()).unwrap_or(&[])`，随后两支都以
   * `!user_inbound_cidrs.is_empty()` 开路 ⇒ `Some([])` 与 `None` 产出逐字节相同的配置、相同的告警。
   *
   * 真正的理由有两条，都不在引擎：
   *  · 存储层已经这么规定了：`crates/store/src/sanitize.rs:682 sanitize_tun_config` 的头注写着
   *    「空/全非法 → 删字段避免 `[]` 触发 norm 翻转」——写 `[]` 会被它当场删掉，等于让 UI 落一个
   *    落不住的形态，回读时又变回缺席（界面与磁盘各说各话的经典入口）。
   *  · 与本页 MTU 那条「清空 = 回到自动 ⇒ 删键」同一口径：不在 config.json 里留一条什么都不声明的空项。
   */
  function commitInboundExclude(next: string[]): void {
    const cleaned = next.map((s) => s.trim()).filter((s) => s !== '');
    const nextTun: TunModeConfig = { ...tun };
    if (cleaned.length === 0) delete nextTun.inboundExcludeCidrs;
    else nextTun.inboundExcludeCidrs = cleaned;
    commit('tun-inbound-exclude', update({ tunConfig: nextTun }));
  }

  const bypassLan = bypassLanState(config);

  const [mtu, setMtu] = useState(tun.mtu === undefined ? '' : String(tun.mtu));
  const [mtuInvalid, setMtuInvalid] = useState(false);
  useEffect(() => {
    setMtu(tun.mtu === undefined ? '' : String(tun.mtu));
    setMtuInvalid(false);
  }, [tun.mtu]);

  /** 清空 = 回到自动：**删键**而不是写 `undefined`，落盘才是真缺席。 */
  function commitMtu(): void {
    const parsed = parseMtuInput(mtu);
    if (parsed.invalid) {
      setMtuInvalid(true);
      return;
    }
    setMtuInvalid(false);
    const next: TunModeConfig = { ...tun };
    if (parsed.mtu === undefined) delete next.mtu;
    else next.mtu = parsed.mtu;
    commit('tun-mtu', update({ tunConfig: next }));
  }

  function commitNatType(v: UdpNatType | 'default'): void {
    const next: TunModeConfig = { ...tun };
    if (v === 'default') delete next.udpNatType;
    else next.udpNatType = v;
    commit('tun-nat-type', update({ tunConfig: next }));
  }

  /**
   * IPv6 开着而 FakeIP 关着时，不支持 IPv6 的节点会连不上 —— 桌面那条提示还要先判「接管方式是
   * TUN」，移动端**不判**：Android 恒 TUN（见 `mobile/home/MobileHomeScreen.tsx` 的
   * `MOBILE_TAKEOVER`），而盘上的 `proxyModeType` 默认值是 `systemProxy` 且移动端已经没有控件能改
   * 它 ⇒ 照读那个字段会让这条提示**永远不出现**，正好在唯一需要它的平台上失声。
   */
  /*
   * 自己的组网网段结算报告拉取（呈现在 `TunReports.tsx`）。
   *
   * 拉不到停在 `null` —— 报告自己说「读取失败，暂时拿不到这份报告」，
   * **不折成任何一种结论**。这与节点屏 `MobileNodesScreen.tsx:217-231` 消费同一条
   * force-route 命令时的取向逐字一致：报告答不出来是允许的，编一个答案不是。
   *
   * 只在挂载时拉一次：报告按当前保存配置和观测计算，
   * 页面停留期间不会自己变新。
   */
  const [forceRoute, setForceRoute] = useState<EndpointForceRouteReport | null>(null);
  useEffect(() => {
    let cancelled = false;
    api.config
      .endpointForceRouteReport()
      .then((next) => {
        if (!cancelled) setForceRoute(next);
      })
      .catch(() => {
        if (!cancelled) setForceRoute(null);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const fakeIpEnabled =
    (config.configSchemaVersion ?? 0) >= 2 && config.dnsDefaults
      ? config.dnsDefaults.unmatchedAction?.type === 'fakeIp'
      : (config.dnsConfig?.enableFakeIp ?? true);
  const ipv6Hint = !!config.enableIPv6 && !fakeIpEnabled;

  return (
    <>
    <SettingsGroup header={t('settings.tun.takeoverBlock')}>
      {/* 协议栈选择器已随上游 TUN `stack` 弃用整体移除（桌面同批）。MTU 留空 = 不下发，由内核按运行环境
          取默认（Polaris 不持有平台 → MTU 表），故占位符只说「内核默认」、不写数字。 */}
      <SettingsRow
        first
        stacked
        id="tun-mtu"
        label="MTU"
        desc={t('settings.tun.mtuDesc')}
        problem={mtuInvalid ? t('settings.tun.mtuInvalid', { min: MTU_MIN, max: MTU_MAX }) : undefined}
        control={
          <MobileTextInput
            mono
            inputMode="numeric"
            value={mtu}
            invalid={mtuInvalid}
            ariaLabel="MTU"
            placeholder={t('settings.tun.mtuAutoPlaceholder')}
            onChange={(v) => {
              setMtu(v);
              setMtuInvalid(false);
            }}
            onCommit={commitMtu}
          />
        }
      />
      <SettingsRow
        id="tun-auto-route"
        label={t('settings.tun.autoRoute')}
        desc={t('settings.tun.autoRouteDesc')}
        control={
          <MobileSwitch
            checked={tun.autoRoute}
            ariaLabel={t('settings.tun.autoRoute')}
            onChange={(v) => patchTun('tun-auto-route', { autoRoute: v })}
          />
        }
      />
      <SettingsRow
        stacked
        id="tun-nat-type"
        label={t('settings.tun.natType')}
        desc={t('settings.tun.natTypeDesc')}
        control={
          <MobileSelect
            value={tun.udpNatType ?? 'default'}
            ariaLabel={t('settings.tun.natType')}
            onChange={(v) => commitNatType(v as UdpNatType | 'default')}
          >
            {NAT_TYPE_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {t(o.key)}
              </option>
            ))}
          </MobileSelect>
        }
      />
      <SettingsRow
        id="tun-ipv6"
        label={t('settings.general.enableIPv6')}
        desc={t('mobileHelp.ipv6')}
          descDetails={t('settings.network.enableIPv6Desc')}
        problem={ipv6Hint ? t('settings.network.ipv6NodeFakeIpHint') : undefined}
        control={
          <MobileSwitch
            checked={!!config.enableIPv6}
            ariaLabel={t('settings.general.enableIPv6')}
            onChange={(v) => commit('tun-ipv6', update({ enableIPv6: v }))}
          />
        }
      />
    </SettingsGroup>
      {/*
        绕过局域网（桌面 `SettingsNetwork.tsx` 的总开关 + 旁路清单，同两个字段）。清单由 `config:get`
        边界注入生效值（缺席即 27 条 `DEFAULT_BYPASS_LAN`），`injectedList` 不兜底，理由同本页头注。
      */}
      <SettingsGroup header={t('settings.tun.bypassLanBlock')}>
        <SettingsRow
          first
          id="tun-bypass-lan"
          label={t('settings.advanced.bypassLAN')}
          desc={t('mobileHelp.bypassLan')}
          descDetails={t('settings.tun.bypassLanDesc')}
          control={
            <MobileSwitch
              checked={bypassLan.checked}
              ariaLabel={t('settings.advanced.bypassLAN')}
              onChange={(v) => commit('tun-bypass-lan', update({ bypassLAN: v }))}
            />
          }
        />
        {bypassLan.showList && (
          <SettingsRow
            stacked
            id="tun-bypass-lan-list"
            label={t('settings.network.bypassFold')}
            desc={t('settings.tun.bypassLanListHint')}
            control={
              <MobileListEditor
                id="tun-bypass-lan-list"
                value={injectedList(config.bypassLANList, 'bypassLANList')}
                onChange={(next) => commit('tun-bypass-lan-list', update({ bypassLANList: next }))}
                placeholder="172.16.0.0/12"
                ariaLabel={t('settings.network.bypassFold')}
                addLabel={t('settings.tun.addCidr')}
                importLabel={t('common.bulkImport')}
                importHint={t('settings.listImportHint')}
                removeLabel={t('common.delete')}
                confirmLabel={t('common.confirm')}
                cancelLabel={t('common.cancel')}
                emptyLabel={t('settings.tun.bypassLanListEmpty')}
              />
            }
          />
        )}
      </SettingsGroup>
      {/*
        连入来源排除（桌面 `SettingsTun.tsx:383-408` 那一块）。桌面把清单折进 `Fold`，
        移动端**不折**：这一块只有一张清单，折叠只是在一次点击背后藏一个已经很小的东西，
        而 §4.12 要的是常驻可达。桌面的 `tip` 落成常驻 `desc`（同本页其余行）。
      */}
      <SettingsGroup header={t('settings.tun.inboundExcludeBlock')}>
        <SettingsRow
          first
          stacked
          id="tun-inbound-exclude"
          label={t('settings.tun.inboundExcludeFold')}
          desc={t('settings.tun.inboundExcludeHint')}
          control={
            <MobileListEditor
              id="tun-inbound-exclude"
              value={injectedList(tun.inboundExcludeCidrs, 'tunConfig.inboundExcludeCidrs')}
              onChange={commitInboundExclude}
              placeholder="100.64.0.0/10"
              ariaLabel="CIDR"
              addLabel={t('settings.tun.addCidr')}
              importLabel={t('common.bulkImport')}
              importHint={t('settings.listImportHint')}
              removeLabel={t('common.delete')}
              confirmLabel={t('common.confirm')}
              cancelLabel={t('common.cancel')}
              emptyLabel={t('settings.tun.inboundExcludeEmpty')}
            />
          }
        />
      </SettingsGroup>
      {/* Android/iOS 无法完整探测其他应用的 VPN 路由；这里只保留自己的组网结算。 */}
      <MobileEndpointForceRouteBlock report={forceRoute} servers={config.servers ?? []} />
    </>
  );
}
