/**
 * 「本机其它隧道与 Polaris 争不争同一网段」报告块（只读）。
 *
 * # 为什么这一块必须按 `status` 分支，而不是读 `conflicts.length`
 *
 * 2026-09-08 的现场：用户跑着自建 Tailscale 控制面（tailnet 前缀 `32.0.0.0/24`），与 Polaris 的
 * TUN 冲突 —— 而应用内没有任何观测面能看见「本机还有别的隧道」。后端把探测与判定接起来之后，
 * 剩下的全部风险都集中在渲染这一步：**macOS / Windows 根本没有探测实现**（只有 Linux 有），
 * 而报障那台正是 macOS。把「没探成」画成一句自信的「无冲突」，比什么都不显示更坏 ——
 * 用户会据此排除掉真正的病因。
 *
 * 故四态各有各的话，且**只有 `probed` 且 `conflicts` 为空**那一支才允许出现「无冲突」的措辞：
 *  - `notProbed`   → 本次没探（核没起过 / 不是 TUN 模式 / 探测腿没跑完）；
 *  - `unsupported` → 本平台没有探测实现，判定**未进行**；
 *  - `probeFailed` → 探测命令失败，判定**未进行**（带后端诊断串）；
 *  - `probed`      → 看过了。空 = 真的没冲突；非空 = 逐条列出「谁的哪一段、撞哪一类」。
 *
 * 判别联合由 `contracts/tunnel-conflict-report.ts` 在类型层强制：不先判 `status` 就取
 * `conflicts`，tsc 直接报错。
 *
 * # `probed` 为什么有两句话（而不是一句带计数的）
 *
 * `foreignTunnels` 是**后端已经收过噪声的展示面**：link-local 与组播在 Rust 侧
 * （`runtime::proxy::tunnel_conflict`）摘掉，摘掉多少条走 `suppressedRoutes`。本组件一条过滤
 * 都不写 —— 写在这里会逼出第二份 CIDR 包含算术（TS 侧只有 `cidrsOverlap`），而两份判据迟早会漂。
 *
 * 2026-09-12 真机实测：一台 Tailscale 断开的 mac 上，36 条外来隧道路由**全部**是这两族
 * ⇒ 收完 `foreignTunnels` 为空。那时若仍用「另有 0 条隧道路由」这句，这一支在界面上就与
 * 「压根没探」同形了 —— 这正是整块要防的那件事。故零条时换一句话说，并把被收掉的条数
 * 一并摆出来：它是「这次真的读了一张路由表」剩下的唯一可见证据。
 *
 * # 为什么抽成独立组件
 *
 * 拉取（`useEffect` + IPC）留在 `SettingsTun`，本组件**纯按 props 渲染**：本仓 vitest 是
 * node 环境无 jsdom，`useEffect` 一行都不跑 ⇒ 四态在 `SettingsTun` 里永远只观测得到首帧。
 * 抽出来之后四态可以逐个喂进去、用 `renderToStaticMarkup` 真渲染并断言界面文本
 * （见 `TunnelConflictBlock.status-branches.test.tsx`）。
 */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';

import type {
  TunnelConflictKind,
  TunnelConflictReport,
} from '@/contracts/tunnel-conflict-report';
import { SetBlock } from './Primitives';

/**
 * 后端给的是 Node 约定名（`platform_tag`：darwin / win32 / linux / other）。这里只把它换成
 * 用户认得的产品名；认不出的值**原样显示**，不猜、也不吞（新平台加进来时宁可显示 `other`）。
 */
const PLATFORM_LABEL: Record<string, string> = {
  darwin: 'macOS',
  win32: 'Windows',
  linux: 'Linux',
};

/**
 * 冲突类别 → 文案键。写成 `if` 链而不是 `Record<Kind, key>` 表：G6a（i18n 覆盖门）只认
 * `t('字面量')` 调用点，表驱动的键在那一侧看不见 —— 三条译文会变成「声明了却没消费」的死键。
 */
