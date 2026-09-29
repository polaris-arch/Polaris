import { describe, expect, it, vi } from 'vitest';
import { recoveryText } from './recovery-text';

describe('renderer recovery text', () => {
  it('follows a language change after the recovery module was loaded', () => {
    let language = 'en-US';
    vi.stubGlobal('localStorage', { getItem: () => language });
    try {
      expect(recoveryText('report')).toBe('Export bug report (Debug)');
      language = 'zh-CN';
      expect(recoveryText('report')).toBe('导出故障报告（Debug）');
    } finally {
      vi.unstubAllGlobals();
    }
  });
  it('不依赖主 i18next 初始化即可从辅助 locale 读取完整逃生文案', () => {
    for (const id of ['title', 'body', 'reload', 'report', 'reportBusy', 'reportHint', 'reportDone', 'reportFailed'] as const) {
      const text = recoveryText(id);
      expect(text).not.toContain('native.fatalPage');
      expect(text.length).toBeGreaterThan(2);
    }
  });
});
