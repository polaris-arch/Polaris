import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const read = (relative: string): string =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8');

const SCREEN = read('./LogsScreen.tsx');

describe('会话诊断模式接线', () => {
  it('诊断切换不经过持久配置或暂存层', () => {
    const start = SCREEN.indexOf('const onToggleDiagnostic = useCallback');
    const end = SCREEN.indexOf('\n  }, [diagnosticBusy', start);
    expect(start).toBeGreaterThan(-1);
    expect(end).toBeGreaterThan(start);
    const body = SCREEN.slice(start, end);
    expect(body).not.toContain('saveConfig');
    expect(body).not.toContain('stage(');
  });
});
