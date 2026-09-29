/**
 * WireGuard **字段表**的平台无关半 —— 从 `WgDialog.tsx` 里拆出来的那一半。
 *
 * # 为什么要拆（2026-09-06，移动端组网三张表落地时）
 *
 * 与 `field-spec.ts` 从 `FieldSpec.tsx` 里拆出来是**同一件事、同一个理由**：`WgDialog.tsx` 一个文件
 * 里住着两样东西 —— **字段规格**（这张 `FieldSpec[]` 表，纯数据、与平台无关）与**桌面弹窗**
 * （`Modal` / `FormTabs` / `.fld` / `.input`，外观全落在 `prototype.css` / `components.css`
 * = 桌面层叠链）。移动端要复用的只有前者。
 *
 * 而 `mobile/nodes/nodes-screen.test.tsx` ② 的 A 腿边界断言按**扩展名**判：`.tsx` = 桌面屏组件，
 * 一律不许被移动端 import。规格与弹窗同住一个 `.tsx` 时，移动端要么整份引进来（把 `Modal` /
 * `FormTabs` 一起拖进移动包，且违背那条边界的**意图**：控件会藏到 A 腿的源码取材面之外），
 * 要么在移动端重抄一张字段表 —— 而那等于给「普通 WG 节点有没有 Reserved 入口」「接入模式在 WARP
 * 下禁不禁用」这些判据造第二个答案，且**本仓没有任何门**守得住那类分叉。
 *
 * 拆开之后两侧各自成立：本文件零 React（连 `react` 都不认识），两个客户端共用；`WgDialog.tsx`
 * 只剩弹窗，并把 `wgSpec` **原样再导出**，故既有的调用点与门一个字都不用改。
 *
 * 🔴 **本文件是覆盖门的判据面**（`contracts/protocol-settings-coverage.test.ts` 的 `WireGuardSettings`
 * 编辑器清单里，`WgDialog.tsx` 这一行随本次搬运换成了本文件）。那道门要的是**控件**——
 * 只在 `wg-logic.ts` 里加读写而不在这张表里加一项，字段仍然是「用户改不了」，门会红。
 */

import type { ServerConfig } from '@/contracts/types';
import type { FieldSpec, FormValues, SelectOption } from './field-spec';
import { isWarpDraft } from './wg-logic';
import { ON_DEMAND_FIELD } from './on-demand-field';

/**
 * WG 字段表。`draft` / `base` 只被接入模式那一项的禁用判定用到（WARP 判据 = 端点域名 + `warpDevice`
 * 标记，两者分别来自草稿与存量设置），故收成函数（同 `TsSettingsDialog` 的 `mainSpec(exitOpts)`）
 * 而非模块级常量。
 *
 * `draft` 是**必传**：`FieldSpec.disabled` 是静态布尔（见 `FieldSpec.tsx` 那条的文档），少传一个
 * 可选参数就会把禁用静默退化成可用，而这个开关禁用与否是阻断级的。
 */
