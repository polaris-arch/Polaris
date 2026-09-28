/**
 * 「Polaris 自己的组网节点之间，谁把谁的网段吃掉了」结算块（只读）。
 *
 * # 这一块要说的那句话
 *
 * inline `ip_cidr` 走 first-match，两个组网节点声明同一段时，只有结算顺序更早的节点取得它。
 * `zeroCoverageServerIds` 指本次计算中一个节点的所有具体段均被吸收；必须单独告警并点名节点。
 * 报告由已保存配置与可用观测重算，不能据此宣称此刻节点正在运行、流量已承载或一定可达。
 *
 * # 为什么这里必须消费 `hasObservation`
 *
 * Tailscale 的段集恒含两条硬编码默认常量（`TAILNET_CGNAT` / `TAILNET_ULA_V6`）。没有运行期观测
 * 地址时，两个 TS 节点的段**必然**完全重合 —— 那不是「两个账号撞车」的证据，只是两份一样的猜测
 * （创建期硬闸当年就是栽在这个前提上：自建 headscale 实测把地址发成 `32.0.0.28`，与官方段不相交）。
 * 故「败方是没有观测的 Tailscale 节点」时，本块补一句证据强度说明，不让用户据此去删节点。
 *
 * # 为什么落在这一页
 *
 * 与同页的「本机其它隧道」报告成对：那条问「**别人的**隧道与我撞不撞」，本条问「**我自己的**
 * 几个组网节点之间撞不撞」。两条都是「后端跑真判据、把结果读回来」的只读报告，形态与
 * 「本平台生效的排除网段」同源。节点卡上的「网段被覆盖」角标读的是**同一条命令的同一份
 * `absorbed`**（`screens/nodes/nodes-logic.shadowedCidrNamed`）：角标答「这个节点有没有段被吃」，
 * 本块答「谁被吃干净了、证据强度如何」。同源不同粒度，不可能一个说被覆盖、另一个说没有。
 *
 * 拉取留在 `SettingsTun`，本组件纯按 props 渲染（理由同 `TunnelConflictBlock`：node 环境无
 * jsdom，`useEffect` 不跑，状态在页面里永远只观测得到首帧）。
 */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import type { ServerConfig } from '@/contracts/types';
import type { EndpointForceRouteReport } from '@/contracts/endpoint-force-route-report';
import { isAccountBasedProtocol } from '@/domain/endpoint-routes';
import { revealOnToggle } from '@/components/reveal';
import { SetBlock } from './Primitives';

export interface EndpointForceRouteBlockProps {
  /** `null` = 尚未取得当前已保存配置的报告。 */
  report: EndpointForceRouteReport | null;
  loading?: boolean;
  error?: boolean;
  onRefresh?: () => void;
  /** 已保存配置中的节点；暂存回显不可混入这份后端报告。 */
  servers: readonly ServerConfig[];
}

const PROTOCOL_LABELS: Record<string, string> = {
  wireguard: 'WireGuard',
  tailscale: 'Tailscale',
  openconnect: 'OpenConnect',
  'openvpn-client': 'OpenVPN',
  'masque-client': 'MASQUE',
};

