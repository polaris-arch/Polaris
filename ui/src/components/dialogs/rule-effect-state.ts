/**
 * 规则「效果」两格的**平台无关状态** —— 目标出站的值编码、DNS 效果的整份草稿与候选。
 *
 * # 为什么搬出 `.tsx`
 *
 * 两端的规则表单要落**同一条**效果语义：`target` 的编码（`proxy` / `direct` / `block` /
 * `node:<id>`）是 `rule-submit.ts` 与下拉之间的协议，DNS 那八格的初值反解更是一张
 * 「旧配置怎么读回草稿」的判据表。它们此前住在 `RuleRouteEffect.tsx` / `RuleDnsEffect.tsx`
 * 里，而从 `.tsx` 取一个 hook 会把整棵桌面字段组件树（连 `Csel` / `FlagImg`）拖进移动端闭包
 * —— 与「取类型一律从小写 `field-spec`」同一条约束。
 *
 * 呈现层仍各写各的：桌面那两个 `*Fields` 组件、移动端的 `RuleFormPanel` 面板。
 * 搬过来的**一行判据都没有改**，桌面调用点由那两个 `.tsx` 原样再导出承接。
 */

import { useEffect, useMemo, useState } from 'react';
import type { TFunction } from 'i18next';
import type {
  BuiltinDhcpStatus,
  DnsServerGroup,
  DnsServerResource,
  RuleDnsAnswerMode,
  RuleDnsEffect,
  RuleDnsResolver,
  RuleRouteEffect,
  ServerConfig,
  UserConfig,
} from '@/contracts/types';
import { api } from '@/ipc';
import { buildDnsActionGroups, dnsActionChoice } from './dns-action-options';

/**
 * 存量 route 效果 → 目标出站下拉的值。
 *
 * 这个编码是**下拉与提交腿之间的协议**（`rule-submit.ts` 按 `node:` 前缀反解 `targetServerId`），
 * 两端必须逐字相同：写错一档的后果不是显示不对，是把一条「代理到香港 01」的规则存成「默认代理」。
 * 注意 `proxy` 有两个来源 —— 没有 effect（新建）与「proxy 但没指定节点」（快选默认代理）。
 */
export function ruleRouteTargetChoice(baseRouteEffect: RuleRouteEffect | null): string {
  if (!baseRouteEffect) return 'proxy';
  if (baseRouteEffect.action === 'direct') return 'direct';
  if (baseRouteEffect.action === 'block') return 'block';
  return baseRouteEffect.targetServerId ? `node:${baseRouteEffect.targetServerId}` : 'proxy';
}

/**
 * dhcp transport（`dns-netenv`）在本机是否可用：`api.networkProfile.builtinDhcpStatus()`，后端判据即生成侧
 * `ProbeFacts::dhcp_unavailable`（内置 DHCP 解析器与 dhcp 源场景**同一个** transport、同一个判据）。
 * 拿不到 ⇒ `null`（调用方不置灰、不猜）。
 *
 * 规则表单的 DNS 动作下拉与移动端场景表单的「DHCP」那一档共用它 —— 同一个事实不取两次、不各写一份。
 */
export function useBuiltinDhcpStatus(): BuiltinDhcpStatus | null {
  const [status, setStatus] = useState<BuiltinDhcpStatus | null>(null);
  useEffect(() => {
    let active = true;
    api.networkProfile
      .builtinDhcpStatus()
      .then((next) => {
        if (active) setStatus(next && typeof next.available === 'boolean' ? next : null);
      })
      .catch(() => {
        if (active) setStatus(null);
      });
    return () => {
      active = false;
    };
  }, []);
  return status;
}

/**
 * DNS 效果的状态 —— resolver / answerMode / action（+ hosts 兜底 / predefined 三段）+ 目标下拉候选。
 * 从 `RuleForm` 外提，供状态与其消费的 JSX（桌面 `RuleDnsEffectFields` / 移动 `RuleFormPanel`）
 * 共用同一份计算。
 */
