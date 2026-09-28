/**
 * TUN 页的组网网段结算报告（移动端）。桌面对照：
 * `components/screens/settings/EndpointForceRouteBlock.tsx`。
 *
 * # 为什么另起一份呈现，而不是 import 桌面那两块
 *
 * 桌面报告渲染的是 `./Primitives` 的 `SetBlock` → `dialogs/Csel` 那整条桌面层叠链，
 * 契约 A1 不许进移动入口（判据 `ui/src/styles/css-oracle.test.ts` ⑤）。
 * 结算类型来自共享 `@/contracts/endpoint-force-route-report`；Tailscale 判定复用
 * `@/domain/endpoint-routes` 的 `isAccountBasedProtocol`。
 *
 * # 为什么是纯 props 组件（拉取留在 `TunPage`）
 *
 * 本仓 vitest 是 node 环境无 jsdom，`useEffect` 一行都不跑 ⇒ 报告由纯 props 真渲染测试。
 * Android/iOS 的外来 VPN 路由探测恒为 Unsupported，移动端没有该卡也不请求该报告；
 * 桌面仍保留其四态报告。自己的组网网段结算是真实能力，继续显示。
 */

import type { CSSProperties, ReactElement, ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import type { ServerConfig } from '@/contracts/types';
import type { EndpointForceRouteReport } from '@/contracts/endpoint-force-route-report';
import { isAccountBasedProtocol } from '@/domain/endpoint-routes';
import { RefreshIcon } from '../screens/rules/Primitives';
import { SettingsGroup, SettingsNote } from './SettingsChrome';

/** 逐条清单的排版。移动端没有桌面的 `.cidr-eff-list`，同族样式就地给。 */
const LIST: CSSProperties = {
  margin: '8px 0 0',
  padding: '0 0 0 18px',
  fontSize: '0.75rem',
  lineHeight: 1.55,
  color: 'hsl(var(--fg-dim))',
};

const MONO: CSSProperties = { fontFamily: 'var(--mono)' };

/**
 * 告警行。桌面用 `.plat-warn`（那个类默认 `display:none`，只在 Linux 由 CSS 放出来），
 * 移动端走本屏既有的 `SettingsNote tone="warn"` —— 同一条「缺席不是错误，不用错误色」的口径。
 */
function Warn({ id, children }: { id: string; children: ReactNode }): ReactElement {
  return (
    <SettingsNote id={id} tone="warn">
      {children}
    </SettingsNote>
  );
}

/**
 * 「Polaris 自己的组网节点之间谁把谁的网段吃掉了」结算（只读）。
 *
 * 报告本身移动端**早就在拉**了（节点屏的「被覆盖网段」角标与规则屏的重叠角标消费同一条命令的
 * 同一份 `absorbed`）；这一层补的是「谁被吃干净了、证据强度如何」—— 而那恰恰是静默失效
 * （节点活着、engaged，流量一条都不到）唯一的可见出口。
 */
export function MobileEndpointForceRouteBlock({
  report,
  loading,
  error,
  onRefresh,
  servers,
}: {
  /** `null` = 还没有成功读取过。 */
  report: EndpointForceRouteReport | null;
  loading: boolean;
  error: boolean;
  onRefresh: () => void;
  /** 把报告里的 serverId 换成用户看得懂的节点名，并判断败方是不是 Tailscale。 */
  servers: readonly ServerConfig[];
}): ReactElement {
  const { t } = useTranslation();
  const nameOf = (id: string): string => servers.find((s) => s.id === id)?.name ?? id;
  const isTs = (id: string): boolean =>
    isAccountBasedProtocol(servers.find((s) => s.id === id)?.protocol);
  const coverageText = (coverage: EndpointForceRouteReport['servers'][number]['coverage']): string => {
    if (coverage === 'covered') return t('mobileSettings.forceRouteCovered');
    if (coverage === 'absorbedEmpty') return t('mobileSettings.forceRouteAbsorbedEmpty');
    return t('mobileSettings.forceRouteNothingToRoute');
  };

  let body: ReactNode;
  if (report === null) {
    body = (
      <div role={error ? 'alert' : 'status'}>
        <SettingsNote id="force-route-unavailable">
          {t(loading ? 'settings.tun.forceRouteLoading' : 'settings.tun.forceRouteUnavailable')}
        </SettingsNote>
      </div>
    );
  } else if (report.servers.length === 0) {
    body = <SettingsNote id="force-route-empty">{t('mobileSettings.forceRouteEmpty')}</SettingsNote>;
  } else {
    const zero = report.servers.filter((s) => report.zeroCoverageServerIds.includes(s.serverId));
    // 证据强度：败方里有「没有观测地址的 Tailscale 节点」⇒ 这次重合可能只是两份默认常量
    // （两个尚未连上的 TS 节点段集**必然**重合），不足以让用户据此去删节点。
    const weakEvidence = report.servers.some(
      (s) => s.absorbed.length > 0 && !s.hasObservation && isTs(s.serverId),
    );
    body = (
      <>
        {zero.length > 0 && (
          <>
            <Warn id="force-route-zero-coverage">{t('mobileSettings.forceRouteZeroCoverage')}</Warn>
            <ul style={LIST}>
              {zero.map((s) => (
                <li key={s.serverId}>
                  <b>{nameOf(s.serverId)}</b>
                  {s.absorbed.map((a) => (
                    <span key={a.cidr}>
                      {' '}
                      <span style={MONO}>{a.cidr}</span>{' '}
                      {t('mobileSettings.forceRouteAbsorbedBy', { node: nameOf(a.byServerId) })}
                    </span>
                  ))}
                </li>
              ))}
            </ul>
          </>
        )}
        {zero.length === 0 && report.absorbedCount > 0 && (
          <SettingsNote id="force-route-absorbed-only">
            {t('mobileSettings.forceRouteAbsorbedOnly', { count: report.absorbedCount })}
          </SettingsNote>
        )}
        {zero.length === 0 && report.absorbedCount === 0 && (
          <SettingsNote id="force-route-none">
            {t('mobileSettings.forceRouteNone', { count: report.servers.length })}
          </SettingsNote>
        )}
        {weakEvidence && (
          <SettingsNote id="force-route-no-observation">
            {t('mobileSettings.forceRouteNoObservation')}
          </SettingsNote>
        )}
        <details className="ms-force-details">
          <summary>{t('mobileSettings.forceRouteDetails', { count: report.servers.length })}</summary>
          <p>{t('mobileSettings.forceRouteCalculationHint')}</p>
          <ol className="ms-force-list">
            {report.servers.map((server) => (
              <li key={server.serverId}>
                <b>{nameOf(server.serverId)}</b>
                <span>{coverageText(server.coverage)}</span>
                {isTs(server.serverId) && (
                  <span>{server.hasObservation
                    ? t('mobileSettings.forceRouteObserved')
                    : t('mobileSettings.forceRouteUnobservedTs')}</span>
                )}
                {server.leg === 'preferredBy' ? (
                  <span>{t('mobileSettings.forceRoutePreferredBy')}</span>
                ) : server.leg === 'inline' && server.emitted.length > 0 ? (
                  <span>{t('mobileSettings.forceRouteInline')}: <span style={MONO}>{server.emitted.join(', ')}</span></span>
                ) : server.leg === 'externalRuleSet' && server.externalRuleSetCidrs.length > 0 ? (
                  <span>{t('mobileSettings.forceRouteExternal')}: <span style={MONO}>{server.externalRuleSetCidrs.join(', ')}</span></span>
                ) : null}
                {server.absorbed.map((entry) => (
                  <span key={`${entry.cidr}:${entry.byServerId}`}>
                    {t('mobileSettings.forceRouteAbsorbedCidr')}: <span style={MONO}>{entry.cidr}</span>{' '}
                    {t('mobileSettings.forceRouteAbsorbedBy', { node: nameOf(entry.byServerId) })}
                  </span>
                ))}
              </li>
            ))}
          </ol>
        </details>
      </>
    );
  }

  return (
    <SettingsGroup header={<div className="ms-force-head">
      <span>{t('settings.tun.forceRouteBlock')}</span>
      <button type="button" className="ms-force-refresh" onClick={onRefresh} disabled={loading}
        aria-label={`${t('common.refresh')} · ${t('settings.tun.forceRouteBlock')}`}
        title={`${t('common.refresh')} · ${t('settings.tun.forceRouteBlock')}`}>
        <RefreshIcon />
      </button>
    </div>}>
      <SettingsNote id="force-route-hint" title={t('settings.tun.forceRouteBlock')} summary={t('mobileHelp.forceRoute')}>{t('mobileSettings.forceRouteCalculationHint')}</SettingsNote>
      {loading && report !== null && <p className="ms-force-feedback" role="status">{t('settings.tun.forceRouteLoading')}</p>}
      {error && report !== null && <p className="ms-force-feedback error" role="alert">{t('settings.tun.forceRouteUnavailable')}</p>}
      {body}
    </SettingsGroup>
  );
}
