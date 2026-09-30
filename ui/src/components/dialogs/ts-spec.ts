/**
 * Tailscale **字段表**的平台无关半 —— 从 `TsSettingsDialog.tsx` 里拆出来的那一半。
 *
 * 拆分理由与 `wg-spec.ts` / `field-spec.ts` **逐字相同**（见那两份文件头注）：规格是纯数据、
 * 与平台无关；弹窗（`Modal` / `FormTabs` / `.fld`）的外观住在桌面层叠链上，且移动端按契约 A1
 * 不许 import 桌面 `.tsx`。拆开之后本文件零 React，`TsSettingsDialog.tsx` 与
 * `mobile/forms/TsSettingsPanel.tsx` 共用同一张表 —— 两端不会各有一份「Tailscale 有哪些字段」。
 *
 * 🔴 **本文件是覆盖门的判据面**（`contracts/protocol-settings-coverage.test.ts` 的
 * `TailscaleSettings` 编辑器清单里，`TsSettingsDialog.tsx` 这一行随本次搬运换成了本文件）。
 * 那道门要的是**控件**：只在 `ts-settings-logic.ts` 里加读写而不在这张表里加一项，
 * 字段仍然是「用户改不了」，门会红。
 *
 * 分组（基础 / 路由 / 高级）不在这里，在 `mesh-form-layout.ts#groupTsFields` —— 那份也是 `.ts`，
 * 两端共用同一份分区，桌面渲染成页签、移动端渲染成可折叠段。
 */

import type { FieldSpec, SelectOption } from './field-spec';
import { EXIT_CUSTOM } from './ts-settings-logic';
import { ON_DEMAND_FIELD } from './on-demand-field';

export function tsMainSpec(
  exitOpts: readonly SelectOption[],
  detourOpts: readonly SelectOption[],
  interfaceOpts: readonly SelectOption[],
): FieldSpec[] {
  return [
    { t: 'text', k: 'hostname', label: 'ts.hostname', ph: 'sway-macbook' },
    { t: 'select', k: 'exitNode', label: 'ts.exitNode', options: exitOpts },
    { t: 'text', k: 'exitNodeCustom', label: 'ts.exitNodeCustom', ph: '100.x.y.z / hostname', mono: true, when: (v) => v.exitNode === EXIT_CUSTOM },
    // 接入模式（上游 `AccessModeField`，同绑 reverseMesh）：上游 归常显的「接入与出口」段，故**不入高级折叠**——
    // 它决定 `meshUsesSystemInterface`，进而决定该节点是否参与测速（domain/endpoint-routes.ts:320）。
    // 藏起来 = 用户卡在「为什么这个节点测不出速」。降级由后端兜底（builder/outbounds.rs:145 在非 TUN /
    // Windows 上把 system_interface 打回 false），故此处只需如实告知，不必复刻 上游的置灰选择器。
    { t: 'switch', k: 'reverseMesh', label: 'ts.reverseMesh', hint: 'ts.reverseMeshHint' },
    { t: 'switch', k: 'alwaysRouteSubnets', label: 'ts.alwaysRoute', hint: 'ts.alwaysRouteHint' },
    { t: 'switch', k: 'acceptRoutes', label: 'ts.acceptRoutes', hint: 'ts.acceptRoutesHint' },
    // routes ≠ advertiseRoutes（两个相反方向，上游 同样分列两处、绝不合并）：
    //   routes         = 把这些网段的流量**送进**此节点（force-route 源，等价 WG allowedIPs）；
    //   advertiseRoutes= 本机作子网路由器**对外宣告**我能到达这些段。
    { t: 'text', k: 'routes', label: 'ts.routes', ph: '192.168.50.0/24, 10.0.0.0/24', mono: true, opt: true },
    { t: 'switch', k: 'exitNodeAllowLanAccess', label: 'ts.allowLan', hint: 'ts.allowLanHint' },
    { t: 'text', k: 'advertiseRoutes', label: 'ts.advertiseRoutes', ph: '192.168.1.0/24, 10.0.0.0/8', mono: true, opt: true },
    // 前置代理 —— **对 上游的有意偏离**（它的 Tailscale 表单没有这一项）。接线与实测见
    // `detour-options.ts` 文件头 / `crates/config-engine/src/singbox/endpoint.rs`。
    //
    // **提示文案与 WG/WARP 那两处刻意不同**：Tailscale 经前置代理的是**控制面 / DERP 的 TCP 拨号**
    // （2026-07-31 loopback A/B 实测：有 detour ⇒ 控制面直连 0 次、SOCKS5 `CONNECT` 32 次），
    // 只需 TCP，没有 WG 那条「必须支持 UDP 转发」的硬约束。抄同一句话会误导用户去换代理。
    { t: 'select', k: 'detour', label: 'ts.detour', options: detourOpts, hint: 'ts.detourHint' },
    { t: 'select', k: 'bindInterface', label: 'node.bindInterface', options: interfaceOpts, hint: 'node.bindInterfaceHint' },
  ];
}
export const TS_ADV_SPEC: FieldSpec[] = [
  { t: 'text', k: 'controlUrl', label: 'ts.controlUrl', ph: 'https://controlplane.tailscale.com', mono: true, opt: true },
  { t: 'text', k: 'advertiseTags', label: 'ts.aclTags', ph: 'tag:server, tag:exit', mono: true, opt: true },
  { t: 'switch', k: 'ephemeral', label: 'ts.ephemeral', hint: 'ts.ephemeralHint' },
  // 低频专家项，跟 上游 一样归「高级」。u16：越界值会让整份 UserConfig 反序列化失败（同 server_config.rs:208
  // 记的那类整机不可用），故提交前限 1..=65535，越界按未填处理。
  // 自己这条 WireGuard 腿的 UDP 口。留空 = tsnet 随机选；填死才能在上游路由做端口映射，
  // 决定的是「能不能直连打洞」而不是「通不通」—— 不填也能用（回落 DERP 中继），只是绕远。
  // 越界口径同 relayServerPort（见其注释）。
  { t: 'number', k: 'listenPort', label: 'ts.listenPort', hint: 'ts.listenPortHint', ph: '41641', mono: true, opt: true },
  { t: 'number', k: 'relayServerPort', label: 'ts.relayPort', ph: '0', mono: true, opt: true },
  { t: 'switch', k: 'sshServer', label: 'ts.ssh', hint: 'ts.sshHint' },
  // P4b 按名解析：与 acceptDefaultResolvers 强联动。后端 `accept_default_resolvers` **只在 resolveByName
  // 为真的分支里被读**（builder/dns.rs:1069，选节点的谓词就是 resolve_by_name==Some(true)）—— 故此前
  // 「接受 DNS（MagicDNS）」那个常显开关是**恒无效**的：resolveByName 无处可设 ⇒ dns-tailscale server 永不发射。
  // 两者同归高级并加 `when` 门控，既复刻 上游 分区，也让「开了没反应」这件事结构上不再可能。
  { t: 'switch', k: 'resolveByName', label: 'ts.resolveByName', hint: 'ts.resolveByNameHint' },
  { t: 'switch', k: 'acceptDefaultResolvers', label: 'ts.acceptDefaultResolvers', hint: 'ts.acceptDefaultResolversHint', when: (v) => v.resolveByName === true },
  // 按需连接：与 `bindInterface` 同属 ServerConfig 顶层，定义与读写收在 `on-demand-field.ts` 一份。
  ON_DEMAND_FIELD,
];
