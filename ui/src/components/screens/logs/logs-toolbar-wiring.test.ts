import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const read = (relative: string): string =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8');

const SCREEN = read('./LogsScreen.tsx');
const MOBILE_SCREEN = read('../../../mobile/connections/MobileConnectionsScreen.tsx');

describe('日志级别直接保存回执', () => {
  it.each([
    { platform: '桌面', source: SCREEN, level: 'v' },
    { platform: '移动端', source: MOBILE_SCREEN, level: 'next' },
  ])('$platform 写盘成功后才提示保存了哪个级别', ({ source, level }) => {
    const start = source.indexOf('const onLevelChange = useCallback');
    const end = source.indexOf('\n  );', start);
    expect(start).toBeGreaterThan(-1);
    expect(end).toBeGreaterThan(start);
    const body = source.slice(start, end);
    const write = `await saveConfig({ logLevel: ${level} });`;
    expect(body).toMatch(/stage\(\{[\s\S]*?\}\);\s+return;/);
    expect(body.match(/toast\.success\(/g)).toHaveLength(1);
    const feedback = body.split(`${write}\n        `)[1]?.split('\n')[0];
    expect(feedback).toMatch(/^toast\.success\(/);
    expect(feedback).toContain("('logs.currentLevel')");
    expect(feedback).toContain(`${level}.toUpperCase()`);
    expect(feedback).toContain("('common.saved')");
  });
});

describe('会话诊断模式接线', () => {
  it('诊断切换不经过持久配置或暂存层', () => {
    const start = SCREEN.indexOf('const onToggleDiagnostic = useCallback');
    const end = SCREEN.indexOf('\n  }, [diagnosticBusy', start);
    expect(start).toBeGreaterThan(-1);
    expect(end).toBeGreaterThan(start);
    const body = SCREEN.slice(start, end);
    expect(body).not.toContain('saveConfig');
    expect(body).not.toContain('stage(');
    expect(body).toContain("toast.info(t(enabled ? 'logs.diagnosticEnabled' : 'logs.diagnosticDisabled'))");
    expect(body.indexOf('await api.logs.setDiagnostic')).toBeLessThan(body.indexOf('toast.info'));
  });
});
