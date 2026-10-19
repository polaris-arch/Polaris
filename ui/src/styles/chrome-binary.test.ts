import { describe, expect, it, vi } from 'vitest';

const fs = vi.hoisted(() => ({ existsSync: vi.fn(), statSync: vi.fn() }));
vi.mock('node:fs', () => fs);
import { resolveChromeBinary } from './chrome-binary.test-support';

describe('shared browser location without import-time probing', () => {
  it('does not probe fixed paths on import; an explicit executable can bypass discovery', () => {
    expect(fs.existsSync).not.toHaveBeenCalled();
    const override: string | undefined = '/explicit/browser';
    expect(override ?? resolveChromeBinary()).toBe('/explicit/browser');
    expect(fs.existsSync).not.toHaveBeenCalled();
  });
  it('keeps ordered existing-file discovery', () => {
    fs.existsSync.mockImplementation((path: string) => path !== '/missing');
    fs.statSync.mockImplementation((path: string) => ({ isFile: () => path === '/browser' }));
    expect(resolveChromeBinary(['/missing', '/directory', '/browser', '/later'])).toBe('/browser');
    expect(fs.existsSync.mock.calls.map(([path]) => path)).toEqual(['/missing', '/directory', '/browser']);
  });
  it('fails explicitly when no candidate is a file', () => {
    fs.existsSync.mockReturnValue(false);
    expect(() => resolveChromeBinary(['/missing'])).toThrow('不 skip');
  });
});
