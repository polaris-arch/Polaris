import { afterEach, describe, expect, it, vi } from 'vitest';

const fs = vi.hoisted(() => ({ existsSync: vi.fn(), statSync: vi.fn() }));
vi.mock('node:fs', () => fs);
import { chromeCandidates, resolveChromeBinary } from './chrome-binary.test-support';

afterEach(() => vi.unstubAllEnvs());

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
  it('finds installed Windows Chrome or Edge under standard OS roots', () => {
    vi.stubEnv('ProgramFiles', 'D:\\Programs');
    vi.stubEnv('ProgramFiles(x86)', 'D:\\Programs (x86)');
    vi.stubEnv('LOCALAPPDATA', 'D:\\Users\\tester\\AppData\\Local');
    const candidates = chromeCandidates('win32');
    const edge = 'D:\\Programs (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
    expect(candidates).toContain('D:\\Programs\\Google\\Chrome\\Application\\chrome.exe');
    expect(candidates).toContain('D:\\Users\\tester\\AppData\\Local\\Google\\Chrome\\Application\\chrome.exe');
    fs.existsSync.mockImplementation((path: string) => path === edge);
    fs.statSync.mockReturnValue({ isFile: () => true });
    expect(resolveChromeBinary(candidates)).toBe(edge);
    fs.existsSync.mockReturnValue(false);
    expect(() => resolveChromeBinary(candidates)).toThrow('不 skip');
  });
});