function kindLabel(kind: TunnelConflictKind, t: TFunction): string {
  if (kind === 'fakeIpOverlap') return t('settings.tun.tunnelConflictKindFakeIp');
  if (kind === 'meshOverlap') return t('settings.tun.tunnelConflictKindMesh');
  return t('settings.tun.tunnelConflictKindTun');
}

export interface TunnelConflictBlockProps {
  /** `null` = 还没拉到 / 拉取失败。与「探过了没冲突」同样不许混为一谈。 */
  report: TunnelConflictReport | null;
}

export function TunnelConflictBlock({ report }: TunnelConflictBlockProps) {
  const { t } = useTranslation();
  return (
    <SetBlock header={t('settings.tun.tunnelConflictBlock')}>
      <div className="card-sub">{t('settings.tun.tunnelConflictHint')}</div>
      {report === null ? (
        <div className="card-sub">{t('settings.tun.tunnelConflictUnavailable')}</div>
      ) : report.status === 'notProbed' ? (
        <div className="card-sub">{t('settings.tun.tunnelConflictNotProbed')}</div>
      ) : report.status === 'unsupported' ? (
        // 未探测的平台恒走这一支。这句话是本组件存在的全部理由，措辞里不许出现任何
        // 「没有冲突」的等价说法 —— 它说的是「判定没跑」，不是「跑完了是干净的」。
        <Warn>
          {t('settings.tun.tunnelConflictUnsupported', {
            platform: PLATFORM_LABEL[report.platform] ?? report.platform,
          })}
        </Warn>
      ) : report.status === 'probeFailed' ? (
        <Warn>
          <span>{t('settings.tun.tunnelConflictFailed')}</span>
          <span className="mono">{report.error}</span>
        </Warn>
      ) : report.conflicts.length === 0 ? (
        // 唯一一句断言，分两种说法 —— 因为**最常见的那一种是展示面为零**：任何一台有 utun 的
        // mac 上，外来隧道宣告的全是 link-local 与组播（2026-09-12 真机实测 36 条全是），
        // 后端已把它们收掉。那时若沿用下面那句「另有 0 条隧道路由」，这一支在界面上就与
        // 「压根没探」同形了。故零条时改说「没有任何一条宣告业务网段」，并把被收掉的条数
        // 一并给出 —— 这两件事合起来才让「探测真的跑过」保持可见。
        <div className="card-sub">
          {report.foreignTunnels.length === 0
            ? t('settings.tun.tunnelConflictNoBusinessRanges', { count: report.suppressedRoutes })
            : t('settings.tun.tunnelConflictNone', { count: report.foreignTunnels.length })}
        </div>
      ) : (
        <>
          <Warn>{t('settings.tun.tunnelConflictFound', { count: report.conflicts.length })}</Warn>
          <ul className="cidr-eff-list">
            {report.conflicts.map((conflict, i) => (
              <li key={`${conflict.interface}-${conflict.prefix}-${conflict.kind}-${i}`}>
                <span className="mono">
                  {conflict.interface} {conflict.prefix}
                </span>{' '}
                {kindLabel(conflict.kind, t)}
              </li>
            ))}
          </ul>
        </>
      )}
    </SetBlock>
  );
}

/**
 * `.plat-warn` 的配色是本仓既有的告警行，但它默认 `display:none`（只在 Linux 由 CSS 放出来，
 * 见 components.css 的 `:root[data-os="lin"] .plat-warn`）⇒ 用它就必须显式覆盖 display，
 * 否则整条告警在 mac/win 上**静默消失**，而那正是最需要它的两个平台。
 * 与同页「生效排除面」的诊断行同一写法。
 */
function Warn({ children }: { children: ReactNode }) {
  return (
    <div className="plat-warn" style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      {children}
    </div>
  );
}

export default TunnelConflictBlock;
