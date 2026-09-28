/**
 * TUN 页的两块**只读报告**（移动端）。桌面对照：
 * `components/screens/settings/TunnelConflictBlock.tsx` 与 `EndpointForceRouteBlock.tsx`。
 *
 * # 为什么另起一份呈现，而不是 import 桌面那两块
 *
 * 桌面两块渲染的是 `./Primitives` 的 `SetBlock` → `dialogs/Csel` 那整条桌面层叠链，
 * 契约 A1 不许进移动入口（判据 `ui/src/styles/css-oracle.test.ts` ⑤）。
 * **但判据与类型一个字都不重写**：四态判别联合来自 `@/contracts/tunnel-conflict-report`
 * （不先判 `status` 就取 `conflicts` 在 tsc 层就报错），Tailscale 判定来自
 * `@/domain/endpoint-routes` 的 `isAccountBasedProtocol`，文案五语种与桌面同一批键。
 *
 * # 为什么是纯 props 组件（拉取留在 `TunPage`）
 *
 * 与桌面同一条理由：本仓 vitest 是 node 环境无 jsdom，`useEffect` 一行都不跑 ⇒ 四态在页面里
 * 永远只观测得到首帧。抽出来之后四态可以逐个喂进去、用 `renderToStaticMarkup` 真渲染并断言
 * 界面文本（见 `TunReports.test.tsx`）。
 *
 * # 🔴 Android 上第一块**恒走 `unsupported` 那一支**，而那正是它存在的理由
 *
 * `crates/system-integration/src/route_probe.rs` 的探测实现只有 Linux / macOS / Windows 三家，
 * 其余平台落 `TunnelProbeOutcome::Unsupported`。故这一块在手机上显示的是「判定未进行」，
 * **不是**「无冲突」—— 后者会让用户据此排除掉真正的病因（2026-09-08 那台 macOS 的原样形态）。
 * 「只有 `probed` 且 `conflicts` 为空那一支才允许出现无冲突的措辞」这条纪律逐字照搬。
 */

