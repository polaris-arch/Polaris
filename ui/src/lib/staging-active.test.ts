import { readFileSync, readdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it } from 'vitest';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { getStagingActive } from '@/store/use-staging-active';
import { editRoute } from './staged-config';

const KEY = {
  ordinary: 'bypassLANList',
  bypassed: 'selectedServerId',
  exempt: 'ghProxyPrefix',
} as const;

describe('普通编辑的 native admission', () => {
  const appState = useAppStore.getState();
  const stagedState = useStagedConfigStore.getState();

  afterEach(() => {
    useAppStore.setState(appState);
    useStagedConfigStore.setState(stagedState);
  });

  function expectRoute(setting: string, expected: 'staged' | 'direct') {
    expect(editRoute(setting, getStagingActive())).toBe(expected);
  }

  it.each([
    { state: 'unknown', running: undefined, starting: false, stopping: false },
    { state: 'stopped', running: false, starting: false, stopping: false },
    { state: 'starting', running: false, starting: true, stopping: false },
    { state: 'running', running: true, starting: false, stopping: false },
    { state: 'stopping', running: false, starting: false, stopping: true },
  ])('普通编辑在 $state 时也交给 native admission', ({ running, starting, stopping }) => {
    useAppStore.setState({
      proxyStatus: running === undefined ? undefined : { running },
      proxyStarting: starting,
      proxyStopping: stopping,
    });
    useStagedConfigStore.setState({ enabled: true, entries: [] });

    expectRoute(KEY.ordinary, 'staged');
  });

  it('总开关关闭时普通编辑仍走兼容路径', () => {
    useAppStore.setState({ proxyStatus: { running: true } });
    useStagedConfigStore.setState({ enabled: false });
    expectRoute(KEY.ordinary, 'direct');
  });

  it('专用绕过键和豁免键保持直写', () => {
    useStagedConfigStore.setState({ enabled: true });
    expectRoute(KEY.bypassed, 'direct');
    expectRoute(KEY.exempt, 'direct');
  });
});

describe('接线：编辑入口共享同一个 native admission 开关', () => {
  const SRC = fileURLToPath(new URL('..', import.meta.url)); // ui/src/

  function sources(dir = SRC, out: string[] = []): string[] {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      const p = join(dir, e.name);
      if (e.isDirectory()) sources(p, out);
      else if (/\.(ts|tsx)$/.test(e.name) && !e.name.includes('.test.')) out.push(p);
    }
    return out;
  }
  const stripComments = (s: string): string =>
    s.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^[ \t]*\/\/.*$/gm, '');
  const relWith = (needle: string): string[] =>
    sources()
      .filter((p) => stripComments(readFileSync(p, 'utf8')).includes(needle))
      .map((p) => relative(SRC, p).replaceAll('\\', '/'))
      .sort();

  // 变异对照：把任一入口改回 `useStagedConfigStore((s) => s.enabled)` → 本条转红。
  it('读 store 原始开关的地方只剩 useStagingActive 这一处', () => {
    expect(relWith('(s) => s.enabled')).toEqual(['store/use-staging-active.ts']);
  });

  // 所有 editRoute / splitPatchByRoute 调用点都必须使用共享 hook 或在回调内重新取值。
  // 变异对照：新增一个编辑入口、自己取 `STAGED_CONFIG_ENABLED` 常量喂进去 → 本条转红。
  it('每个 editRoute / splitPatchByRoute 调用点都走共享开关', () => {
    const routers = new Set([...relWith('editRoute('), ...relWith('splitPatchByRoute(')]);
    // 判定本体与按键分流器自身不是「编辑入口」，它们收 boolean 实参。
    routers.delete('lib/staged-config.ts');
    routers.delete('components/screens/settings/config-patch-route.ts');
    // 同理：这两个 hook 文件的 `stagingEnabled` 由调用方（NodesScreen/RuleDialog，已持有
    // useStagingActive()）注入为参数，不在文件内部再取一次——判据没有第二个取值源，只是
    // 传递链多了一段（2026-08-30 随 5B/5C 拆分外提）。
    routers.delete('components/screens/nodes/use-node-actions.ts');
    routers.delete('components/dialogs/rule-submit.ts');
    const holders = new Set([...relWith('useStagingActive()'), ...relWith('getStagingActive()')]);
    expect([...routers].filter((f) => !holders.has(f))).toEqual([]);
  });
});
