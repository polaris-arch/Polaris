/**
 * 组网重叠角标谓词的判定断言。契约 §Rules「角标 — 组网 force-route 重叠(meshOverlapRuleIds)」。
 *
 * 重点不是「函数跑通」，而是钉住三条会静默失效的判据：**前缀相交（非字面量相等）**、
 * **跨族不相交**、**禁用规则不算**。
 */
import { describe, it, expect } from 'vitest';
import type { Rule, ServerConfig } from '@/contracts/types';
import type {
  EndpointForceRouteReport,
  ServerForceRoute,
} from '@/contracts/endpoint-force-route-report';
import {
  cidrsOverlap,
  cidrOverlapsAny,
  forceRoutedCidrsFromReport,
  meshOverlapRuleIds,
} from './mesh-rule-overlap';
import { endpointForcedRouteCidrs, TAILNET_CGNAT, TAILNET_ULA_V6 } from './endpoint-routes';

function rule(id: string, values: string[], enabled = true, type: Rule['type'] = 'ipCidr'): Rule {
  return { id, type, values, action: 'proxy', enabled } as Rule;
}

describe('cidrsOverlap', () => {
  it('包含关系相交（这正是字面量比对答不出的那一类）', () => {
    // 变异守卫：把实现退化成 `a.trim() === b.trim()` → 本例转红。
    expect(cidrsOverlap('10.0.0.0/8', '10.8.0.0/24')).toBe(true);
    expect(cidrsOverlap('10.8.0.0/24', '10.0.0.0/8')).toBe(true);
  });

  it('不相交的同族段 → false', () => {
    expect(cidrsOverlap('10.8.0.0/24', '10.9.0.0/24')).toBe(false);
    expect(cidrsOverlap('192.168.1.0/24', '10.0.0.0/8')).toBe(false);
  });

  it('裸 IP 视作 /32 与 /128', () => {
    expect(cidrsOverlap('10.8.0.5', '10.8.0.0/24')).toBe(true);
    expect(cidrsOverlap('10.9.0.5', '10.8.0.0/24')).toBe(false);
    expect(cidrsOverlap('fd7a:115c:a1e0::1', 'fd7a:115c:a1e0::/48')).toBe(true);
  });

  it('全网段覆盖一切（force-route 全隧道节点）', () => {
    expect(cidrsOverlap('0.0.0.0/0', '203.0.113.7/32')).toBe(true);
    expect(cidrsOverlap('::/0', 'fd7a::1/128')).toBe(true);
  });

  it('v6 前缀相交 / 不相交', () => {
    expect(cidrsOverlap('fd7a:115c:a1e0::/48', 'fd7a:115c:a1e0:ab12::/64')).toBe(true);
    // fd00::/8 与 fd7a:… 的前 8 bit 同为 0xfd → **相交**（Tailscale 的 fd7a 段本就落在 ULA fd00::/8 内）。
    expect(cidrsOverlap('fd7a:115c:a1e0::/48', 'fd00::/8')).toBe(true);
    // 真不相交要看第 8 bit：fc(…1100) vs fd(…1101)。
    expect(cidrsOverlap('fd7a:115c:a1e0::/48', 'fc00::/8')).toBe(false);
    expect(cidrsOverlap('fd7a:115c:a1e0::/48', '2001:db8::/32')).toBe(false);
  });

  it('v6 前缀落在 16-bit 组中间时按位比对（非整组）', () => {
    // /36 = 前两组整取 + 第三组只取高 4 bit。守住 `groupMask` 的部分掩码分支：
    // 若把它写成「整组取或整组丢」，1000 与 1fff 会判不相交、1000 与 2000 会判相交 → 两条同时转红。
    expect(cidrsOverlap('2001:db8:1000::/36', '2001:db8:1fff::/36')).toBe(true);
    expect(cidrsOverlap('2001:db8:1000::/36', '2001:db8:2000::/36')).toBe(false);
  });

  it('跨族恒不相交（v4 与 v6 不得互判命中）', () => {
    // 变异守卫：若把两族比对写成「任一 parse 成功即比」会误判 → 转红。
    expect(cidrsOverlap('10.0.0.0/8', 'fd7a::/16')).toBe(false);
    expect(cidrsOverlap('::/0', '0.0.0.0/0')).toBe(false);
  });

  it('非法输入恒 false（不抛、不误命中）', () => {
    expect(cidrsOverlap('999.1.1.1/8', '10.0.0.0/8')).toBe(false);
    expect(cidrsOverlap('', '10.0.0.0/8')).toBe(false);
    expect(cidrsOverlap('10.0.0.0/33', '10.0.0.0/8')).toBe(false);
    expect(cidrsOverlap('not-an-ip', '10.0.0.0/8')).toBe(false);
  });
});

