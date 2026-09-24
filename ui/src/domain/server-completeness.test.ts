import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { ServerConfig } from '@/contracts/types';
import type { TailcatSettings } from '@/contracts/types/protocol-settings';
import {
  ADDRESSLESS_PROTOCOLS,
  ALL_PROTOCOLS,
  isAddresslessProtocol,
  isServerComplete,
  protocolRequirementError,
  tailcatSettingsError,
} from './server-completeness';

const base = (protocol: ServerConfig['protocol']): ServerConfig => ({
  id: protocol,
  name: protocol,
  protocol,
  address: 'vpn.example.com',
  port: 443,
});

describe('server completeness endpoint VPN coverage', () => {
  it('runtime registry covers every Protocol member added for endpoint VPNs', () => {
    for (const protocol of ['hysteria', 'tor', 'openconnect', 'openvpn-client', 'masque-client'] as const) {
      expect(ALL_PROTOCOLS).toContain(protocol);
    }
  });

  it('Tor is addressless, while endpoint VPNs require their nested settings', () => {
    expect(isServerComplete({ ...base('tor'), address: '', port: 0 })).toBe(true);
    expect(isServerComplete({
      ...base('openconnect'),
      openconnectSettings: { server: 'vpn.example.com:443', username: 'u', password: 'p', flavor: 'anyconnect' },
    })).toBe(true);
    expect(isServerComplete({
      ...base('openvpn-client'),
      openvpnClientSettings: { server: 'vpn.example.com', server_port: 1194, username: 'u', password: 'p', tls: {} },
    })).toBe(true);
    expect(isServerComplete(base('openvpn-client'))).toBe(false);
  });

  it('MASQUE 只要地址/端口（凭据可选、设置块可缺）；缺地址不完整', () => {
    expect(isServerComplete(base('masque-client'))).toBe(true);
    expect(isServerComplete({ ...base('masque-client'), address: '' })).toBe(false);
  });
});

// ── Tailcat（T2）──
const K1 = 'dinJxIQiMsfg+X5vvV6QhuPIaxT4C1Buurk/GDTskCw=';
const K2 = 'zajBCXWDxF7WZrnWNJ0Y4T91BEcb2ZnVjbOK2s6cFHo=';
const tc = (over: Partial<TailcatSettings>): TailcatSettings => ({
  serverPublicKey: K1,
  serverDiscoKey: K2,
  derpRegion: 1,
  ...over,
});

describe('Tailcat 完备性：逐条镜像 Rust tailcat_emit_check', () => {
  it('合法形态：region 模式 / servers 模式（串与对象混合）/ 可选 key 空串', () => {
    expect(tailcatSettingsError(tc({}))).toBeNull();
    expect(tailcatSettingsError(tc({ derpRegion: undefined, derpServers: ['d.example', { host: 'e.example' }] }))).toBeNull();
    expect(tailcatSettingsError(tc({ preSharedKey: '', privateKey: K1 }))).toBeNull();
  });

  it('key：缺失 / hex / url-safe / 无填充 / 首尾空白 → tailcat-key-invalid', () => {
    const hex = 'e851d9f41f9826b95d2d18325d0c27ba124384f9f152d1e81ceab402df2c9874';
    for (const bad of [undefined, '', hex, K1.replace('+', '-'), K1.slice(0, 43), ` ${K1}`]) {
      expect(tailcatSettingsError(tc({ serverPublicKey: bad })), String(bad)).toBe('tailcat-key-invalid');
    }
    expect(tailcatSettingsError(tc({ serverDiscoKey: undefined }))).toBe('tailcat-key-invalid');
    expect(tailcatSettingsError(tc({ preSharedKey: hex }))).toBe('tailcat-key-invalid');
    expect(tailcatSettingsError(tc({ privateKey: K1.slice(1) }))).toBe('tailcat-key-invalid');
    expect(tailcatSettingsError(undefined)).toBe('tailcat-key-invalid');
  });

  it('DERP：都没设 / 两者都设 / region≤0 或非整数 / servers 项缺 host → tailcat-derp-invalid', () => {
    for (const over of [
      { derpRegion: undefined },
      { derpServers: ['d.example'] },
      { derpRegion: 0 },
      { derpRegion: -2 },
      { derpRegion: 1.5 },
      { derpRegion: undefined, derpServers: [{ ipv4: '192.0.2.1' }] },
      { derpRegion: undefined, derpServers: [''] },
    ] as Partial<TailcatSettings>[]) {
      expect(tailcatSettingsError(tc(over)), JSON.stringify(over)).toBe('tailcat-derp-invalid');
    }
  });

  it('Tailcat 无地址：空地址 + 0 端口完备；设置不合格则不完备', () => {
    const node = { ...base('tailcat'), address: '', port: 0, tailcatSettings: tc({}) } as ServerConfig;
    expect(isServerComplete(node)).toBe(true);
    expect(protocolRequirementError({ ...node, tailcatSettings: tc({ derpRegion: 0 }) })).toMatch(/DERP/);
    expect(isServerComplete({ ...node, tailcatSettings: undefined })).toBe(false);
  });

  it('无地址名单 ⟺ Rust store/src/sanitize.rs 的 `addressless`（跨语言对拍）', () => {
    const src = readFileSync(fileURLToPath(new URL('../../../crates/store/src/sanitize.rs', import.meta.url)), 'utf8');
    const m = /let addressless = matches!\(proto_lower\.as_str\(\),([^)]*)\);/.exec(src);
    expect(m, 'sanitize.rs 的 addressless 判据解析失败 —— 解析不到必须转红').not.toBeNull();
    const rust = [...(m as RegExpExecArray)[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]).sort();
    expect(rust.length).toBeGreaterThanOrEqual(3);
    expect([...ADDRESSLESS_PROTOCOLS].sort()).toEqual(rust);
    expect(isAddresslessProtocol('Tailcat')).toBe(true);
    expect(isAddresslessProtocol('masque-client')).toBe(false);
  });
});