export function useRuleDnsEffect(
  baseDnsEffect: RuleDnsEffect | null,
  initialPlane: 'route' | 'dns' | undefined,
  dnsServers: DnsServerResource[],
  dnsGroups: DnsServerGroup[],
  servers: ServerConfig[],
  dnsDefaults: UserConfig['dnsDefaults'],
  t: TFunction,
  /** 内置 DHCP 解析器不可用原因的措辞映射（缺省原样；移动端换成手机上的那一句，见 `netenvOption`）。 */
  netenvReasonKeyOf?: (reasonKey: string) => string,
) {
  const [dnsResolver, setDnsResolver] = useState<RuleDnsResolver>(
    () => baseDnsEffect?.resolver ?? 'inherit',
  );
  const [dnsAnswerMode, setDnsAnswerMode] = useState<RuleDnsAnswerMode>(
    () => baseDnsEffect?.answerMode ?? 'real',
  );
  const [dnsAction, setDnsAction] = useState(() =>
    dnsActionChoice(baseDnsEffect?.action) ??
    (baseDnsEffect?.answerMode === 'fakeIp'
      ? 'fakeIp'
      : baseDnsEffect?.resolver === 'proxy'
        ? 'server:builtin-remote'
          : baseDnsEffect?.resolver === 'direct' || initialPlane === 'dns'
          ? 'server:builtin-domestic'
          : 'server:builtin-domestic'),
  );
  const [dnsFallbackAction, setDnsFallbackAction] = useState(() => {
    const fallback = baseDnsEffect?.action?.type === 'hostsFirst'
      ? baseDnsEffect.action.fallback
      : undefined;
    if (fallback?.type === 'server') return `server:${fallback.serverId}`;
    if (fallback?.type === 'group') return `group:${fallback.groupId}`;
    if (fallback?.type === 'fakeIp') return 'fakeIp';
    return `server:${dnsDefaults?.directServerId || 'builtin-domestic'}`;
  });
  const basePredefined = baseDnsEffect?.action?.type === 'predefined'
    ? baseDnsEffect.action
    : undefined;
  const [dnsPredefinedRcode, setDnsPredefinedRcode] = useState(
    () => basePredefined?.rcode ?? 'NOERROR',
  );
  const [dnsPredefinedAnswer, setDnsPredefinedAnswer] = useState(
    () => basePredefined?.answer?.join('\n') ?? '',
  );
  const [dnsPredefinedNs, setDnsPredefinedNs] = useState(
    () => basePredefined?.ns?.join('\n') ?? '',
  );
  const [dnsPredefinedExtra, setDnsPredefinedExtra] = useState(
    () => basePredefined?.extra?.join('\n') ?? '',
  );
  /** 内置解析器「当前网络 DHCP 下发的 DNS」在本机是否可用；拿不到 ⇒ null（不置灰，不猜）。 */
  const netenvStatus = useBuiltinDhcpStatus();
  const dnsActionGroups = useMemo(
    () => buildDnsActionGroups({
      servers: dnsServers,
      groups: dnsGroups,
      nodes: servers,
      t,
      currentValue: dnsAction,
      includeNetenv: true,
      netenvStatus,
      netenvReasonKeyOf,
    }),
    [dnsServers, dnsGroups, servers, t, dnsAction, netenvStatus, netenvReasonKeyOf],
  );
  const dnsFallbackGroups = useMemo(
    () => buildDnsActionGroups({
      servers: dnsServers,
      groups: dnsGroups,
      nodes: servers,
      t,
      currentValue: dnsFallbackAction,
      includeHosts: false,
      responses: ['fakeIp'],
    }),
    [dnsServers, dnsGroups, servers, t, dnsFallbackAction],
  );

  return {
    dnsResolver,
    setDnsResolver,
    dnsAnswerMode,
    setDnsAnswerMode,
    dnsAction,
    setDnsAction,
    dnsFallbackAction,
    setDnsFallbackAction,
    dnsPredefinedRcode,
    setDnsPredefinedRcode,
    dnsPredefinedAnswer,
    setDnsPredefinedAnswer,
    dnsPredefinedNs,
    setDnsPredefinedNs,
    dnsPredefinedExtra,
    setDnsPredefinedExtra,
    dnsActionGroups,
    dnsFallbackGroups,
    netenvStatus,
  };
}

export type UseRuleDnsEffect = ReturnType<typeof useRuleDnsEffect>;

/**
 * 「选了哪个 DNS 动作」对 `answerMode` / `resolver` 的**联动**。
 *
 * 两端必须同一份：少了它，选「FakeIP」而 `answerMode` 还停在 `real`，存下去的规则会把
 * 一个内部矛盾的效果交给引擎（动作说返回 FakeIP、答案模式说给真实 IP）。
 * 逐字取自桌面 `RuleDnsEffectFields` 的 onChange（`RuleDnsEffect.tsx`）。
 */
export function dnsEffectLinkage(choice: string): {
  answerMode: RuleDnsAnswerMode;
  resolver: RuleDnsResolver;
} {
  return {
    answerMode: choice === 'fakeIp' ? 'fakeIp' : 'real',
    resolver: choice === 'server:builtin-remote' ? 'proxy' : 'direct',
  };
}
