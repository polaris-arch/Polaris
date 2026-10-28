/**
 * 路由规则 × 组网(WG/Tailscale) force-route 段的**重叠判定**（`meshOverlapRuleIds`）。
 *
 * # 它回答什么
 *
 * 组网节点会为自己的 `allowedIPs` / tailnet 段发射 force-route 条目（`endpointForcedRouteCidrs`），
 * 但**自定义规则的优先级更高**（优先级链：自定义规则 → 应用分流 → 组网 → 绕过局域网 → 智能 → 默认出口）。
 * 于是一条 `ipCidr` 规则只要与组网段相交，那一段流量就**不再走组网节点**了 —— sing-box 的
 * 「首条命中即生效」让这件事完全静默：配置能生成、内核能起、连接也通，只是没走你以为的路。
 * 本谓词把它在规则列表里标成「覆盖组网」角标（对齐 上游 `rules-page.tsx:109-131`）。
 *
 * # 为什么在这里新写一份 CIDR 相交
 *
 * 生成侧的跨节点结算（Rust `builder::endpoint_routes::settle_force_route_claims`，经
 * `endpoint_force_route_report` 命令回给节点卡角标）只做**字面量去重**（同一条 cidr 串被两个
 * 节点声明），够它自己那个「同段先声明者胜」的判定，但答不了「`10.0.0.0/8` 与 `10.8.0.0/24` 相交吗」。
 * `domain/rules.ts` 的 `isValidIpCidr` 只判形状。故此处移植 上游 `src/shared/ip.ts:60-102` 的
 * 前缀比对算法（v4 用 uint32、v6 用 BigInt，跨族恒不相交），**逐字同口径**，不引第三方依赖。
 *
 * 纯函数、无 I/O。
 */
import type { Rule } from '../contracts/types';
import type { EndpointForceRouteReport } from '../contracts/endpoint-force-route-report';
import { dedupe } from './collections';
import { ruleIpCidrs } from './rules';

import { cidrsOverlap } from './ip-cidr';
export { ipv4CidrsOverlap, ipv6CidrsOverlap, cidrsOverlap } from './ip-cidr';

/** target 是否与候选集里任一 CIDR 相交（v4+v6 家族感知）。 */
export function cidrOverlapsAny(target: string, candidates: string[]): boolean {
  return candidates.some((c) => cidrsOverlap(target, c));
}

/**
 * 「本轮块 0c 实际让哪些段走组网节点」—— 取自**后端本次结算**，渲染端不再自己算一份。
 *
 * 对位 Rust `builder::endpoint_routes::settled_force_route_cidrs`（`emitted` ∪
 * `externalRuleSetCidrs`），并集口径必须与它一致。
 *
 * # 为什么不能是渲染端重算
 *
 * 角标此前走 `meshForcedRouteCidrs(meshForceRoutedServers(...))`，而那份重算的段来自
 * `endpointForcedRouteCidrs` —— 对 Tailscale **恒**产出两条硬编码默认常量
 * （`TAILNET_CGNAT` / `TAILNET_ULA_V6`），而自建 headscale 的 `prefixes.v4` 可自定义，实测发到
 * `32.0.0.0/24`，与官方段零相交。于是用户写一条覆盖自建 tailnet 的规则、那条规则确实会遮蔽
 * tailnet（自定义规则排在组网之前，首匹配），角标却**结构性不亮**。真实前缀只存在于运行期观测
 * 地址里，渲染端拿不到这个维度。
 *
 * # 为什么必须并上 `externalRuleSetCidrs`
 *
 * 自建 tailnet 的观测段走的是 `externalRuleSet` 腿（段在文件里、热重载），那条腿的 `emitted`
 * 恒空 —— 只读 `emitted` 的话，换成后端真值也照样一个段都看不见（见契约里该字段的注释）。
 *
 * # 拿不到报告 ⇒ 返回空集 ⇒ 一个角标都不标
 *
 * 宁可漏标不可假警报（同本屏 `missingResIds` 那段的纪律）：空集会让
 * [`meshOverlapRuleIds`] 短路返回空。**不退回本地重算兜底** —— 那正是本函数要终结的形态本身。
 */
export function forceRoutedCidrsFromReport(
  report: EndpointForceRouteReport | null
): string[] {
  if (report === null) return [];
  return dedupe(
    (report.servers ?? []).flatMap((s) => [
      ...(s.emitted ?? []),
      ...(s.externalRuleSetCidrs ?? []),
    ])
  );
}

/**
 * 与组网 force-route 段重叠的**已启用**规则 id 集合（供规则列表就地角标）。
 *
 * - 只看 `ipCidr` 条件（`ruleIpCidrs`）：域名/端口/进程类规则与 IP 路由段不在同一判定面上，标了是噪音。
 * - **只判已启用规则**：禁用规则不下发，本就抢不走组网的路由（与 `missingResourceRuleIds` 同口径）。
 * - `meshCidrs` 必须由调用方传 [`forceRoutedCidrsFromReport`] 的结果，即**后端本次结算**的那一份
 *   （见该函数的头注：渲染端重算结构上看不见自建 tailnet 的真实前缀）。
 */
export function meshOverlapRuleIds(rules: Rule[], meshCidrs: string[]): Set<string> {
  const ids = new Set<string>();
  if (!meshCidrs.length) return ids;
  for (const r of rules || []) {
    if (!r.enabled) continue;
    if (ruleIpCidrs(r).some((c) => cidrOverlapsAny(c, meshCidrs))) ids.add(r.id);
  }
  return ids;
}
