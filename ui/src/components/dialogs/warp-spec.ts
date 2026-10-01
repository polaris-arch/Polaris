/**
 * WARP 接入表单的平台无关半 —— 从 `WarpDialog.tsx` 里拆出来的那一半。
 *
 * 拆分理由与 `wg-spec.ts` / `ts-spec.ts` / `field-spec.ts` **逐字相同**（见 `field-spec.ts` 头注）：
 * 规格与校验是纯数据/纯函数，弹窗的外观住在桌面层叠链上，而移动端按契约 A1 不许 import 桌面 `.tsx`。
 * 本文件零 React，`WarpDialog.tsx` 与 `mobile/forms/WarpPanel.tsx` 共用同一份。
 *
 * # 为什么连「提交前校验」也搬过来（不只是字段表）
 *
 * 校验规则**不许重实现**：注册腿的三档拒绝（名字空 / 选了 WARP+ 却没填许可证 / 端点填了但解析不出）
 * 决定的是「这次提交发不发那一个远端请求」。两端各写一份 `if` 的必然结局是两边对同一份输入给出
 * 不同结论，而这类分叉本仓没有任何门守得住 —— 同 `field-spec.ts#parseNumberField` 那条
 * 「全库唯一实现点」的理由。故判据收在 [`planWarpSubmit`]，两个客户端只负责把结论画出来。
 *
 * ⚠️ **`reserved` / `warpDevice` 两个键的控件不在这里**：它们由注册应答直接落进
 * `WireGuardSettings`（`WarpDialog.tsx#doRegister` 那段），不是用户填的字段。
 * 覆盖门的编辑器清单因此仍然要包含 `WarpDialog.tsx`。
 */

import type { FieldSpec, FormValues, SelectOption } from './field-spec';
import type { ServerConfig } from '@/contracts/types';
import { WARP_MTU } from '@/domain/warp';
import { DETOUR_NONE } from './detour-options';
import { ON_DEMAND_FIELD, onDemandDraftValue } from './on-demand-field';

/**
 * 新建时的默认名 `WARP`（不是 `Cloudflare WARP`）：节点卡/节点行的名字位很窄，长名会被截断成
 * 「Cloudflare W…」，而「Cloudflare」这一段对用户毫无区分度 —— 单例槽位里只可能有一个 WARP。
 * 改名不影响身份判定：`isWarpServer` 认的是 `warpDevice` 凭据与 `*.cloudflareclient.com`
 * 端点域名，从不看 name。
 */
export const WARP_DEFAULT_NAME = 'WARP';

/** 计费档。`plus` 需要一条许可证串（`api.server.registerWarp` / `applyWarpLicense` 的入参）。 */
export type WarpPlan = 'free' | 'plus';

/** "host:port" → {host,port}；非法返 null（IPv6 用 [::1]:port）。 */
export function parseHostPort(ep: string): { host: string; port: number } | null {
  const s = ep.trim();
  if (!s) return null;
  let host: string;
  let portStr: string;
  if (s.startsWith('[')) {
    const c = s.indexOf(']');
    if (c < 0) return null;
    host = s.slice(1, c);
    const rest = s.slice(c + 1);
    if (!rest.startsWith(':')) return null;
    portStr = rest.slice(1);
  } else {
    const i = s.lastIndexOf(':');
    if (i < 0) return null;
    host = s.slice(0, i);
    portStr = s.slice(i + 1);
  }
  const port = Number(portStr);
  if (!host || !Number.isInteger(port) || port <= 0 || port > 65535) return null;
  return { host, port };
}

