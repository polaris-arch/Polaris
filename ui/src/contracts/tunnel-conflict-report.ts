/**
 * 本机**外来**隧道冲突报告 —— Rust `runtime::proxy::tunnel_conflict::TunnelConflictSnapshot::to_wire`
 * 的线格式对位（`tunnel_conflict_report` 命令的载荷）。
 *
 * # 为什么是判别联合，而不是一个带可选 `conflicts` 的对象
 *
 * 后端**刻意**只在 `probed` 那一支带 `conflicts` 键，其余三支缺键。理由写在 Rust 侧同名模块的
 * 头注里：带一个空数组的话，渲染端最自然的 `conflicts.length === 0` 会把「没探成」读成一句
 * 自信的「无冲突」—— 而 macOS / Windows **根本没有探测实现**，2026-09-08 报障那台正是 macOS。
 *
 * 把它写成判别联合是让这条纪律在**类型层**生效：不先判 `status` 就取 `conflicts`，tsc 直接报错。
 * 写成 `{ status: string; conflicts?: TunnelConflict[] }` 就把这道保护拆了，渲染端又能写出
 * `(report.conflicts ?? []).length === 0`。
 */

/** 冲突类别。镜像 Rust `builder::tunnel_conflict::ConflictKind`（serde camelCase）。 */
export type TunnelConflictKind = 'fakeIpOverlap' | 'meshOverlap' | 'tunAddressOverlap';

/**
 * 探测到的一条**别人的**隧道路由。`interface` 只用于告诉用户「是谁」，不参与判据。
 *
 * 后端下发的是**展示面**：link-local（`fe80::/10` / `169.254.0.0/16`）与组播
 * （`ff00::/8` / `224.0.0.0/4`）已在 Rust 侧摘掉，摘掉多少条走 `suppressedRoutes`。
 * 渲染端**不再自己过滤** —— 那会逼出第二份 CIDR 包含算术（本仓 TS 侧只有 `cidrsOverlap`），
 * 而两份判据迟早会漂。收在后端还有一层好处：噪声根本不跨 IPC，命令的每个消费方口径一致。
 */
export interface ForeignTunnelRoute {
  interface: string;
  prefix: string;
}

/** 一条判定结果：某个外来接口宣告的前缀，与本次发射配置的哪一类段相交。 */
export interface TunnelConflict {
  interface: string;
  prefix: string;
  kind: TunnelConflictKind;
}

/**
 * 本次判定用的三组段，全部取自**发射那一刻**交给内核的那份配置
 * （核起来后用户再改配置也不会改写它，见 Rust `ConflictCriteria` 的头注）。
 */
export interface TunnelConflictCriteria {
  fakeipRanges: string[];
  meshCidrs: string[];
  tunAddresses: string[];
}

export type TunnelConflictReport =
  /** 本会话还没探过：核没起过 / 本次不是 TUN 模式 / 探测腿尚未跑完。 */
  | { status: 'notProbed' }
  /** 本平台没有探测实现（macOS / Windows）。`platform` 取 Node 约定名（darwin / win32 / linux / other）。 */
  | { status: 'unsupported'; platform: string }
  /** 探测命令失败（spawn / 超时 / 非零退出）。`error` 是后端的诊断串。 */
  | { status: 'probeFailed'; error: string }
  /** 探到了 —— **只有这一支**里的空 `conflicts` 才是一句断言。 */
  | {
      status: 'probed';
      /** 外来隧道宣告的**业务**网段（link-local / 组播已在后端收掉，见 `ForeignTunnelRoute`）。 */
      foreignTunnels: ForeignTunnelRoute[];
      /**
       * 被收掉的 link-local / 组播条数。
       *
       * 它在界面上不是装饰：2026-09-12 真机实测，一台 Tailscale 断开的 mac 上 36 条 foreign
       * **全部**是这两族 ⇒ `foreignTunnels` 为空。只显示「0 条」的话，「探过了」与「压根没探」
       * 又变回同一副样子 —— 而这四支判别联合存在的全部理由就是把这两件事分开。
       */
      suppressedRoutes: number;
      conflicts: TunnelConflict[];
      criteria: TunnelConflictCriteria;
    };