export function EndpointForceRouteBlock({
  report,
  loading = false,
  error = false,
  onRefresh,
  servers,
}: EndpointForceRouteBlockProps) {
  const { t } = useTranslation();
  const serverOf = (id: string) => servers.find((s) => s.id === id);
  const nameOf = (id: string) => serverOf(id)?.name ?? id;
  const isTs = (id: string) =>
    isAccountBasedProtocol(serverOf(id)?.protocol);

  if (loading || error || report === null) {
    return (
      <Block loading={loading} onRefresh={onRefresh}>
        <div className="card-sub" role={error ? 'alert' : undefined}>
          {loading
            ? t('settings.tun.forceRouteLoading')
            : t('settings.tun.forceRouteUnavailable')}
        </div>
      </Block>
    );
  }
  if (report.servers.length === 0) {
    return (
      <Block loading={loading} onRefresh={onRefresh}>
        <div className="card-sub">{t('settings.tun.forceRouteEmpty')}</div>
      </Block>
    );
  }

  const zero = report.servers.filter((s) => report.zeroCoverageServerIds.includes(s.serverId));
  // 证据强度：败方里有「没有观测地址的 Tailscale 节点」⇒ 这次重合可能只是两份默认常量。
  const weakEvidence = report.servers.some(
    (s) => s.absorbed.length > 0 && !s.hasObservation && isTs(s.serverId),
  );

  return (
    <Block loading={loading} onRefresh={onRefresh}>
      {zero.length > 0 && (
        <>
          <div className="plat-warn" style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            <span>{t('settings.tun.forceRouteZeroCoverage')}</span>
          </div>
          <ul className="cidr-eff-list">
            {zero.map((s) => (
              <li key={s.serverId}>
                <b>{nameOf(s.serverId)}</b>
                {s.absorbed.map((a) => (
                  <span key={a.cidr}>
                    {' '}
                    <span className="mono">{a.cidr}</span>{' '}
                    {t('settings.tun.forceRouteAbsorbedBy', { node: nameOf(a.byServerId) })}
                  </span>
                ))}
              </li>
            ))}
          </ul>
        </>
      )}
      {zero.length === 0 && report.absorbedCount > 0 && (
        <div className="card-sub">
          {t('settings.tun.forceRouteAbsorbedOnly', { count: report.absorbedCount })}
        </div>
      )}
      {zero.length === 0 && report.absorbedCount === 0 && (
        <div className="card-sub">
          {t('settings.tun.forceRouteNone', { count: report.servers.length })}
        </div>
      )}
      {weakEvidence && (
        <div className="card-sub">{t('settings.tun.forceRouteNoObservation')}</div>
      )}
      <details className="tun-details mesh-route-details" onToggle={revealOnToggle}>
        <summary>{t('settings.tun.forceRouteDetails')}</summary>
        <ol className="mesh-route-nodes">
          {report.servers.map((entry) => {
            const server = serverOf(entry.serverId);
            const declared = server?.protocol === 'wireguard'
              ? server.wireguardSettings?.allowedIPs ?? []
              : [];
            const allocated = entry.leg === 'externalRuleSet'
              ? entry.externalRuleSetCidrs
              : entry.emitted;
            return (
              <li key={entry.serverId}>
                <div className="mesh-route-node-title">
                  <b>{nameOf(entry.serverId)}</b>
                  <span>{PROTOCOL_LABELS[server?.protocol ?? ''] ?? server?.protocol ?? entry.serverId}</span>
                  {isTs(entry.serverId) && (
                    <span className="mesh-route-observation">
                      {entry.hasObservation
                        ? t('settings.tun.forceRouteObserved')
                        : t('settings.tun.forceRouteEstimated')}
                    </span>
                  )}
                </div>
                {entry.leg === 'preferredBy' ? (
                  <>
                    <div className="card-sub">{t('settings.tun.forceRouteDeclared')}</div>
                    {declared.length > 0 ? (
                      <ul className="cidr-eff-list">
                        {declared.map((cidr, index) => <li className="mono" key={index}>{cidr}</li>)}
                      </ul>
                    ) : (
                      <div className="card-sub">{t('settings.tun.forceRouteNoDeclared')}</div>
                    )}
                    <div className="card-sub">{t('settings.tun.forceRoutePreferredByNote')}</div>
                  </>
                ) : (
                  <>
                    <div className="card-sub">
                      {entry.leg === 'externalRuleSet'
                        ? t('settings.tun.forceRouteRuleSetRanges')
                        : t('settings.tun.forceRouteAllocated')}
                    </div>
                    {allocated.length > 0 ? (
                      <ul className="cidr-eff-list">
                        {allocated.map((cidr) => <li className="mono" key={cidr}>{cidr}</li>)}
                      </ul>
                    ) : (
                      <div className="card-sub">{t('settings.tun.forceRouteNoRanges')}</div>
                    )}
                  </>
                )}
                {entry.absorbed.length > 0 && (
                  <>
                    <div className="card-sub">{t('settings.tun.forceRouteAbsorbedRanges')}</div>
                    <ul className="cidr-eff-list">
                      {entry.absorbed.map(({ cidr, byServerId }) => (
                        <li key={`${cidr}-${byServerId}`}>
                          <span className="mono">{cidr}</span>{' → '}{nameOf(byServerId)}
                        </li>
                      ))}
                    </ul>
                  </>
                )}
              </li>
            );
          })}
        </ol>
      </details>
    </Block>
  );
}

/** 块壳与说明行在四条腿上逐字相同，抽出来免得「改了一处、另外三处还是旧话」。 */
function Block({
  children,
  loading,
  onRefresh,
}: { children: ReactNode; loading: boolean; onRefresh?: () => void }) {
  const { t } = useTranslation();
  return (
    <SetBlock header={
      <div className="mesh-route-heading">
        <span>{t('settings.tun.forceRouteBlock')}</span>
        {onRefresh && (
          <button type="button" className="btn ghost sm" onClick={onRefresh} disabled={loading}>
            {loading ? t('settings.tun.forceRouteLoading') : t('common.refresh')}
          </button>
        )}
      </div>
    }>
      <div className="card-sub">{t('settings.tun.forceRouteHint')}</div>
      {children}
    </SetBlock>
  );
}

export default EndpointForceRouteBlock;
