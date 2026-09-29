/**
 * 「规则 → 流量」分段（IA §2.2「Content order — 流量 segment」）。
 *
 * 内容顺序逐条对齐桌面 `RulesScreen.tsx`（plane='route'）：
 *  1. `mode-warn`（:573-586）——「下面这些规则现在不决定任何事」，必须先于它们；
 *  2. 地区分流卡（:589-595，`GeoCard.tsx`）—— 一条凌驾于单条规则之上的常设裁决；
 *  3. 两条 `rules-note`（:598-629）—— 限定列表；
 *  4. 规则数与优先级头（:663-689）；
 *  5. 有序规则列表（:691-743）。
 *
 * **纯呈现**：所有数据与回调由容器 `RulesScreen.tsx` 注入。这样门可以拿夹具直接渲染它，
 * 不必先把整个 store / IPC 立起来 —— 「测方法体」与「测接线」是两件事，这里被测的是前者。
 */

import type { ReactElement, ReactNode } from 'react';
import type { RegionRoutingConfig } from '@/contracts/types';
import {
  Card,
  Cards,
  Choice,
  EmptyState,
  InlineError,
  NoticeBanner,
  OrderedList,
  PriorityFlow,
  Switch,
  SwitchRow,
} from './Primitives';
import { RuleRow, type RuleRowProps } from './RuleRow';

export type RuleRowBinding = Omit<RuleRowProps, 't' | 'index' | 'isFirst' | 'isLast' | 'plane'>;

export interface TrafficSegmentProps {
  t: (key: string, vars?: Record<string, unknown>) => string;
  /** 非智能分流模式（global / direct）：流量效果整体不生效。 */
  modeInactive: boolean;
  regionRouting: RegionRoutingConfig;
  onRegionChange: (next: RegionRoutingConfig) => void;
  isSmartMode: boolean;
  onBackToSmart: () => void;
  /** 已按优先级排好序的规则；序号即数组下标 + 1。 */
  rows: readonly RuleRowBinding[];
  meshInfo?: ReactNode;
  /** 写失败的行内回显查询（为什么是行内而不是全局 toast，见 `Primitives.InlineError` 头注）。 */
  errorOf: (key: string) => string | undefined;
}

