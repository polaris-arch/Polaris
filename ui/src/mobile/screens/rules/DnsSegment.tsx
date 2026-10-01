/**
 * 「规则 → DNS」分段（IA §2.2「Content order — DNS segment」）+ 它的两个二级页。
 *
 * 内容顺序：优先级链 → 系统保护规则（折叠）→ 自定义规则数 → 有序规则列表 → 未命中默认动作。
 * 对应桌面 `RulesScreen.tsx`（plane='dns'）的 :640-745。
 *
 * # 为什么服务器 / 分组是二级页，不是第二层分段条
 *
 * 桌面 DNS 工作区自己有三个视图（规则 / 服务器 / 分组，`RulesScreen.tsx:542`）。照搬会得到
 * 「四路分段条」套「三路分段条」两层：compact 下吃掉两行，且两层的活动态在视觉上读作
 * 「一个控件两个高亮」。收敛文档 §1.5 已裁定：**规则列表是 DNS 分段的主体，服务器与分组改为
 * 屏头溢出里的二级页**。决定性的是频次差 —— 规则天天读写，服务器/分组配一次用很久。
 *
 * # 本批做不到的两处，各自在场且带可见理由（IA §3.3 第 2 条）
 *
 * 1. **系统保护规则展开后的那块设置面板**：桌面在这里内嵌 `SettingsDns`（`DnsPolicyWorkspace`
 *    的 `view="system"`）—— 那是设置屏的面（F4 线）。三条系统规则的文字说明照常渲染，
 *    只是不内嵌那块面板。
 * 2. **未命中默认动作的修改**：桌面用 `Csel`，移动端对应物是 `select-sheet`；但它的选项集要
 *    `buildDnsActionGroups` 那整套（服务器 / 分组 / Hosts / 响应动作四个分组），
 *    与「加/改 DNS 服务器」的表单是同一条腿，一起归后续线。**当前值照常显示** ——
 *    隐藏它等于让用户以为默认动作不存在。
 */

import type { ReactElement } from 'react';
import {
  Cards,
  EmptyState,
  Flow,
  Fold,
  InlineError,
  OrderedList,
  PriorityFlow,
  SwitchRow,
} from './Primitives';
import { RuleRow } from './RuleRow';
import type { RuleRowBinding } from './TrafficSegment';

/** 桌面 `RulesScreen.tsx:62-66` 那三条系统保护规则的文案 key，逐字照搬。 */
export const DNS_SYSTEM_RULE_KEYS = [
  'rules.dnsWorkspace.systemRuleNode',
  'rules.dnsWorkspace.systemRuleBootstrap',
  'rules.dnsWorkspace.systemRuleLocal',
] as const;

/**
 * 二级页的一条 DNS 服务器 / 分组。展示用值全部由容器解出来（本文件是纯呈现）。
 *
 * 🔴 **引用数这一栏 2026-09-13（批 10）补上，旧理由随之作废。**
 * 旧理由是「引用图是 `DnsPolicyWorkspace.tsx` 的文件内私有函数，照抄一份会让同一张图有两个实现」。
 * 本批把它搬进了 `@/components/dialogs/dns-resource-logic`（纯 `.ts`，两端 import **同一份**），
 * 「照抄」那条路因此不用走 —— 而删除入口既然接上了，删除前的护栏就必须在场：
 * 引用数正是「删了会不会让别的东西失效」的唯一依据。
 */
export interface DnsResourceRow {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  /** 内置服务器：不可停用、不可删（桌面 `isProtectedDnsServer`）。 */
  builtin?: boolean;
  /** 被几处引用（规则 / 分组 / 其它服务器的 bootstrap / 默认动作）。删除护栏的依据。 */
  references: number;
}

export interface DnsSegmentProps {
  t: (key: string, vars?: Record<string, unknown>) => string;
  rows: readonly RuleRowBinding[];
  /** 未命中默认动作的当前值（已解成可读文案）。 */
  defaultActionText: string;
  /** `hostsFirst` 时的「未命中后的服务器」；其余动作为 undefined。 */
  defaultFallbackText?: string;
  errorOf: (key: string) => string | undefined;
  /** 桌面在这里内嵌设置面板，移动端不嵌 —— 这句是可见的缺席理由。 */
  systemPaneAbsentReason: string;
}

