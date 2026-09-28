/**
 * 首页三张分析卡的**按字节聚合**，纯函数。
 *
 * 规格：`mobile-kit/component-specs/{traffic-composition,rule-hits,host-traffic-top}.md`
 * + `mobile-kit/data-contract.json#cards.*`。
 *
 * # 一个窗口，三张卡（`data-contract.json#globalRules` 第 2 条）
 *
 * `traffic-composition` / `rule-hits` / `host-traffic-top` 共用**活动连接**这一个窗口。
 * 它们在同一屏上被一起读，口径不同就必然当着用户的面互相打脸。任何一张要改窗口，三张一起改。
 * 代价是「连接一关就从图里消失」——这是诚实的，且三张一致。
 *
 * 被否掉的两个替代（记在这里，免得有人再开一次）：
 *  · **并上已结束历史**：那份历史在上限处淘汰最旧的，于是得到的是一个**有界近似**却被印成精确值；
 *  · **真正的累计命中计数器**：数据面里**没有**这个东西，要新加一个核内计数器 ——
 *    不值得为一张首页卡做一次数据面改动。
 *
 * # 不许用 `ConnectionAggHost.count` / `ConnectionAggOutbound.count`
 *
 * 那两个字段是**连接条数**，不是字节数（`data-contract.json#sources.connections-aggregate.traps`）。
 * 它们为桌面拓扑视图而存在。按字节排名的卡必须自己聚合 `ConnectionEntry.upload + download`。
 *
 * # 判据与后端**逐字同源**，不另发明
 *
 * `outboundOf` / `hostNameOf` 是 `crates/stats-engine/src/aggregator.rs` 的
 * `outbound_of` / `host_name_of` 的 1:1 转写（该文件头注写明这两条原本就是从前端
 * `connections-aggregate.ts` 搬过去的，那份前端文件已随搬迁删除，Rust 侧成了唯一真值）。
 *
 * `outboundOf` 取 **`chains[0]`**（首跳出站 tag；`chains` 为空或首项是空串时回落 `Direct`），
 * 与 `traffic-composition.md:25` 及 `data-contract.json:160` 逐字一致 —— 那两处原先写的是
 * 「last element of `chains`」，已于 2026-09-04 更正为 `chains[0]`。**这一条不再是偏离**：
 * 早先的头注说的是更正之前的 spec 文本，代码本身一直与实况（`aggregator.rs`）一致。
 *
 * ⚠️ `hostNameOf` **仍是**一处偏离，取实况：spec 只写了 host → destinationIP 两级，
 * 实况还有第三级回落到 `rule`。这条登记在
 * `~/docs/polaris/design/polaris-mobile-screen-home-2026-09-04.md`。
 */
import type { ConnectionEntry, ConnectionsDetailUpdate } from '@/contracts/types';

/** 一条按字节排名的条目。 */
export interface ByteSlice {
  readonly key: string;
  readonly bytes: number;
}

/** 规则命中的一格。`policy` 决定环的颜色（spec：跟策略走，不跟规则走）。 */
export interface RuleHit {
  /** `named:` 与 `policy:` 隔离同字名称/类别，点击活动页按同一键筛选。 */
  readonly key: string;
  readonly rule: string;
  readonly kind: 'named' | 'policy';
  readonly policy: OutboundPolicy;
  readonly count: number;
}

/** 出站策略。环色与图例色都按它取，故三张卡里同一条连接的语义一致。 */
export type OutboundPolicy = 'proxied' | 'direct' | 'blocked' | 'unknown';

/** 单条连接的字节数。`upload` / `download` 是 optional：缺席按 0 参与排名，但**不得**当作测得的 0 渲染。 */
export function bytesOf(e: ConnectionEntry): number {
  return (e.upload ?? 0) + (e.download ?? 0);
}

/** 出站 tag。`aggregator.rs:286` 的 1:1 转写。 */
export function outboundOf(e: ConnectionEntry): string {
  const first = e.chains[0];
  return first !== undefined && first !== '' ? first : 'Direct';
}

