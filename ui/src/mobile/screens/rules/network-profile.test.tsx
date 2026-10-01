/**
 * 移动端「网络场景」呈现层的门（spec §6.1 / §6.2；形态与依据见 `NetworkProfilesPage.tsx` / `RuleRow.tsx` 头注）。
 *
 * 守两件**丢了不会有别的门说话**的事：
 *  ① 规则行的场景徽标：五档（正常 / 已删除 / 已停用 / 本机不可用 / 告警）每档都画出来、颜色档位对，
 *     且不生效的四档把桌面挂在 tip 上的那句落成**常驻**说明（§4.12）；本机不可用时原因用手机上的那一句
 *     （`dhcpNeedsPrivilege` 的桌面文案说的是 Linux 系统代理，在手机上是错的）。
 *  ② 二级页：读取态三档、停用的场景不画探测结果、不可用的探测结果用 warn 档、删除 / 编辑 / 启停三颗真的通到容器。
 *
 * 本仓 vitest 是 `environment:'node'`，`renderToStaticMarkup` 不跑 effect —— 这里的组件全是纯呈现，
 * 取数（`useResolvedProbes`）在容器里，不在本门射程内（容器接线由 `screen-parity` 的锚与 `rules-screen.test` 守）。
 */
import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ReactElement } from 'react';
import type { Rule } from '@/contracts/types';
import { PROBE_REASON_KEYS, type RuleProfileBadge } from '@/domain/network-profile';
import { RuleRow } from './RuleRow';
import { NetworkProfilesPage, type NetworkProfileRowModel } from './NetworkProfilesPage';
import { mobileReasonKey } from './network-profile-copy';

/** 插值可见的 `t`：`key` 或 `key{json}`，断言能同时看见键与插进去的值。 */
const t = (key: string, vars?: Record<string, unknown>): string =>
  vars && Object.keys(vars).length > 0 ? `${key}${JSON.stringify(vars)}` : key;

const rule: Rule = {
  id: 'r1',
  type: 'domain',
  values: ['corp.example'],
  action: 'direct',
  enabled: true,
  remarks: '公司内网',
  networkProfileId: 'np-1',
};

const noop = (): void => {};
function row(badge: RuleProfileBadge | null): string {
  return renderToStaticMarkup(
    <RuleRow
      t={t}
      rule={rule}
      index={0}
      enabled
      plane="route"
      isFirst
      isLast
      deleteConfirming={false}
      sheetOpen={false}
      onOpenSheet={noop}
      onToggle={noop}
      onMove={noop}
      onReorder={noop}
      onDuplicate={noop}
      onDelete={noop}
      networkProfileBadge={badge}
    />,
  );
}