export function DnsSegment({
  t,
  rows,
  defaultActionText,
  defaultFallbackText,
  systemPaneAbsentReason,
  errorOf,
}: DnsSegmentProps): ReactElement {
  return (
    <>
      {/*
        优先级链与系统保护 fold 是**卡片块**，medium/expanded 下按 IA §2.2「Breakpoints」走双列。
        计数头留在外面（它是下面那条有序列表的头）；「未命中默认动作」那张卡也留在外面 ——
        它在列表**之后**，单独包一层 `Cards` 只会得到一个 `:only-child`，跨满两列后与现在逐像素相同，
        是纯噪声。
      */}
      <Cards>
        <PriorityFlow
          label={t('rules.dnsWorkspace.priority')}
          steps={[
            { id: 'system', label: t('rules.dnsWorkspace.systemStage'), active: true },
            { id: 'custom', label: t('rules.chainCustom') },
            { id: 'default', label: t('rules.chainDefault') },
          ]}
        />

        <Fold
          title={
            <>
              <span>{t('rules.dnsWorkspace.systemRules')}</span>
              <span className="mr-pill">{t('settings.dns.builtinTag')}</span>
            </>
          }
          count={DNS_SYSTEM_RULE_KEYS.length}
          /* 桌面把这句放在 `Fold` 的 `tip`（= `data-tip`）里。触屏没有 hover ⇒ 常驻（§4.12）。 */
          note={t('rules.dnsWorkspace.systemRulesAlwaysFirst')}
        >
          {DNS_SYSTEM_RULE_KEYS.map((key) => (
            <div key={key}>{t(key)}</div>
          ))}
          <div className="mr-note">{systemPaneAbsentReason}</div>
        </Fold>
      </Cards>

      <div>
        <div className="mr-card-h">
          <span>{t('rules.chainCustom')}</span>
          <span className="mr-cnt">{rows.length}</span>
        </div>
        <div className="mr-note">{t('rules.priorityTip')}</div>
        <InlineError text={errorOf('order:dns')} />
      </div>

      {/* 有序列表：DNS 规则同样是「首个命中生效」，同样恒单列。 */}
      {rows.length === 0 ? (
        <EmptyState text={t('rules.empty')} />
      ) : (
        <OrderedList label={t('rules.chainCustom')}>
          {rows.map((row, i) => (
            <RuleRow
              key={row.rule.id}
              {...row}
              t={t}
              plane="dns"
              index={i}
              error={errorOf(`rule:${row.rule.id}`)}
              isFirst={i === 0}
              isLast={i === rows.length - 1}
            />
          ))}
        </OrderedList>
      )}

      {/* 未命中默认动作：桌面 `DnsPolicyWorkspace.tsx:240-267`，位置同样在规则列表之后。 */}
      <section className="mr-card">
        <div className="mr-card-h">
          <span>{t('settings.dns.defaultUnmatched')}</span>
          <span className="mr-pill">{t('settings.dns.builtinTag')}</span>
        </div>
        <div className="mr-note">{t('rules.dnsWorkspace.defaultLast')}</div>
        <div className="mr-row-title" style={{ marginTop: 8 }}>
          {defaultActionText}
        </div>
        {defaultFallbackText != null && (
          <div className="mr-note">
            {t('rules.dnsHostsFallback')} · {defaultFallbackText}
          </div>
        )}
      </section>
    </>
  );
}

/**
 * 二级页：DNS 服务器 / 服务器组。
 *
 * 两者结构相同（名称 + 描述 + 引用数 + 启停），故共用一个呈现 —— 它们在桌面就是同一段 JSX
 * 的两个分支（`DnsPolicyWorkspace.tsx:272-370`）。
 *
 * **这是无序集合**，因此照常走 `Flow`（medium/expanded 双列）。规则列表那条「恒单列」的例外
 * 只针对有序列表，别把例外扩大成整屏。
 */
export function DnsResourcePage({
  t,
  rows,
  emptyText,
  onToggle,
  onEdit,
  onDelete,
  deleteConfirmingId,
  errorOf,
}: {
  t: (key: string, vars?: Record<string, unknown>) => string;
  rows: readonly DnsResourceRow[];
  emptyText: string;
  onToggle: (id: string, next: boolean) => void;
  onEdit: (id: string) => void;
  onDelete: (id: string) => void;
  /** 处于「再点一次即删」武装态的那一行（状态由屏单点持有，同规则行 / 资源行）。 */
  deleteConfirmingId: string | null;
  errorOf: (key: string) => string | undefined;
}): ReactElement {
  if (rows.length === 0) return <EmptyState text={emptyText} />;
  return (
    <Flow>
      {rows.map((row) => {
        const confirming = deleteConfirmingId === row.id;
        return (
          <div key={row.id} className="mr-row">
            <div className="mr-row-main">
              <div className="mr-row-title">{row.name}</div>
              <div className="mr-row-sub">{row.description}</div>
              {/* 引用数：0 引用时说「未被引用」而不是印一个孤零零的 0
                  （与资源行的 `RefBadge` 同口径）。它是删除键旁边那条护栏的可见部分。 */}
              <div className="mr-row-pills">
                <span className="mr-pill">
                  {row.references > 0
                    ? t('rules.dnsWorkspace.referenceCount', { count: row.references })
                    : t('resources.unreferenced')}
                </span>
              </div>
              {/* 桌面把「内置服务器必须保持启用」挂在开关的 `data-tip` 上
                  （`DnsPolicyWorkspace.tsx:291`）。这里是常驻的第二行（§4.12）。 */}
              <SwitchRow
                id={`mr-dnsres-${row.id}`}
                title={t('settings.dns.serverEnabled')}
                description={row.builtin === true ? t('settings.dns.builtinRequired') : undefined}
                checked={row.enabled}
                onChange={() => onToggle(row.id, !row.enabled)}
                disabled={row.builtin === true}
              />
              <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                <button type="button" className="mr-btn" onClick={() => onEdit(row.id)}>
                  {t('common.edit')}
                </button>
                {/* 内置三个服务器**不画**删除键（同桌面 `DnsPolicyWorkspace.tsx:302`）：
                    它们是引导链与两条默认解析腿的载体，删掉解析整条断。
                    这不是「缺席带理由」那一档 —— 那一档针对的是移动端没接的能力，
                    而这一颗在两端都不存在。 */}
                {row.builtin !== true && (
                  <button
                    type="button"
                    /* `.confirming` 是与 `lib/confirm-twice.ts` 的**跨文件契约**，不是样式：
                       漏了它，第二次点击的 pointerdown 先解除武装、click 又重新武装，
                       文案在「删除 ↔ 再点一次」之间无限翻转，永远删不掉。 */
                    className={confirming ? 'mr-btn danger confirming' : 'mr-btn danger'}
                    onClick={() => onDelete(row.id)}
                  >
                    {confirming ? t('common.confirmAgain') : t('common.delete')}
                  </button>
                )}
              </div>
              <InlineError text={errorOf(`dnsres:${row.id}`)} />
            </div>
          </div>
        );
      })}
    </Flow>
  );
}
