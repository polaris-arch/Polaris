/**
 * Polaris **自己的**组网节点之间 force-route 段互相吸收的结算报告 —— Rust
 * `builder::endpoint_routes::EndpointForceRouteReport` 的线格式对位
 * （`endpoint_force_route_report` 命令的载荷）。
 *
 * 与 [`TunnelConflictReport`](./tunnel-conflict-report) 成对、射程不重叠：那条问「本机**别的**
 * 隧道与我撞不撞」，本条问「我自己配的这几个组网节点之间撞不撞」。成因与自救动作都不一样。
 *
 * # 消费纪律（两条，缺一即把报告读反）
 *
 *  1. **`zeroCoverageServerIds` 是静默失效，不是信息**：节点活着、engaged、用户以为它在工作，
 *     而它的段被更早声明的节点全部抢走 ⇒ 流量一条都不会到它那儿。必须点名到节点。
 *  2. **`hasObservation === false` 时不许把「段重合」读成「账号撞车」**：Tailscale 的段集恒含两条
 *     硬编码默认常量（`TAILNET_CGNAT` / `TAILNET_ULA_V6`），没有运行期观测地址时两个 TS 节点的段
 *     **必然**完全重合 —— 那不是证据，只是两份一样的猜测。
 */

/** 块 0c 为一个组网节点选的发射腿。只有 `inline` 腿产出 `ip_cidr` 字面量、参与跨节点去重。 */
export type ForceRouteLeg = 'preferredBy' | 'externalRuleSet' | 'inline';

/** 单节点的 force-route 覆盖三态。 */
export type ForceRouteCoverage =
  /** engaged 且自身有段要发，本轮确实发得出属于它的 force-route 规则。 */
  | 'covered'
  /** 🔴 有段要发却一条都发不出 —— 全被更早的节点抢先声明走了。 */
  | 'absorbedEmpty'
  /** 自身本来就没有段要发（如 WARP：全隧道 anycast 出口，天生没有「具体段」）。不是缺陷。 */
  | 'nothingToRoute';

/** 一条被更早声明者抢走的段：`cidr` 不经本节点路由，实际生效的是 `byServerId` 那个节点。 */
export interface AbsorbedCidr {
  cidr: string;
  byServerId: string;
}

export interface ServerForceRoute {
  serverId: string;
  leg: ForceRouteLeg;
  /** 本轮该节点有没有可用的运行期观测段。假 ⇒ 它发的是默认常量段（见模块头注纪律 2）。 */
  hasObservation: boolean;
  /** 本节点**实际发射**的具体段（只有 `inline` 腿非空）。 */
  emitted: string[];
  absorbed: AbsorbedCidr[];
  coverage: ForceRouteCoverage;
}

export interface EndpointForceRouteReport {
  /** 逐节点结算，顺序 = 发射顺序 = 「谁先占」的顺序（与产物 `route.rules` 的相对顺序逐项一致）。 */
  servers: ServerForceRoute[];
  /** 覆盖面被吸收干净的节点 id。**空数组是有意义的结果**（本轮没有节点被吃干净），不是「没算出来」。 */
  zeroCoverageServerIds: string[];
  /** 被吸收的段总数（逐条被丢弃的 cidr +1）。 */
  absorbedCount: number;
}
