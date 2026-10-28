/**
 * 托盘浮层 WebKit 保真验证 harness（仅本机验证用，不进产物）。
 * mockIPC 喂 demo 数据 → 渲染真实 <TrayMenu>（含 tray-overlay.css）。
 * 与 src/tray/main.tsx 对齐（applyOs/applyTrayTheme + index.css + tray-overlay.css），
 * 但 os/theme 由 URL query 决定，便于对拍 mac 透明窗形态。
 */
import React from 'react';
import ReactDOM from 'react-dom/client';
import { mockIPC } from '@tauri-apps/api/mocks';
import { emit } from '@tauri-apps/api/event';
import { DEMO_CONFIG as BASE_CONFIG } from './harness-fixture';
import type { ServerConfig, UserConfig } from './src/contracts/types';
import TrayMenu from './src/tray/TrayMenu';
import { applyTrayTheme } from './src/tray/tray-theme';
import './src/styles/index.css';
import './src/tray/tray-overlay.css';

const params = new URLSearchParams(location.search);
const os = (params.get('os') ?? 'mac') as 'mac' | 'win' | 'lin';
const theme = params.get('theme') as 'light' | 'dark' | undefined;
// 节点数量可调（验证多项时的窗高/截断）。默认给出足够多的节点以触发主视图较高内容。
const nNodes = Number(params.get('nodes') ?? '3');

const DEMO_SERVERS: ServerConfig[] = Array.from({ length: nNodes }, (_, i) => ({
  id: `s${i + 1}`,
  name: i === 0 ? '香港 IEPL · 01' : `节点 ${String(i + 1).padStart(2, '0')}`,
  protocol: i % 2 === 0 ? 'vless' : 'trojan',
  address: `n${i + 1}.example.net`,
  port: 443,
  uuid: `demo-uuid-${i + 1}`,
  subscriptionId: 'sub1',
}));

// 托盘「节点·最近」MRU 验证用假历史：最近优先，最多 3 条（对齐 server_switch 的 push_recent_server_id
// 上限 3）；?recent=0 关闭（验证「无历史」时退回旧「节点」标题的分支）。默认取前 3 个 demo 节点倒序，
// 与当前选中 s1 拼接后 TrayMenu.recentItems 恰好渲染 3 项（current + 2 history）。
const recentCount = Number(params.get('recent') ?? Math.min(nNodes, 3));
const recentServerIds = DEMO_SERVERS.slice(0, recentCount)
  .map((s) => s.id)
  .reverse();

let demoConfig: UserConfig = {
  ...structuredClone(BASE_CONFIG),
  servers: DEMO_SERVERS,
  selectedServerId: 's1',
  recentServerIds,
  proxyMode: 'smart',
  proxyModeType: 'tun',
  uiTheme: theme ?? 'system',
  language: 'zh-CN',
};

// Fixture transport only: never resizes a native window or invokes real backend actions.
const control = {
  reportedHeights: [] as number[],
  async setRecent(count: number) {
    demoConfig = { ...demoConfig, recentServerIds: DEMO_SERVERS.slice(0, count).map(s => s.id).reverse() };
    await emit('event:configChanged', structuredClone(demoConfig));
  },
};
declare global {
  interface Window {
    __trayHeightHarness: typeof control;
    __trayReportedHeight: number;
  }
}
window.__trayHeightHarness = control;

mockIPC((cmd, payload) => {
  switch (cmd) {
    case 'config_get': return Promise.resolve({ success: true, data: structuredClone(demoConfig) });
    case 'proxy_get_status': return Promise.resolve({ running: false });
    case 'ipinfo_get': return Promise.resolve({ proxy: null, direct: null });
    case 'server_get_all': return Promise.resolve(DEMO_SERVERS);
    case 'plugin:os|platform': return Promise.resolve(os === 'mac' ? 'macos' : os === 'win' ? 'windows' : 'linux');
    case 'tray_resize': {
      // Record the renderer request; Playwright separately applies the native 80..700/720 clamp
      // to its viewport. This mock itself does NOT resize a browser or native window.
      const h = (payload as { height?: number })?.height ?? 0;
      window.__trayReportedHeight = h;
      control.reportedHeights.push(h);
      return Promise.resolve(null);
    }
    default: return Promise.resolve(null);
  }
}, { shouldMockEvents: true });

document.documentElement.setAttribute('data-os', os);
applyTrayTheme(theme);

// 验证专用：透明 body 后面铺一层「桌面」底色，好让卡片 + 顶部三角在截图里可辨。
// z-index:-1 落在 body::before/after(z:1) 与 #tray-root 之下；不影响布局/尺寸。
const backdrop = document.createElement('div');
backdrop.style.cssText = 'position:fixed;inset:0;z-index:-1;background:#3a3f4b';
document.body.appendChild(backdrop);

ReactDOM.createRoot(document.getElementById('tray-root')!).render(
  <React.StrictMode>
    <TrayMenu />
  </React.StrictMode>,
);
