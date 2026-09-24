/**
 * 协议「必填字段是否齐备」的**单一真值**（忠实迁移 上游 `src/shared/server-completeness.ts`）。
 *
 * 上游 中它是主侧 `ConfigManager.validateConfig`（缺则 throw）与渲染侧首页连接闸门 `isServerComplete`
 * （缺则置灰）的共用判据。Polaris 后端校验在 Rust（crates/store/validate.rs），渲染侧此前无等价谓词；
 * 本模块补齐渲染侧的纯前端判定，供「删选中兜底出口」可用性过滤（node-delete-fallback）等消费——
 * 杜绝把配置不齐/不可路由的节点当兜底出口静默选中（#291 类裸奔）。
 *
 * 新增协议：只改这里一处 + 同步 ALL_PROTOCOLS（与 types.ts `Protocol` 联合派生），消费方自动覆盖。
 */
import type { ServerConfig, Protocol } from '../contracts/types';
import type { TailcatSettings } from '../contracts/types/protocol-settings';
import { isMeshNodeUnroutable } from './endpoint-routes';

/** 受支持协议的权威运行时清单（从 types.ts `Protocol` 联合派生）。 */
export const ALL_PROTOCOLS: readonly Protocol[] = [
  'vless',
  'vmess',
  'trojan',
  'hysteria2',
  'shadowsocks',
  'anytls',
  'tuic',
  'naive',
  'snell',
  'socks',
  'http',
  'ssh',
  'wireguard',
  'tailscale',
  'hysteria',
  'tor',
  'openconnect',
  'openvpn-client',
  'masque-client',
  'tailcat',
  'custom',
];

/** 自定义协议的 outbound 是否合法形态（对象且含非空字符串 type）。语义不校验，交内核。 */
export function isValidCustomOutbound(outbound: unknown): boolean {
  return (
    !!outbound &&
    typeof outbound === 'object' &&
    !Array.isArray(outbound) &&
    typeof (outbound as Record<string, unknown>).type === 'string' &&
    ((outbound as Record<string, unknown>).type as string).trim().length > 0
  );
}

const KNOWN = new Set<string>(ALL_PROTOCOLS);

/**
 * **无地址协议**：没有 address/port（Tailscale 连控制面；Tor 内嵌客户端，传 server 内核 decode 失败；
 * Tailcat 由服务端公钥 + DERP 定位）。镜像 Rust `store/src/sanitize.rs` 的 `addressless`
 * （`server-completeness.test.ts` 读源码对拍）。消费方：完备性豁免、NodeDialog 隐藏地址行（设计 D8）。
 */
export const ADDRESSLESS_PROTOCOLS: readonly Protocol[] = ['tailscale', 'tor', 'tailcat'];
export function isAddresslessProtocol(protocol: string | undefined): boolean {
  return !!protocol && ADDRESSLESS_PROTOCOLS.includes(protocol.toLowerCase() as Protocol);
}

/** 32 字节的标准 base64 恒为 44 字符、恰一个 `=` 填充；反之亦然（同 Rust `tailcat_emit_check` 的 key_ok）。 */
const TAILCAT_KEY = /^[A-Za-z0-9+/]{43}=$/;

/**
 * Tailcat 设置能否下发 —— **逐条照 Rust `tailcat_emit_check`**（config-engine，生成侧剔节点 + store 落盘门
 * 共用那一个函数），返回同名 reason token，不合格就是 `null` 以外的值。两边必须同宽：这边宽了，节点保存后被
 * store 的 sanitize **静默丢掉**；窄了，用户存不进一个内核收得下的节点。
 *  - 服务端公钥 / disco 公钥必填，PSK / 私钥可空，四把都须 44 字符标准 base64（url-safe / hex / 无填充都是整核 FATAL）；
 *  - `derpRegion > 0`（整数）与 `derpServers` 非空恰好其一，servers 每项（串或对象的 `host`）非空。
 */
export function tailcatSettingsError(
  s: TailcatSettings | undefined
): 'tailcat-key-invalid' | 'tailcat-derp-invalid' | null {
  if (!s) return 'tailcat-key-invalid';
  const ok = (k: unknown) => typeof k === 'string' && TAILCAT_KEY.test(k);
  const optOk = (k: unknown) => k == null || k === '' || ok(k);
  if (!ok(s.serverPublicKey) || !ok(s.serverDiscoKey) || !optOk(s.preSharedKey) || !optOk(s.privateKey)) {
    return 'tailcat-key-invalid';
  }
  const region = typeof s.derpRegion === 'number' && Number.isInteger(s.derpRegion) && s.derpRegion > 0;
  const servers: unknown[] = Array.isArray(s.derpServers) ? s.derpServers : [];
  const hostsOk = servers.every((item) => {
    const host =
      typeof item === 'string'
        ? item
        : item && typeof item === 'object' && !Array.isArray(item)
          ? (item as Record<string, unknown>).host
          : undefined;
    return typeof host === 'string' && host !== '';
  });
  return region === (servers.length === 0) && hostsOk ? null : 'tailcat-derp-invalid';
}