import type { CSSProperties, ReactElement, ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';

import type { ServerConfig } from '@/contracts/types';
import type { EndpointForceRouteReport } from '@/contracts/endpoint-force-route-report';
import type {
  TunnelConflictKind,
  TunnelConflictReport,
} from '@/contracts/tunnel-conflict-report';
import { isAccountBasedProtocol } from '@/domain/endpoint-routes';
import { SettingsGroup, SettingsNote } from './SettingsChrome';

/**
 * 后端给的是 Node 约定名（`platform_tag`：darwin / win32 / linux / other）。这里只把它换成
 * 用户认得的产品名；认不出的值**原样显示**，不猜、也不吞 —— Android 今天正是「认不出」的那一档，
 * 编一个名字出来就是在一句「判定未进行」里掺一条没核实过的信息。
 */
const PLATFORM_LABEL: Record<string, string> = {
  darwin: 'macOS',
  win32: 'Windows',
  linux: 'Linux',
};

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
 * 冲突类别 → 文案键。写成 `if` 链而不是 `Record<Kind, key>` 表：i18n 覆盖门只认
 * `t('字面量')` 调用点，表驱动的键在那一侧看不见 —— 四条译文会变成「声明了却没消费」的死键。
 * （与桌面那份逐字同形，包括默认路由那一支「只陈述两个声索人同时在场、不预言谁赢」的取向。）
 */
function kindLabel(kind: TunnelConflictKind, t: TFunction): string {
  if (kind === 'fakeIpOverlap') return t('settings.tun.tunnelConflictKindFakeIp');
  if (kind === 'meshOverlap') return t('settings.tun.tunnelConflictKindMesh');
  if (kind === 'defaultRouteContended') return t('settings.tun.tunnelConflictKindDefaultRoute');
  return t('settings.tun.tunnelConflictKindTun');
}

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

/** 「本机其它隧道与 Polaris 争不争同一网段」（只读，四态各说各的话）。 */
export function MobileTunnelConflictBlock({
  report,
}: {
  /** `null` = 还没拉到 / 拉取失败。与「探过了没冲突」同样不许混为一谈。 */
  report: TunnelConflictReport | null;
}): ReactElement {
  const { t } = useTranslation();
  return (
    <SettingsGroup header={t('settings.tun.tunnelConflictBlock')}>
      <SettingsNote id="tunnel-conflict-hint" title={t('settings.tun.tunnelConflictBlock')} summary={t('mobileHelp.tunnelConflict')}>{t('settings.tun.tunnelConflictHint')}</SettingsNote>
      {report === null ? (
        <SettingsNote id="tunnel-conflict-unavailable">
          {t('settings.tun.tunnelConflictUnavailable')}
        </SettingsNote>
      ) : report.status === 'notProbed' ? (
        <SettingsNote id="tunnel-conflict-not-probed">
          {t('settings.tun.tunnelConflictNotProbed')}
        </SettingsNote>
      ) : report.status === 'unsupported' ? (
        // 🔴 Android 恒走这一支。措辞里不许出现任何「没有冲突」的等价说法 ——
        // 它说的是「判定没跑」，不是「跑完了是干净的」。
        <Warn id="tunnel-conflict-unsupported">
          {t('settings.tun.tunnelConflictUnsupported', {
            platform: PLATFORM_LABEL[report.platform] ?? report.platform,
          })}
        </Warn>
      ) : report.status === 'probeFailed' ? (
        <Warn id="tunnel-conflict-failed">
          {t('settings.tun.tunnelConflictFailed')} <span style={MONO}>{report.error}</span>
        </Warn>
      ) : report.conflicts.length === 0 ? (
        // 唯一一句断言，分两种说法 —— 因为最常见的那一种是展示面为零（外来隧道宣告的全是
        // link-local 与组播，后端已收掉）。零条时若沿用「另有 0 条隧道路由」，这一支在界面上
        // 就与「压根没探」同形了，而这四支判别联合存在的全部理由就是把这两件事分开。
        <SettingsNote id="tunnel-conflict-none">
          {report.foreignTunnels.length === 0
            ? t('settings.tun.tunnelConflictNoBusinessRanges', { count: report.suppressedRoutes })
            : t('settings.tun.tunnelConflictNone', { count: report.foreignTunnels.length })}
        </SettingsNote>
      ) : (
        <>
          <Warn id="tunnel-conflict-found">
            {t('settings.tun.tunnelConflictFound', { count: report.conflicts.length })}
          </Warn>
          <ul style={LIST}>
            {report.conflicts.map((conflict, i) => (
              <li key={`${conflict.interface}-${conflict.prefix}-${conflict.kind}-${i}`}>
                <span style={MONO}>
                  {conflict.interface} {conflict.prefix}
                </span>{' '}
                {kindLabel(conflict.kind, t)}
              </li>
            ))}
          </ul>
        </>
      )}
    </SettingsGroup>
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
  servers,
}: {
  /** `null` = 还没拉到 / 拉取失败。 */
  report: EndpointForceRouteReport | null;
  /** 把报告里的 serverId 换成用户看得懂的节点名，并判断败方是不是 Tailscale。 */
  servers: readonly ServerConfig[];
}): ReactElement {
  const { t } = useTranslation();
  const nameOf = (id: string): string => servers.find((s) => s.id === id)?.name ?? id;
  const isTs = (id: string): boolean =>
    isAccountBasedProtocol(servers.find((s) => s.id === id)?.protocol);

  let body: ReactNode;
  if (report === null) {
    body = (
      <SettingsNote id="force-route-unavailable">
        {t('settings.tun.forceRouteUnavailable')}
      </SettingsNote>
    );
  } else if (report.servers.length === 0) {
    body = <SettingsNote id="force-route-empty">{t('settings.tun.forceRouteEmpty')}</SettingsNote>;
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
            <Warn id="force-route-zero-coverage">{t('settings.tun.forceRouteZeroCoverage')}</Warn>
            <ul style={LIST}>
              {zero.map((s) => (
                <li key={s.serverId}>
                  <b>{nameOf(s.serverId)}</b>
                  {s.absorbed.map((a) => (
                    <span key={a.cidr}>
                      {' '}
                      <span style={MONO}>{a.cidr}</span>{' '}
                      {t('settings.tun.forceRouteAbsorbedBy', { node: nameOf(a.byServerId) })}
                    </span>
                  ))}
                </li>
              ))}
            </ul>
          </>
        )}
        {zero.length === 0 && report.absorbedCount > 0 && (
          <SettingsNote id="force-route-absorbed-only">
            {t('settings.tun.forceRouteAbsorbedOnly', { count: report.absorbedCount })}
          </SettingsNote>
        )}
        {zero.length === 0 && report.absorbedCount === 0 && (
          <SettingsNote id="force-route-none">
            {t('settings.tun.forceRouteNone', { count: report.servers.length })}
          </SettingsNote>
        )}
        {weakEvidence && (
          <SettingsNote id="force-route-no-observation">
            {t('settings.tun.forceRouteNoObservation')}
          </SettingsNote>
        )}
      </>
    );
  }

  return (
    <SettingsGroup header={t('settings.tun.forceRouteBlock')}>
      <SettingsNote id="force-route-hint" title={t('settings.tun.forceRouteBlock')} summary={t('mobileHelp.forceRoute')}>{t('settings.tun.forceRouteHint')}</SettingsNote>
      {body}
    </SettingsGroup>
  );
}