export function wgSpec(
  draft: FormValues,
  base: ServerConfig | undefined,
  detourOpts: readonly SelectOption[],
  interfaceOpts: readonly SelectOption[],
): FieldSpec[] {
  return [
    { t: 'text', k: 'address', label: 'wg.address', ph: '203.0.113.7', mono: true },
    { t: 'number', k: 'port', label: 'wg.port', ph: '51820', mono: true },
    { t: 'text', k: 'privateKey', label: 'wg.priv', ph: 'wOE... =', mono: true, secret: true },
    { t: 'text', k: 'localAddress', label: 'wg.local', ph: '10.0.0.2/32, fd00::2/128', mono: true },
    { t: 'text', k: 'peerPublicKey', label: 'wg.pub', ph: 'HIg... =', mono: true },
    { t: 'text', k: 'preSharedKey', label: 'wg.psk', mono: true, opt: true, secret: true },
    { t: 'text', k: 'allowedIPs', label: 'wg.allowed', ph: '10.0.0.0/24', mono: true, opt: true },
    { t: 'number', k: 'persistentKeepalive', label: 'wg.keep', ph: '25', mono: true },
    { t: 'number', k: 'mtu', label: 'wg.mtu', ph: '1408', mono: true },
    { t: 'number', k: 'workers', label: 'wg.workers', hint: 'wg.workersHint', mono: true, opt: true },
    // Reserved：**对所有 WG 节点开放**，不是 WARP 专属。上游 `wireguard-form.tsx:488` 同样把它放在
    // 通用 WG 表单里（其 `:53` 注释「reserved 仅 Cloudflare WARP 等需要」说的是**用途**，不是限制）——
    // 「等」不是虚指：任何在 WG 之上做多路复用的服务端（WARP 类网关、自建 xray/sing-box 对端）都靠
    // 这 3 字节把包分派回正确的会话，填不上就只是**连得上、不通**，没有任何报错。此前它只有
    // `WarpDialog` 有入口，普通 WG 节点在 UI 上改不了（`wg-logic.ts` 靠 `...base` 原样带过）。
    { t: 'text', k: 'reserved', label: 'wg.reserved', ph: '0, 0, 0', mono: true, opt: true },
    // 接入模式（上游 `shared/mesh-fields.tsx:32` 的 `AccessModeField`，同绑 reverseMesh）：上游 归常显的
    // 「接入与出口」段、紧邻全隧道开关，故摆在 allowInternet 之前，不入任何折叠——它决定
    // `meshUsesSystemInterface`，进而决定该节点是否参与测速（domain/endpoint-routes.ts:320）。
    // 藏起来 = 用户卡在「为什么这个节点测不出速」。
    //
    // **形态：开关，不是 上游的 gVisor/System 置灰选择器**（与 TsSettingsDialog:69 同一条理由）。
    // 上游 那个选择器在非 TUN / Windows 上置灰显示 gVisor，是渲染端自行复刻平台探测；Polaris 的降级
    // 由后端兜底且**两族同一条代码**：`builder/outbounds.rs:145` 一次算出 `downgrade_mesh`
    // （= mesh_uses_system_interface && !system_interface_available），WG 分支 `:156-159` 打回
    // `system=false` / `name=None`，TS 分支 `:170-172` 打回 `system_interface=false`；
    // `system_interface_available` 本身 = TUN 模式 && 非 Windows（`builder/generate.rs:259-260`）。
    // 故此处只需 hint 如实写明降级条件，不必把平台探测接线搬进渲染端。
    //
    // WARP 下**可见但禁用**，不再整条隐掉（此前是 `when: v => !isWarpDraft(v, base)`，上游
    // `wireguard-form.tsx:299` 的 `{!isWarp && …}` 同款）。不能开是对的（判据与真机实证见
    // `wg-logic.ts#isWarpDraft`），但**隐藏且不解释**会让用户分不清「不支持」与「没做」——
    // 同一形态本轮已在 `WarpDialog` 一并改掉，两个弹窗对同一个概念呈现一致。
    // ⚠️ 禁用只挡住「这次编辑新写入」，提交侧另有否决（`buildWgServer` 的 `isWarpDraft` 那支）。
    { t: 'switch', k: 'reverseMesh', label: 'wg.reverseMesh', hint: 'wg.reverseMeshHint', disabled: isWarpDraft(draft, base), disabledHint: 'wg.reverseMeshWarp' },
    { t: 'switch', k: 'allowInternet', label: 'wg.allowInternet', hint: 'wg.allowInternetHint' },
    { t: 'switch', k: 'alwaysRouteSubnets', label: 'wg.alwaysRoute', hint: 'wg.alwaysRouteHint' },
    // 前置代理 —— **对 上游的有意偏离**（它的 WG 表单没有这一项，`SingBoxEndpoint` 类型也没有
    // 这个键；生成侧的接线与实测见 `detour-options.ts` 文件头 / `singbox/endpoint.rs`）。
    //
    // hint 里那句 UDP 是承重的：WG 的握手走 **UDP**，经前置代理时是 SOCKS5 `UDP_ASSOCIATE`
    // （2026-07-31 loopback A/B 实测：有 detour ⇒ 直达 peer 的 UDP 包 **0**、SOCKS UDP_ASSOCIATE 15 次）。
    // 前置代理只支持 TCP ⇒ 本节点起不来，且**不回落直连**、没有任何报错 —— 用户看到的就是「连上了不通」。
    // 不写这句，这个控件就是个陷阱。
    { t: 'select', k: 'detour', label: 'wg.detour', options: detourOpts, hint: 'wg.detourHint' },
    { t: 'select', k: 'bindInterface', label: 'node.bindInterface', options: interfaceOpts, hint: 'node.bindInterfaceHint' },
  // 按需连接：与 `bindInterface` 同属 ServerConfig 顶层，定义与读写收在 `on-demand-field.ts` 一份。
  ON_DEMAND_FIELD,
  ];
}
