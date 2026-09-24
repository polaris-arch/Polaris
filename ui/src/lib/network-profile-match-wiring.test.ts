/**
 * 网络场景命中态（N4）接线守卫：后端只发**无载荷**变更信号，渲染端必须据此重拉
 * `resolvedSources()`（`matched` 是真值）。缺了订阅或重拉依赖，圆点就停在打开面板那一刻 —— 纯逻辑单测
 * 全绿也看不出来（本仓没有 React 渲染测试设施），故钉源码结构。
 *
 * 另钉事件名两侧逐字一致：`ipc-channels.ts` 与 Rust `events.rs` 各写一份字符串，漂了订阅静默收不到。
 */
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { IPC_CHANNELS } from '@/domain/ipc-channels';

const read = (rel: string) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
/** 去注释（注释里提到函数名不算接线）；`[^:]` 前瞻避免把 `https://` 当行注释切掉。 */
const code = (src: string) => src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1');

describe('网络场景命中态：信号 → 重拉', () => {
  const panel = code(read('../components/screens/rules/NetworkProfilePanel.tsx'));
  const start = panel.indexOf('export function useResolvedProbes(');
  const end = panel.indexOf('\n}\n', start);
  const hook = panel.slice(start, end);

  it('切片自检：截到了 useResolvedProbes 且确实在拉 resolvedSources', () => {
    expect(start).toBeGreaterThanOrEqual(0);
    expect(hook).toContain('.resolvedSources()');
  });

  it('订阅 onMatchChanged，且信号计数进了重拉 effect 的依赖', () => {
    const sub = hook.match(/onMatchChanged\(\s*\(\)\s*=>\s*(\w+)\(/);
    expect(sub, 'useResolvedProbes 必须订阅 api.networkProfile.onMatchChanged').not.toBeNull();
    const setter = sub![1];
    const tick = hook.match(new RegExp(`const \\[(\\w+),\\s*${setter}\\]`));
    expect(tick, `找不到 ${setter} 对应的状态`).not.toBeNull();
    const deps = [...hook.matchAll(/\},\s*\[([^\]]*)\]\);/g)].map((m) => m[1]);
    expect(
      deps.some((d) => d.split(',').map((s) => s.trim()).includes(tick![1])),
      `重拉 effect 的依赖里没有 ${tick![1]}：信号来了也不重拉`,
    ).toBe(true);
  });

  it('事件名与 Rust events.rs 逐字一致', () => {
    const rust = read('../../../src-tauri/src/events.rs');
    const m = rust.match(/EVENT_NETWORK_PROFILE_MATCH_CHANGED:\s*&str\s*=\s*"([^"]+)"/);
    expect(m, 'events.rs 缺 EVENT_NETWORK_PROFILE_MATCH_CHANGED').not.toBeNull();
    expect(IPC_CHANNELS.EVENT_NETWORK_PROFILE_MATCH_CHANGED).toBe(m![1]);
  });
});