export function TrafficSegment({
  t,
  modeInactive,
  regionRouting,
  onRegionChange,
  isSmartMode,
  onBackToSmart,
  rows,
  meshInfo,
  errorOf,
}: TrafficSegmentProps): ReactElement {
  const { enabled, region, reverse } = regionRouting;

  return (
    <>
      {/*
        ①–③ 是**卡片块**，medium/expanded 下按 IA §2.2「Breakpoints」走双列
        （`Cards`）。④ 的计数头留在外面：它是⑤那条有序列表的头，跟着列表走才对；
        把它塞进双列会让「自定义规则 N」落到右栏、列表在下面横跨整幅。
      */}
      <Cards>
        {/* ① 模式警告：global / direct 下**仅流量路由效果**未生效；DNS 效果仍生效。 */}
        {modeInactive && (
          <NoticeBanner
            tone="warn"
            text={t('rules.modeWarn')}
            action={{ label: t('rules.backToSmart'), onClick: onBackToSmart }}
          />
        )}
        <InlineError text={errorOf('proxyMode')} />

        {/* ② 地区分流卡。桌面把它摆在 mode-warn 之后、两条 note 之前，这里同序。 */}
        <Card
          className={enabled ? 'mr-geo-card has-options' : 'mr-geo-card'}
          title={
            <>
              <span>{t('rules.regionRouting')}</span>
              <span style={{ marginLeft: 'auto' }}>
                <Switch
                  id="mr-geo-enabled"
                  checked={enabled}
                  onChange={() => onRegionChange({ ...regionRouting, enabled: !enabled })}
                  label={t('rules.regionRouting')}
                />
              </span>
            </>
          }
          /* 说明必须跟随 reverse 反转：回国模式下语义相反。恒显正向文案是误导，桌面为此改过一次。 */
          note={reverse ? t('rules.regionRoutingSubReverse') : t('rules.regionRoutingSub')}
        >
          {!isSmartMode && <div className="mr-note warn">{t('rules.regionRoutingSmartOnly')}</div>}
          <InlineError text={errorOf('regionRouting')} />

          {/*
            优先级链讲的是**全局**链，与地区分流开关无关（关掉它不改变链本身），
            故不随 `enabled` 收起 —— 只有下面地区分流自己的参数才收。
          */}
          <div className="mr-geo-priority" style={{ marginTop: 12 }}>
            <PriorityFlow
              label={t('rules.routingPriority')}
              steps={[
                { id: 'custom', label: t('rules.chainCustom'), active: true },
                { id: 'app', label: t('rules.chainApp') },
                { id: 'mesh', label: t('rules.chainMesh') },
                { id: 'lan', label: t('rules.chainLan') },
                { id: 'smart', label: t('rules.chainSmart') },
                { id: 'default', label: t('rules.chainDefault') },
              ]}
            />
          </div>

          {/*
            **关态收起**，不是关态禁用：总开关关掉后仍可点的地区按钮是**假的可操作性** ——
            点了不生效，却照样把值写进 config，真机表现成「切了地区没反应」。桌面为此改过一次。
          */}
          {enabled && (
            <div className="mr-geo-options" style={{ marginTop: 12 }}>
              <div className="mr-note">{t('rules.yourRegion')}</div>
              <Choice
                label={t('rules.yourRegion')}
                active={region}
                onSelect={(next) => onRegionChange({ ...regionRouting, region: next })}
                items={[
                  { id: 'cn' as const, label: t('rules.region.cn') },
                  { id: 'ir' as const, label: t('rules.region.ir') },
                  { id: 'ru' as const, label: t('rules.region.ru') },
                ]}
              />
              {/* 桌面把这句挂在 `.info-i` 的 `data-tip` 上（GeoCard.tsx:147）。触屏没有 hover
                  ⇒ 它在这里是常驻的第二行（§4.12）。 */}
              <SwitchRow
                id="mr-geo-reverse"
                title={t('rules.backHome')}
                description={t('rules.backHomeTip')}
                checked={reverse === true}
                onChange={() => onRegionChange({ ...regionRouting, reverse: !reverse })}
              />
            </div>
          )}
        </Card>

        {/* ③ 限定说明。桌面 `.rules-note` 有两条，移动端只留分流模式这一条。
            另一条（`rules.manualNote`「手动接管：把代理指向本地端口…」）**整条不移植**：手动接管在
            Android 上不存在（`crates/config-engine/src/builder/inbounds.rs` 在 android 下不发 `mixed-in`，
            那个「本地端口」根本没有人在听），而 `config.proxyModeType` 仍可能被桌面备份导入成
            `manual` ⇒ 照读它会在移动端弹出一条指向不存在端口的操作指引。能力缺席按整条移除处置，
            与首页那颗接管方式芯片同一条裁定。 */}
        {modeInactive && <NoticeBanner tone="warn" text={t('rules.modeNote')} />}
      </Cards>

      {/* ④ 规则数 + 优先级说明。桌面把说明藏在 `.info-i` 的 `data-tip` 里（:680），这里常驻。 */}
      <div>
        <div className="mr-card-h">
          <span>{t('rules.priority')}</span>
          <span className="mr-cnt">{rows.length}</span>
        </div>
        <div className="mr-note">{t('rules.priorityTip')}</div>
        {meshInfo && <div className="mr-note">{meshInfo}</div>}
        <InlineError text={errorOf('order:route')} />
      </div>

      {/* ⑤ 有序列表。**恒单列** —— 行号就是优先级，双列会让阅读序不再等于求值序。 */}
      {rows.length === 0 ? (
        <EmptyState text={t('rules.empty')} />
      ) : (
        <OrderedList label={t('rules.priority')}>
          {rows.map((row, i) => (
            <RuleRow
              key={row.rule.id}
              {...row}
              t={t}
              plane="route"
              index={i}
              error={errorOf(`rule:${row.rule.id}`)}
              isFirst={i === 0}
              isLast={i === rows.length - 1}
            />
          ))}
        </OrderedList>
      )}
    </>
  );
}