describe('① 规则行的场景徽标（角标⑤）', () => {
  it('不挂场景 ⇒ 一个字都不画（正面对照：挂了就画）', () => {
    expect(row(null)).not.toContain('rules.networkProfile.');
    expect(row({ state: 'ok', name: '公司', match: 'unknown' })).toContain('rules.networkProfile.badge{&quot;name&quot;:&quot;公司&quot;}');
  });

  it('五档逐档：徽标文案 + 颜色档（正常中性、其余 warn）+ 不生效四档的常驻说明', () => {
    const cases: Array<[RuleProfileBadge, string, boolean, string | null]> = [
      [{ state: 'ok', name: '公司', source: 'system', match: 'unknown' }, 'rules.networkProfile.badge{"name":"公司"}', false, null],
      [{ state: 'missing' }, 'rules.networkProfile.badgeMissing', true, 'rules.networkProfile.badgeMissingTip'],
      [{ state: 'disabled', name: '公司' }, 'rules.networkProfile.badge{"name":"公司"}', true, 'rules.networkProfile.badgeDisabledTip'],
      [
        { state: 'warning', name: '公司', warningKey: 'rules.networkProfile.ipv6DhcpWarn', match: 'unknown' },
        'rules.networkProfile.badge{"name":"公司"}',
        true,
        'rules.networkProfile.ipv6DhcpWarn · rules.networkProfile.matchUnknown',
      ],
      [
        { state: 'unavailable', name: '公司', reasonKey: PROBE_REASON_KEYS.dhcpMonitorMissing! },
        'rules.networkProfile.badge{"name":"公司"}',
        true,
        'rules.networkProfile.badgeUnavailableTip{"reason":"rules.networkProfile.reasonDhcpMonitorMissing"}',
      ],
    ];
    for (const [badge, pill, warn, note] of cases) {
      const html = row(badge);
      // 命中态圆点（N4）只在「正常 / 告警」两档画，且画在徽标文字之前；其余三档徽标里只有文字。
      const dot =
        badge.state === 'ok' || badge.state === 'warning'
          ? '<span class="dot np-unknown" role="img" aria-label="rules.networkProfile.matchUnknown"></span>'
          : '';
      expect(html, `${badge.state}：徽标没画`).toContain(
        `${warn ? 'mr-pill warn' : 'mr-pill'}">${dot}${pill.replace(/"/g, '&quot;')}<`,
      );
      if (note === null) {
        expect(html, `${badge.state}：正常那档不该有 warn 说明行`).not.toContain('class="mr-note warn"');
      } else {
        expect(html, `${badge.state}：常驻说明缺了（§4.12）`).toContain(
          `class="mr-note warn">${note.replace(/"/g, '&quot;')}</div>`,
        );
      }
    }
  });

  it('🔴 本机不可用且原因是 dhcpNeedsPrivilege ⇒ 写手机上的那一句，不写桌面的 Linux 那句', () => {
    const html = row({ state: 'unavailable', name: '公司', reasonKey: PROBE_REASON_KEYS.dhcpNeedsPrivilege! });
    expect(html).toContain('mobileRules.networkProfile.reasonDhcpNoPermission');
    expect(html).not.toContain('rules.networkProfile.reasonDhcpNeedsPrivilege');
    // 映射只换这一句：表外的键原样透传（正反两侧）。
    expect(mobileReasonKey(PROBE_REASON_KEYS.dhcpNeedsPrivilege!)).toBe('mobileRules.networkProfile.reasonDhcpNoPermission');
    expect(mobileReasonKey(PROBE_REASON_KEYS.profileInvalid!)).toBe(PROBE_REASON_KEYS.profileInvalid);
  });
});

function page(over: Partial<Parameters<typeof NetworkProfilesPage>[0]> = {}): string {
  const el: ReactElement = (
    <NetworkProfilesPage
      t={t}
      loadState="ready"
      rows={[]}
      onRetry={noop}
      onToggle={noop}
      onEdit={noop}
      onDelete={noop}
      errorOf={() => undefined}
      {...over}
    />
  );
  return renderToStaticMarkup(el);
}

const ROW_OK: NetworkProfileRowModel = {
  id: 'np-1',
  name: '公司',
  enabled: true,
  summary: '地址段 ×1 (10.0.0.0/8)',
  refs: '2 条流量规则、1 条 DNS 规则在用',
  probe: { text: 'PROBE-OK', unavailable: false },
};