/**
 * 返回该节点缺失协议必填项的英文错误信息；齐备返回 null。
 * 注意：仅校验「协议特有必填字段」，不含 address/port（那是通用校验，见 isServerComplete）。
 */
export function protocolRequirementError(server: ServerConfig): string | null {
  const p = server.protocol?.toLowerCase();
  switch (p) {
    case 'vless':
      return server.uuid?.trim() ? null : 'VLESS server requires uuid';
    case 'vmess':
      return server.uuid?.trim() ? null : 'VMess server requires uuid';
    case 'trojan':
      return server.password?.trim() ? null : 'Trojan server requires password';
    case 'hysteria2':
      return server.password?.trim() ? null : 'Hysteria2 server requires password';
    case 'anytls':
      return server.password?.trim() ? null : 'AnyTLS server requires password';
    case 'tuic':
      return server.uuid?.trim() && server.password?.trim()
        ? null
        : 'TUIC server requires uuid and password';
    case 'naive':
      return server.username?.trim() && server.password?.trim()
        ? null
        : 'Naive server requires username and password';
    case 'snell':
      // psk 进 password（同 trojan/hysteria2 惯例）；version 是主开关，仅 4|6 合法（server/port 由通用分支校验）。
      return server.password?.trim() &&
        (server.snellSettings?.version === 4 || server.snellSettings?.version === 6)
        ? null
        : 'Snell server requires psk and version (4 or 6)';
    case 'shadowsocks':
      return server.shadowsocksSettings?.method?.trim() &&
        server.shadowsocksSettings?.password?.trim()
        ? null
        : 'Shadowsocks server requires encryption method and password';
    case 'wireguard':
      return server.wireguardSettings?.privateKey?.trim() &&
        server.wireguardSettings?.peerPublicKey?.trim() &&
        server.wireguardSettings?.localAddress?.length
        ? null
        : 'WireGuard server requires privateKey, peerPublicKey and localAddress';
    case 'hysteria':
      return (server.hysteriaSettings?.authStr?.trim() || server.hysteriaSettings?.auth?.trim()) &&
        (server.hysteriaSettings?.upMbps ?? 0) > 0 &&
        (server.hysteriaSettings?.downMbps ?? 0) > 0
        ? null
        : 'Hysteria server requires auth, upMbps and downMbps';
    case 'tor':
      return null;
    case 'openconnect': {
      const settings = server.openconnectSettings;
      return settings?.server?.trim() && settings.username?.trim() && settings.password?.trim() && settings.flavor?.trim()
        ? null
        : 'OpenConnect server requires server, username, password and flavor';
    }
    case 'openvpn-client': {
      const settings = server.openvpnClientSettings;
      return settings?.server?.trim() && settings.server_port && settings.username?.trim() && settings.password?.trim() && settings.tls
        ? null
        : 'OpenVPN server requires server, port, username, password and tls';
    }
    case 'socks':
    case 'http':
    case 'ssh':
      return null; // 仅需 address/port（通用校验）
    case 'masque-client':
      // 地址/凭据/TLS 都在顶层：address/port 走通用校验；Basic 可选（服务端 users 为空时不校验）。
      return null;
    case 'tailcat': {
      const reason = tailcatSettingsError(server.tailcatSettings);
      return reason === 'tailcat-key-invalid'
        ? 'Tailcat server requires server public/disco keys (base64, 32 bytes)'
        : reason === 'tailcat-derp-invalid'
          ? 'Tailcat server requires exactly one of DERP region or DERP servers'
          : null;
    }
    case 'tailscale':
      return null; // 账号制：auth_key 可选（无则运行时交互登录），无硬必填项；亦无 address/port
    case 'custom':
      // raw-JSON 透传：必须是含 type 的 outbound 对象（语义/能否启用由内核 check 判，前端不校验）。
      return isValidCustomOutbound(server.customSettings?.outbound)
        ? null
        : 'Custom protocol requires a JSON outbound object with a "type" field';
    default:
      return `Unsupported protocol: ${p ?? '(empty)'}`;
  }
}

/**
 * 节点是否「配置齐备、可启动代理」——首页连接闸门 / 兜底出口可用性判据。
 * = 存在 + address/port 合法 + 协议必填齐备 + 协议受支持 + 组网节点可路由（非空 allowed_ips）。
 */
export function isServerComplete(server: ServerConfig | undefined | null): boolean {
  if (!server) return false;
  const p = server.protocol?.toLowerCase();
  if (!KNOWN.has(p as string)) return false;
  // 无地址协议（Tailscale / Tor / Tailcat）、custom（raw-JSON 自带 server/port）→ 无 ServerConfig address/port；其余必须有。
  if (!isAddresslessProtocol(p) && p !== 'custom') {
    if (!server.address || server.address.trim() === '') return false;
    if (!server.port || server.port <= 0) return false;
  }
  // 组网节点关外网且无可路由网段：字段虽齐备，但生成期不发射（空 allowed_ips=FATAL）→ 实际不可用。
  if (isMeshNodeUnroutable(server)) return false;
  return protocolRequirementError(server) === null;
}
