/**
 * `meshForceRoutedServers` —— 「本轮真会发射 force-route 的组网节点」这条 engaged 过滤线的单测。
 *
 * # 为什么这组断言要单独存在
 *
 * 它原本寄生在 `mesh-shadowed-cidrs.test.ts` 里（以「被覆盖角标与发射端同口径」的形式表达）。
 * 那个角标已改为消费后端结算报告、`meshShadowedCidrs` 随之删除，但 `meshForceRoutedServers`
 * **另有生产消费点**（`screens/rules/RulesScreen` 的「自定义规则与组网段重叠」提醒），
 * 连同宿主测试一起删掉就会让这条过滤线失去全部覆盖 —— 故原样保留在这里，只把断言的表达面
 * 从「谁被判成被覆盖」换成「谁留在发射集里」。
 *
 * 守的不变量：`alwaysRouteSubnets=false`（「仅出网」）的节点**只在 engaged 时**才发射 force-route，
 * engaged 的两条来源是「被选为主出口」与「被某条 enabled 规则显式指向」。漏掉这层过滤，
 * 上层提醒会对一个本轮根本不发段的节点虚报重叠。
 */
import { describe, it, expect } from 'vitest';
import type { ServerConfig } from '../contracts/types';
import { collectRuleTargetedServerIds, meshForceRoutedServers } from './endpoint-routes';

const wg = (id: string, allowedIPs: string[], extra: Record<string, unknown> = {}): ServerConfig =>
  ({
    id,
    name: id,
    protocol: 'wireguard',
    address: '203.0.113.7',
    port: 51820,
    wireguardSettings: { allowedIPs, ...extra },
  }) as ServerConfig;

describe('meshForceRoutedServers —— 与发射端块 0c 同口径的 engaged 过滤', () => {
  const engaged = wg('a', ['10.0.0.0/24']);
  /** 「仅出网」：alwaysRouteSubnets=false → 未被选中/未被规则指向时本轮**不发射** force-route。 */
  const offMesh = wg('b', ['10.0.0.0/24'], { alwaysRouteSubnets: false });

  it('未 engaged 的「仅出网」节点被过滤掉', () => {
    expect(meshForceRoutedServers([engaged, offMesh], null, new Set()).map((s) => s.id)).toEqual([
      'a',
    ]);
  });

  it('被选为主出口后即 engaged → 回到发射集', () => {
    expect(meshForceRoutedServers([engaged, offMesh], 'b', new Set()).map((s) => s.id)).toEqual([
      'a',
      'b',
    ]);
  });

  it('被启用规则显式指向亦算 engaged（口径经 collectRuleTargetedServerIds）', () => {
    const targeted = collectRuleTargetedServerIds([
      { enabled: true, action: 'proxy', targetServerId: 'b' },
      { enabled: false, action: 'proxy', targetServerId: 'zz' },
    ]);
    // 反向对照同在一条断言里：被 disabled 规则指向的 'zz' 不该进 engaged 集。
    expect(targeted).toEqual(new Set(['b']));
    expect(meshForceRoutedServers([engaged, offMesh], null, targeted).map((s) => s.id)).toEqual([
      'a',
      'b',
    ]);
  });

  it('alwaysRouteSubnets 默认开：不带该字段的节点恒在发射集里', () => {
    // 正向对照：上面三条全靠 offMesh 那一个节点的缺席/在场说话，这条钉住「默认是开」——
    // 若默认被改成关，上面第一条仍绿（两个节点都被滤掉时 `['a']` 会变成 `[]` ⇒ 其实会红），
    // 但语义来源在这里说清楚。
    expect(meshForceRoutedServers([engaged], null, new Set()).map((s) => s.id)).toEqual(['a']);
  });
});