describe('② 二级页', () => {
  it('读取态三档：加载中 / 读失败（带重试）/ 就绪（说明 + 列表）', () => {
    expect(page({ loadState: 'loading' })).toContain('common.loading');
    const failed = page({ loadState: 'error' });
    expect(failed).toContain('common.configLoadFail');
    expect(failed).toContain('>common.retry</button>');
    expect(failed, '读失败时不许画列表（空列表会被读作「没有场景」）').not.toContain('mobileHelp.networkProfiles');
    const ready = page({ rows: [ROW_OK] });
    expect(ready).toContain('mobileHelp.networkProfiles');
    expect(ready).toContain('公司');
    expect(ready).toContain(ROW_OK.refs);
  });

  it('空列表说「还没有网络场景」（不是空白）', () => {
    expect(page({ rows: [] })).toContain('rules.networkProfile.empty');
  });

  it('探测结果：可用 ⇒ 中性说明；不可用 ⇒ warn；停用的场景只标「已停用」、不画探测结果', () => {
    const html = page({
      rows: [
        ROW_OK,
        { ...ROW_OK, id: 'np-2', name: 'B', probe: { text: 'PROBE-BAD', unavailable: true }, warning: 'WARN-IPV6' },
        { ...ROW_OK, id: 'np-3', name: 'C', enabled: false, probe: undefined },
      ],
    });
    expect(html).toContain('<div class="mr-note">PROBE-OK</div>');
    expect(html).toContain('<div class="mr-note warn">PROBE-BAD</div>');
    expect(html).toContain('<div class="mr-note warn">WARN-IPV6</div>');
    const rowC = html.slice(html.indexOf('>C<'));
    expect(rowC).toContain('rules.networkProfile.disabledBadge');
    expect(rowC).not.toContain('PROBE-');
  });

  it('三颗动作真的通到容器（编辑 / 删除 / 启停各带自己那一行的 id）', () => {
    const seen: string[] = [];
    const el = NetworkProfilesPage({
      t,
      loadState: 'ready',
      rows: [ROW_OK],
      onRetry: noop,
      onToggle: (id, next) => seen.push(`toggle:${id}:${String(next)}`),
      onEdit: (id) => seen.push(`edit:${id}`),
      onDelete: (id) => seen.push(`delete:${id}`),
      errorOf: () => undefined,
    });
    /* 纯函数组件：从返回的元素树里找到三颗的 onClick / onChange 直接调（node 环境点不了按钮）。 */
    const walk = (node: unknown): void => {
      if (node === null || typeof node !== 'object') return;
      if (Array.isArray(node)) {
        node.forEach(walk);
        return;
      }
      const props = (node as { props?: Record<string, unknown> }).props;
      if (!props) return;
      if (typeof props.onClick === 'function' && typeof props.children === 'string') {
        if (props.children === 'common.edit' || props.children === 'common.delete') (props.onClick as () => void)();
      }
      if (typeof props.onChange === 'function' && props.title === 'rules.networkProfile.enabled') (props.onChange as () => void)();
      walk(props.children);
    };
    walk(el);
    expect(seen.sort()).toEqual(['delete:np-1', 'edit:np-1', 'toggle:np-1:false']);
  });

  it('写失败落在那一行上（`np:<id>`）', () => {
    const html = page({ rows: [ROW_OK], errorOf: (key) => (key === 'np:np-1' ? 'SAVE-FAILED' : undefined) });
    expect(html).toContain('SAVE-FAILED');
  });
});

describe('③ 命中态（N4）：三态圆点 + 文案', () => {
  const STATES = [
    ['matched', 'dot ok', 'IN'],
    ['unmatched', 'dot idle', 'OUT'],
    ['unknown', 'dot np-unknown', 'UNKNOWN'],
  ] as const;

  it('二级页：给了命中态 ⇒ 圆点（形状/颜色类）+ 常驻文案；没给（停用 / 不可用）⇒ 一个圆点都不画', () => {
    for (const [state, cls, text] of STATES) {
      const html = page({ rows: [{ ...ROW_OK, match: { state, text } }] });
      expect(html, state).toContain(
        `<div class="mr-note mr-np-match"><span class="${cls}" role="img" aria-label="${text}"></span>${text}</div>`,
      );
    }
    expect(page({ rows: [ROW_OK] })).not.toContain('class="dot');
  });

  it('规则行：正常 / 告警两档的徽标带圆点且按命中态换类；其余三档不画圆点', () => {
    for (const [state, cls] of STATES) {
      expect(row({ state: 'ok', name: '公司', match: state }), state).toContain(`<span class="${cls}" role="img"`);
      expect(
        row({ state: 'warning', name: '公司', warningKey: 'W', match: state }),
        state,
      ).toContain(`<span class="${cls}" role="img"`);
    }
    for (const badge of [
      { state: 'missing' },
      { state: 'disabled', name: '公司' },
      { state: 'unavailable', name: '公司', reasonKey: PROBE_REASON_KEYS.profileInvalid! },
    ] as RuleProfileBadge[])
      expect(row(badge), badge.state).not.toContain('class="dot');
  });
});