export function warpAdvancedSpec(
  detourOpts: readonly SelectOption[],
  endpointPlaceholder: string,
  interfaceOpts: readonly SelectOption[],
): FieldSpec[] {
  return [
    { t: 'text', k: 'endpoint', label: 'warp.endpoint', ph: endpointPlaceholder, mono: true },
    { t: 'number', k: 'mtu', label: 'warp.mtu', ph: String(WARP_MTU), mono: true, opt: true },
    { t: 'number', k: 'workers', label: 'wg.workers', hint: 'wg.workersHint', mono: true, opt: true },
    { t: 'number', k: 'keepalive', label: 'warp.keepalive', ph: '25', mono: true, opt: true },
    // 前置代理 —— **对 上游的有意偏离**（它的 WARP 表单没有这一项）。本轮之前这个控件是个
    // **装饰开关**：值写进了 `server.detour`，但 Rust 侧 `Endpoint` 结构体压根没有 detour 字段，
    // 序列化时被丢掉。现已真生效（`builder/outbounds.rs` 的 WG endpoint 腿）。
    //
    // hint 那句 UDP 与 `WgDialog` 同源同因：WARP 就是 WireGuard，握手走 UDP，前置代理不支持
    // UDP 转发就静默不通且不回落直连（实测见 `singbox/endpoint.rs`）。
    { t: 'select', k: 'detour', label: 'warp.detour', options: detourOpts, hint: 'warp.detourHint' },
    { t: 'select', k: 'bindInterface', label: 'node.bindInterface', options: interfaceOpts, hint: 'node.bindInterfaceHint' },
  // 按需连接：与 `bindInterface` 同属 ServerConfig 顶层，定义与读写收在 `on-demand-field.ts` 一份。
  ON_DEMAND_FIELD,
  ];
}


/**
 * 存量 WARP 节点 / 新建 → 表单草稿（与 [`warpAdvancedSpec`] 的键一一对应）。
 *
 * 「缺省即默认」同 `wg-logic.ts` / `ts-settings-logic.ts` 的那条纪律：`mtu` / `keepalive` 缺席
 * 回显成 `undefined`（R2：number 空绝不塞 0），`detour` 缺席回显成哨兵 `DETOUR_NONE`
 * （提交侧 `applyDetour` 再把哨兵翻译成删键，不在盘上写字面量 `'direct'`）。
 */
export function warpDraftFromNode(editNode?: ServerConfig): FormValues {
  const ws = editNode?.wireguardSettings;
  if (!editNode) {
    return {
      endpoint: '',
      mtu: undefined,
      workers: undefined,
      keepalive: undefined,
      detour: DETOUR_NONE,
      bindInterface: '',
      onDemand: false,
    };
  }
  return {
    endpoint: `${editNode.address}:${editNode.port}`,
    mtu: ws?.mtu,
    workers: ws?.workers,
    keepalive: ws?.persistentKeepalive,
    detour: editNode.detour || DETOUR_NONE,
    bindInterface: editNode.bindInterface ?? '',
    onDemand: onDemandDraftValue(editNode),
  };
}

/** 提交被拒的三档。`null` = 放行。 */
export type WarpSubmitReject = 'name' | 'license' | 'endpoint';

export interface WarpSubmitPlan {
  /** 非 `null` ⇒ 调用方就地回显那一档，**一个远端请求都不许发**。 */
  readonly reject: WarpSubmitReject | null;
  /** 放行时的端点覆盖：`null` = 用注册应答里带的那个（新建态用户没填端点时的正常路径）。 */
  readonly endpoint: { readonly host: string; readonly port: number } | null;
}

/**
 * 提交前的**唯一**判据（两端共用，见文件头「为什么连校验也搬过来」）。
 *
 * 三档逐条对应桌面既有行为，一字未改：
 *  ① 名字空 —— 节点没有名字，列表里就是一行空白；
 *  ② 档位选了 WARP+ 却没填许可证 —— 那次注册会按 free 档跑完，用户以为买的加速生效了；
 *  ③ 端点**填了但解析不出**，或**编辑态解析不出**（编辑态端点恒有值，解析不出 = 用户把它改坏了）。
 *    M6：此时必须内联报错并保持表单打开，不得静默回退旧地址后假装提交成功。
 *    新建态留空是合法的 —— 端点由注册应答给（`warp.endpointAuto` 那句占位文案说的就是这件事）。
 */
export function planWarpSubmit(input: {
  readonly name: string;
  readonly plan: WarpPlan;
  readonly license: string;
  readonly endpointRaw: string;
  readonly isEdit: boolean;
}): WarpSubmitPlan {
  if (input.name.trim() === '') return { reject: 'name', endpoint: null };
  if (input.plan === 'plus' && input.license.trim() === '') {
    return { reject: 'license', endpoint: null };
  }
  const raw = input.endpointRaw.trim();
  const endpoint = raw ? parseHostPort(raw) : null;
  if ((raw !== '' && !endpoint) || (input.isEdit && !endpoint)) {
    return { reject: 'endpoint', endpoint: null };
  }
  return { reject: null, endpoint };
}