describe('cidrOverlapsAny', () => {
  it('候选集任一相交即真；空候选集恒假', () => {
    expect(cidrOverlapsAny('10.8.0.1', ['192.168.0.0/16', '10.8.0.0/24'])).toBe(true);
    expect(cidrOverlapsAny('172.16.0.1', ['192.168.0.0/16', '10.8.0.0/24'])).toBe(false);
    expect(cidrOverlapsAny('10.8.0.1', [])).toBe(false);
  });
});

describe('meshOverlapRuleIds', () => {
  const mesh = ['10.8.0.0/24', 'fd7a:115c:a1e0::/48'];

  it('已启用 + ipCidr 与组网段相交 → 标记', () => {
    const ids = meshOverlapRuleIds([rule('a', ['10.8.0.0/32'])], mesh);
    expect(ids).toEqual(new Set(['a']));
  });

  it('禁用规则不标（不下发就抢不走路由）', () => {
    // 变异守卫：删掉 `if (!r.enabled) continue` → 转红。
    expect(meshOverlapRuleIds([rule('a', ['10.8.0.0/32'], false)], mesh)).toEqual(new Set());
  });

  it('非 ipCidr 条件不标（域名/端口不在 IP 路由判定面上）', () => {
    expect(meshOverlapRuleIds([rule('a', ['10.8.0.0/24'], true, 'domain')], mesh)).toEqual(
      new Set()
    );
  });

  it('多条件规则里只要有一条 ipCidr 相交就标', () => {
    const multi: Rule = {
      id: 'm',
      type: 'domain',
      values: ['example.com'],
      conditions: [
        { type: 'domain', values: ['example.com'] },
        { type: 'ipCidr', values: ['203.0.113.0/24', '10.8.0.9'] },
      ],
      action: 'proxy',
      enabled: true,
    } as Rule;
    expect(meshOverlapRuleIds([multi], mesh)).toEqual(new Set(['m']));
  });

  it('无组网段（没开组网/无 force-route）→ 恒空，不逐规则空转', () => {
    expect(meshOverlapRuleIds([rule('a', ['10.8.0.0/24'])], [])).toEqual(new Set());
  });

  it('不相交的规则不标（避免全列表刷警告的噪音角标）', () => {
    expect(meshOverlapRuleIds([rule('a', ['192.168.1.0/24'])], mesh)).toEqual(new Set());
  });
});

/**
 * 「覆盖组网」角标的**真值源**：后端本次结算，而不是渲染端重算。
 *
 * 本组的主用例（①）钉的是一个**真缺陷**：自建 headscale 的 tailnet 前缀由控制面下发（实测
 * `32.0.0.0/24`），它只经**外化 rule-set 腿**进配置（段住在文件里、热重载），于是
 *  - 渲染端重算看到的是两条硬编码常量（用例④证明），
 *  - 后端报告的 `emitted` 对这条腿恒空（用例⑤证明），
 * 两条路都标不出来 —— 而用户那条规则确实会遮蔽 tailnet（自定义规则排在组网之前，首匹配）。
 * 判据因此必须是 `emitted ∪ externalRuleSetCidrs`。
 */
