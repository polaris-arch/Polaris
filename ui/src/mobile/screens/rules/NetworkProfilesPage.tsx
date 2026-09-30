import { MobileInfo } from '../../MobileInfo';
/**
 * 二级页：网络场景（spec §6.1 / §6.2 D13；桌面 `components/screens/rules/NetworkProfilePanel.tsx` 的
 * `NetworkProfilesDialog` + `ProfileList` 那一格）。
 *
 * # 为什么是二级页，不是弹层
 *
 * 桌面是「规则页页头入口 → 列表弹窗 → 编辑弹窗叠在上面」。手机上两层居中弹窗叠在一起，拇指够不到、
 * 键盘一弹就只剩一条边（`forms/FormSheet.tsx` 头注那两条）。与 DNS 服务器 / 分组同一个形态：
 * 列表是规则屏的二级页（屏头溢出进入，两个平面**同一个入口**），编辑是表单宿主的一层
 * （`forms/NetworkProfileFormPanel`）。
 *
 * # 纯呈现
 *
 * 行的每一格都由容器（`RulesScreen.tsx`）算好：判据摘要 / 引用计数走 `domain/network-profile`，
 * 探测结果走后端 IPC（`network-profile-probes#useResolvedProbes`），本文件一条判据都不持有。
 *
 * # §4.12：桌面挂在 tip 上的那几句，这里全是常驻行
 *
 * 「本机将使用 / 不可用（原因）」、告警、停用标记、命中态（圆点 + 文案，N4）都是可见文字；删除的后果（「N 条规则将停止生效」）
 * 写进确认面板的正文，不是悬浮提示。
 */

import type { ReactElement } from 'react';
import type { ProfileMatch } from '@/domain/network-profile';
import { MatchDot } from '@/components/screens/rules/MatchDot';
import { EmptyState, Flow, InlineError, SwitchRow } from './Primitives';

export interface NetworkProfileRowModel {
  id: string;
  name: string;
  enabled: boolean;
  /** 判据摘要（「地址段 ×2 · 搜索域 ×1 (10.0.0.0/8, corp.example)」），容器按桌面同一口径拼好。 */
  summary: string;
  /** 引用计数那一句（`rules.networkProfile.refs`）。 */
  refs: string;
  /**
   * 「本机将使用：…」一行。停用的场景**不给**（后端对停用恒报 `profileInvalid`，
   * 再画一行不可用只是把「已停用」用更吓人的红字重复一遍 —— 同桌面 `profileRowStatus`）。
   */
  probe?: { text: string; unavailable: boolean };
  /** 后端告警（`available` 仍为 true，如 `dhcpIpv6Only`）。 */
  warning?: string;
  /**
   * 命中态（N4，内核 canary 的结果）：圆点 + 常驻文案。只在探测源可用时给（同桌面 `profileRowStatus`
   * 的 `match`：停用 / 不可用的场景谈不上「是否处在该网络」）。
   */
  match?: { state: ProfileMatch; text: string };
}

export function NetworkProfilesPage({
  t,
  loadState,
  rows,
  onRetry,
  onToggle,
  onEdit,
  onDelete,
  errorOf,
}: {
  t: (key: string, vars?: Record<string, unknown>) => string;
  /** 配置读取态（桌面 `ConfigShell` 那一格）：加载中 / 读失败（带重试）/ 就绪。 */
  loadState: 'loading' | 'error' | 'ready';
  rows: readonly NetworkProfileRowModel[];
  onRetry: () => void;
  onToggle: (id: string, next: boolean) => void;
  onEdit: (id: string) => void;
  onDelete: (id: string) => void;
  errorOf: (key: string) => string | undefined;
}): ReactElement {
  if (loadState === 'loading') return <EmptyState text={t('common.loading')} />;
  if (loadState === 'error') {
    return (
      <div>
        <EmptyState text={t('common.configLoadFail')} />
        <button type="button" className="mr-btn" onClick={onRetry}>
          {t('common.retry')}
        </button>
      </div>
    );
  }
  return (
    <>
      <div className="mr-note"><MobileInfo title={t('rules.networkProfile.title')} summary={t('mobileHelp.networkProfiles')} details={t('mobileRules.networkProfile.intro')} /></div>
      {rows.length === 0 ? (
        <EmptyState text={t('rules.networkProfile.empty')} />
      ) : (
        <Flow>
          {rows.map((row) => (
            <div key={row.id} className="mr-row">
              <div className="mr-row-main">
                <div className="mr-row-title">{row.name}</div>
                <div className="mr-row-sub">{row.summary}</div>
                <div className="mr-row-pills">
                  {!row.enabled && (
                    <span className="mr-pill warn">{t('rules.networkProfile.disabledBadge')}</span>
                  )}
                  <span className="mr-pill">{row.refs}</span>
                </div>
                {row.probe !== undefined && (
                  <div className={row.probe.unavailable ? 'mr-note warn' : 'mr-note'}>{row.probe.text}</div>
                )}
                {row.warning !== undefined && <div className="mr-note warn">{row.warning}</div>}
                {row.match !== undefined && (
                  <div className="mr-note mr-np-match">
                    <MatchDot match={row.match.state} label={row.match.text} />
                    {row.match.text}
                  </div>
                )}
                <SwitchRow
                  id={`mr-np-${row.id}`}
                  title={t('rules.networkProfile.enabled')}
                  description={t('rules.networkProfile.enabledHint')}
                  checked={row.enabled}
                  onChange={() => onToggle(row.id, !row.enabled)}
                />
                <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                  <button type="button" className="mr-btn" onClick={() => onEdit(row.id)}>
                    {t('common.edit')}
                  </button>
                  {/* 删除走表单宿主上叠一层确认（`forms/ConfirmPanel`），正文说清「N 条规则将停止生效」——
                      桌面把这句放在原地二次点击的武装态里，触屏上那一行在拇指底下被手指自己挡住。 */}
                  <button type="button" className="mr-btn danger" onClick={() => onDelete(row.id)}>
                    {t('common.delete')}
                  </button>
                </div>
                <InlineError text={errorOf(`np:${row.id}`)} />
              </div>
            </div>
          ))}
        </Flow>
      )}
    </>
  );
}
