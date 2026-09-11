/**
 * 「Polaris 自己的组网节点之间，谁把谁的网段吃掉了」结算块（只读）。
 *
 * # 这一块要说的那句话
 *
 * 同一条 `ip_cidr` 只能指向一个 outbound，内核 first-match ⇒ 两个组网节点声明同一段时，
 * **只有更早发射的那个生效**。后果里最坏的一种不是「少了一段」，而是一个节点的段被**全部**
 * 吸收：节点活着、engaged、用户以为它在工作，而流量一条都不会到它那儿。
 * 这就是 `zeroCoverageServerIds` —— 静默失效，不是信息，故它单独成一条告警并**点名到节点**。
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
import { SetBlock } from './Primitives';

export interface EndpointForceRouteBlockProps {
  /** `null` = 还没拉到 / 拉取失败。 */
  report: EndpointForceRouteReport | null;
  /** 用于把报告里的 serverId 换成用户看得懂的节点名，并判断败方是不是 Tailscale。 */
  servers: readonly ServerConfig[];
}

export function EndpointForceRouteBlock({ report, servers }: EndpointForceRouteBlockProps) {
  const { t } = useTranslation();
  const nameOf = (id: string) => servers.find((s) => s.id === id)?.name ?? id;
  const isTs = (id: string) =>
    isAccountBasedProtocol(servers.find((s) => s.id === id)?.protocol);

  if (report === null) {
    return (
      <Block>
        <div className="card-sub">{t('settings.tun.forceRouteUnavailable')}</div>
      </Block>
    );
  }
  if (report.servers.length === 0) {
    return (
      <Block>
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
    <Block>
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
    </Block>
  );
}

/** 块壳与说明行在四条腿上逐字相同，抽出来免得「改了一处、另外三处还是旧话」。 */
function Block({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  return (
    <SetBlock header={t('settings.tun.forceRouteBlock')}>
      <div className="card-sub">{t('settings.tun.forceRouteHint')}</div>
      {children}
    </SetBlock>
  );
}

export default EndpointForceRouteBlock;