describe('forceRoutedCidrsFromReport —— 角标判据的真值源', () => {
  /** 自建 tailnet：观测地址 32.0.0.28，只走外化 rule-set 腿（emitted 恒空）。 */
  const EXTERNAL_LEG: ServerForceRoute = {
    serverId: 'ts-self',
    leg: 'externalRuleSet',
    hasObservation: true,
    emitted: [],
    externalRuleSetCidrs: ['32.0.0.28/32', '100.100.100.100/32', 'fd7a:115c:a1e0::53/128'],
    absorbed: [],
    coverage: 'covered',
  };
  const INLINE_LEG: ServerForceRoute = {
    serverId: 'wg-a',
    leg: 'inline',
    hasObservation: false,
    emitted: ['10.9.0.0/24'],
    externalRuleSetCidrs: [],
    absorbed: [],
    coverage: 'covered',
  };
  const report = (servers: ServerForceRoute[]): EndpointForceRouteReport => ({
    servers,
    zeroCoverageServerIds: [],
    absorbedCount: 0,
  });

  /** 用户写的一条覆盖自建 tailnet 的规则。 */
  const COVERS_TAILNET = rule('r-tailnet', ['32.0.0.0/24']);

  it('① 仅走 externalRuleSet 腿的 Tailscale 节点 + 覆盖其观测段的规则 ⇒ 角标亮', () => {
    const cidrs = forceRoutedCidrsFromReport(report([EXTERNAL_LEG]));
    expect(meshOverlapRuleIds([COVERS_TAILNET], cidrs)).toEqual(new Set(['r-tailnet']));
  });

  it('② 负向对照：规则不与任何组网段相交 ⇒ 不亮', () => {
    const cidrs = forceRoutedCidrsFromReport(report([EXTERNAL_LEG, INLINE_LEG]));
    expect(meshOverlapRuleIds([rule('r-other', ['203.0.113.0/24'])], cidrs)).toEqual(new Set());
  });

  it('③ 拿不到报告 ⇒ 空段集 ⇒ 一个角标都不标（宁可漏标不可假警报）', () => {
    expect(forceRoutedCidrsFromReport(null)).toEqual([]);
    expect(meshOverlapRuleIds([COVERS_TAILNET], forceRoutedCidrsFromReport(null))).toEqual(
      new Set()
    );
  });

  it('④ 正面对照：渲染端重算结构上看不见这个段 —— 判据非搬到后端不可', () => {
    // 被删掉的 `meshForcedRouteCidrs` 的唯一段来源就是它。对 Tailscale 恒产出两条硬编码常量，
    // 与自建 tailnet 的 32.0.0.0/24 零相交 ⇒ 角标结构性不亮。这一条红了，说明重算那份又活了
    // 或者本组的夹具不再代表自建 tailnet，两种情况都必须停下来看。
    const recomputed = endpointForcedRouteCidrs({
      id: 'ts-self',
      name: 'ts-self',
      protocol: 'tailscale',
      tailscaleSettings: {},
    } as unknown as ServerConfig);
    expect(recomputed).toEqual([TAILNET_CGNAT, TAILNET_ULA_V6]);
    expect(meshOverlapRuleIds([COVERS_TAILNET], recomputed)).toEqual(new Set());
  });

  it('⑤ 正面对照：只读 emitted 也标不出来 —— 并集必须含 externalRuleSetCidrs', () => {
    const emittedOnly = report([EXTERNAL_LEG]).servers.flatMap((s) => s.emitted);
    expect(emittedOnly).toEqual([]);
    expect(meshOverlapRuleIds([COVERS_TAILNET], emittedOnly)).toEqual(new Set());
  });

  it('⑥ inline 腿的 emitted 同样进并集，且跨节点去重', () => {
    const dup: ServerForceRoute = { ...INLINE_LEG, serverId: 'wg-b' };
    const cidrs = forceRoutedCidrsFromReport(report([INLINE_LEG, dup, EXTERNAL_LEG]));
    expect(cidrs.filter((c) => c === '10.9.0.0/24')).toHaveLength(1);
    expect(cidrs).toContain('32.0.0.28/32');
    expect(meshOverlapRuleIds([rule('r-wg', ['10.9.0.5'])], cidrs)).toEqual(new Set(['r-wg']));
  });
});