/** host 显示名。`aggregator.rs:270` 的 1:1 转写：host > destinationIP > rule。 */
export function hostNameOf(e: ConnectionEntry): string {
  const host = e.metadata?.host;
  if (host != null && host !== '') return host;
  const ip = e.metadata?.destinationIP;
  if (ip != null && ip !== '') return ip;
  return e.rule;
}

/**
 * 出站 tag → 策略。
 *
 * 只认两个**语义**出站（直连 / 阻断），其余一律是代理。判据是 tag 名的规范化比较而不是子串匹配：
 * 子串匹配会把一个叫 `直连备用-block-list` 的用户自建出站误判成阻断。
 */
export function policyOf(outbound: string): OutboundPolicy {
  const k = outbound.trim().toLowerCase();
  if (k === '') return 'unknown';
  if (k === 'direct') return 'direct';
  if (k === 'block' || k === 'reject' || k === 'blocked') return 'blocked';
  return 'proxied';
}

/** 运行态命名优先；无可证明归属时只按真实首跳出站的业务类别归组。 */
export function ruleGroupOf(e: Pick<ConnectionEntry, 'chains' | 'ruleName'>): {
  key: string;
  rule: string;
  kind: 'named' | 'policy';
} {
  const name = e.ruleName?.trim();
  if (name) return { key: `named:${name}`, rule: name, kind: 'named' };
  const policy = policyOf(e.chains[0] ?? '');
  return { key: `policy:${policy}`, rule: policy, kind: 'policy' };
}

/** 按 key 聚合字节并降序。`tieBreak` 用 key 字典序 —— 同字节数时排序必须稳定，否则卡片每刷新一次就换一次顺序。 */
function rank(entries: readonly ConnectionEntry[], keyOf: (e: ConnectionEntry) => string): ByteSlice[] {
  const acc = new Map<string, number>();
  for (const e of entries) {
    const k = keyOf(e);
    acc.set(k, (acc.get(k) ?? 0) + bytesOf(e));
  }
  return [...acc]
    .map(([key, bytes]) => ({ key, bytes }))
    .sort((a, b) => b.bytes - a.bytes || a.key.localeCompare(b.key));
}

/** `traffic-composition`：按出站分组的字节占比。 */
export function compositionOf(entries: readonly ConnectionEntry[]): readonly ByteSlice[] {
  return rank(entries, outboundOf);
}

/** `host-traffic-top`：按 host 分组的字节 Top N（首页固定 5，故卡高可预测）。 */
export function hostTopOf(entries: readonly ConnectionEntry[], limit = 5): readonly ByteSlice[] {
  return rank(entries, hostNameOf).slice(0, limit);
}

/**
 * 主机筛选：在**完整活动连接集**上先过滤，再交给 [`hostTopOf`] 排名取前 5。
 *
 * # 判据与后端逐字同源，同 `outboundOf` / `hostNameOf` 那一条
 *
 * 这是 `crates/stats-engine/src/aggregator.rs` 里
 * `project_connections_topology_iter` 那段 filter 的 1:1 转写：
 * `query.trim().to_lowercase()`，空串放行全部，否则 `host_name_of(c)` 或 `outbound_of(c)`
 * 任一含该子串即命中（大小写不敏感）。桌面首页那个筛选框走的就是它（`api.stats.projectTopology`
 * 把 query 发到后端），移动端的三张分析卡是本地聚合、够不着那条 IPC ⇒ 只能在本地转写判据。
 * 另发明一条（比如只匹配 host）会让同一个词在两个客户端上筛出两套结果。
 *
 * # 为什么过滤必须在排名**之前**
 *
 * 卡上只画 Top 5。先排名再筛选 = 用户只能在已经看得见的 5 行里筛，那不解决任何问题；
 * 先筛选再排名才让「排在第 30 位的那个域名」够得着 —— 这正是桌面那条注释
 * 「过滤必须发生在投影之前，搜索不会受常态绘制预算影响」说的同一件事。
 */
