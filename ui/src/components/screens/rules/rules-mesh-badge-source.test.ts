/**
 * 规则列表「覆盖组网」角标的**真值源**门（接线面）。
 *
 * # 守的是什么
 *
 * 角标此前由规则屏**自己重算**：`meshForcedRouteCidrs(meshForceRoutedServers(...))`。那份重算
 * 走 `endpointForcedRouteCidrs`，对 Tailscale **恒**产出 `TAILNET_CGNAT` + `TAILNET_ULA_V6`
 * 两条硬编码常量，且**看不见外化 rule-set 腿**（段住在文件里、热重载）。而自建 headscale 的
 * `prefixes.v4` 由控制面下发（实测 `32.0.0.0/24`），它恰恰只走那条腿。
 *
 * 后果：用户写一条覆盖自建 tailnet 的规则，那条规则**确实会遮蔽 tailnet**（自定义规则排在组网
 * 之前，sing-box 首匹配），角标却结构性不亮 —— 与「没有冲突」长得一模一样。
 *
 * 真值搬到 `endpoint_force_route_report` 命令（吃运行期观测地址、按块 0c 的同一套腿选择与同一次
 * 结算），角标改为消费 `emitted ∪ externalRuleSetCidrs`。
 *
 * # 本门与 `domain/mesh-rule-overlap.test.ts` 的分工（缺一条缝就漏）
 *
 * 那边测的是「给它一份报告，判据算得对」；本门测的是「生产屏到底有没有在用它」。函数被测 ≠
 * 生产在用它 —— 两条判据必须成对交，否则接线换回重算时两边都不会红。
 *
 * 手法沿用 `nodes-shadowed-badge-source.test.tsx` 的源码面形态（去注释后扫代码）：本文件与被扫
 * 文件的注释里都逐字写着被删掉的旧形态，不剥注释必然自伤。
 */
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const read = (rel: string): string =>
  readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');

/** 去注释 —— 负向断言必须跑在它上面（同 `nodes-shadowed-badge-source` 的理由）。 */
const code = (src: string): string =>
  src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/.*$/gm, '$1');

const SCREEN_RAW = read('./RulesScreen.tsx');
const ROUTES_RAW = read('../../../domain/endpoint-routes.ts');
const OVERLAP_RAW = read('../../../domain/mesh-rule-overlap.ts');
const CONTRACT_RAW = read('../../../contracts/endpoint-force-route-report.ts');
const SCREEN = code(SCREEN_RAW);
const ROUTES = code(ROUTES_RAW);
const OVERLAP = code(OVERLAP_RAW);
const CONTRACT = code(CONTRACT_RAW);

describe('接线 · 「覆盖组网」角标的真值源是后端报告', () => {
  it('自曝：取材面还在（去注释后仍是可断言的代码）', () => {
    // 少了这一条，上面四个 `read` 哪天路径写错、读到空串，下面所有负向断言会一起「通过」。
    expect(SCREEN_RAW.length).toBeGreaterThan(1000);
    expect(ROUTES_RAW.length).toBeGreaterThan(1000);
    expect(SCREEN).toContain('export function RulesScreen');
    expect(OVERLAP).toContain('export function forceRoutedCidrsFromReport');
  });

  it('规则屏拉 `endpoint_force_route_report`，并把它原样喂给角标判据', () => {
    expect(SCREEN).toContain('api.config');
    expect(SCREEN).toContain('.endpointForceRouteReport()');
    expect(SCREEN).toContain('forceRoutedCidrsFromReport(forceRouteReport)');
    expect(SCREEN).toContain('meshOverlapRuleIds(rules, forceRoutedCidrsFromReport(forceRouteReport))');
  });

  it('拉不到报告时留在 null（= 空段集 = 不标），不退回本地重算', () => {
    expect(SCREEN).toMatch(/catch\(\(\)\s*=>\s*\{[^}]*setForceRouteReport\(null\)/);
  });

  it('规则屏一行本地重算都不剩', () => {
    for (const banned of [
      'meshForcedRouteCidrs',
      'meshForceRoutedServers',
      'endpointForcedRouteCidrs',
    ]) {
      expect(SCREEN, `规则屏又出现了本地重算：${banned}`).not.toContain(banned);
    }
  });

  it('`meshForcedRouteCidrs` 已从 domain 层删除（不是留着没人用）', () => {
    // 留一份没人用的同义实现，下一个人接上它就等于回归；墓碑注释在原文里，故扫去注释面。
    expect(ROUTES).not.toContain('meshForcedRouteCidrs');
  });

  it('并集口径含 `externalRuleSetCidrs` —— 只读 emitted 对自建 tailnet 恒空', () => {
    // 判据的两个取材面都必须出现在实现里。少了后者，角标对走外化 rule-set 腿的 tailnet
    // 结构性不亮，而这一格恰是本批要修的那个缺陷。
    expect(OVERLAP).toContain('s.emitted');
    expect(OVERLAP).toContain('s.externalRuleSetCidrs');
    // 线格式是 camelCase：Rust 侧 `#[serde(rename_all = "camelCase")]` 漏了会让这个字段恒
    // `undefined`，而 `undefined` 在角标判据里是假值 ⇒ 恒不亮，且 tsc 与两侧单测都不红。
    // Rust 那半由 `endpoint_routes::tests::server_force_route_wire_shape_is_camel_case` 钉住。
    expect(CONTRACT).toContain('externalRuleSetCidrs: string[]');
  });
});