export function filterByHostQuery(
  entries: readonly ConnectionEntry[],
  query: string,
): readonly ConnectionEntry[] {
  const q = query.trim().toLowerCase();
  if (q === '') return entries;
  return entries.filter(
    (e) => hostNameOf(e).toLowerCase().includes(q) || outboundOf(e).toLowerCase().includes(q),
  );
}

/** 三张卡共用的合计。卡头印的总数必须**恰好**是它下面各部分之和（`globalRules` 第 3 条）。 */
export function totalBytesOf(entries: readonly ConnectionEntry[]): number {
  let sum = 0;
  for (const e of entries) sum += bytesOf(e);
  return sum;
}

/**
 * `rule-hits`：活动连接里各规则的**条数**分布，完整保留所有分组。
 *
 * 这里数的是连接**条数**而不是字节 —— 与另外两张卡的「按字节」不冲突：这张卡回答的问题是
 * 「哪些规则在裁决流量」，一条大流量连接不比一条小流量连接更能说明规则在起作用。
 * 卡副标题必须始终写明窗口是活动连接（数字会随连接开关而动，不说明就读作 bug）。
 *
 * 同一规则下若出现多种策略（规则指向一个会切换的出站组），取**该规则下条数最多**的那个策略，
 * 而不是碰到的第一个 —— 后者会让环色随帧抖动。
 */
export function ruleHitsOf(entries: readonly ConnectionEntry[], limit = Number.POSITIVE_INFINITY): readonly RuleHit[] {
  const acc = new Map<string, { rule: string; kind: RuleHit['kind']; count: number; policies: Map<OutboundPolicy, number> }>();
  for (const e of entries) {
    const group = ruleGroupOf(e);
    const key = group.key;
    let slot = acc.get(key);
    if (slot === undefined) {
      slot = { rule: group.rule, kind: group.kind, count: 0, policies: new Map() };
      acc.set(key, slot);
    }
    slot.count += 1;
    const p = policyOf(e.chains[0] ?? '');
    slot.policies.set(p, (slot.policies.get(p) ?? 0) + 1);
  }
  return [...acc]
    .map(([key, slot]) => ({
      key,
      rule: slot.rule,
      kind: slot.kind,
      count: slot.count,
      policy: [...slot.policies].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))[0][0],
    }))
    .sort((a, b) => b.count - a.count || a.key.localeCompare(b.key))
    .slice(0, limit);
}

/**
 * 活动连接表。`detail` 是**增量流**，故消费方要自己维护一份表。
 *
 * `generation` 是这份表的**身份**，不是一个可以忽略的字段：它变了就表示上游换了数据集
 * （重连 / reset / 离线恢复），旧表必须整体丢弃而不是继续 upsert 上去 ——
 * 「忽略 generation 的消费方会在一次重连之后把两份数据集合并」是 `data-contract.json` 逐字记的陷阱。
 */
export interface ActiveTable {
  readonly generation: number;
  readonly byId: ReadonlyMap<string, ConnectionEntry>;
}

export const EMPTY_ACTIVE_TABLE: ActiveTable = { generation: -1, byId: new Map() };

/** 吃一帧增量，返回新表。 */
export function applyDetail(prev: ActiveTable, u: ConnectionsDetailUpdate): ActiveTable {
  const fresh = u.reset || u.generation !== prev.generation;
  const next = new Map(fresh ? [] : prev.byId);
  for (const c of u.connections) next.set(c.id, c);
  /* 常态帧不重复携带静态字段，只给累计计数 ⇒ **就地合并**，不整条替换
     （整条替换会把 `chains` / `rule` 抹掉，三张分析卡当场失去分组依据）。 */
  for (const c of u.counters ?? []) {
    const cur = next.get(c.id);
    if (cur !== undefined) next.set(c.id, { ...cur, upload: c.upload, download: c.download });
  }
  for (const id of u.removedIds ?? []) next.delete(id);
  return { generation: u.generation, byId: next };
}
