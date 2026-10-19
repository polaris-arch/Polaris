import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const source = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');

describe('unlock IPC consumers retain event/disposal admission', () => {
  it('desktop hydration guards both get and nested run, including rejection cleanup', () => {
    const app = source('App.tsx');
    expect(app).toContain('!cancelled && useAppStore.getState().unlock === requestedState');
    expect(app).toContain('if (!isCurrent()) return; // A reset to empty is also a newer state.');
    expect(app).toContain('if (isCurrent()) useAppStore.getState().applyUnlockSnapshot(s)');
    expect(app).toContain('if (isCurrent()) useAppStore.getState().resetUnlock()');
    expect(app).toMatch(/onStopped\(\(\) => \{\s*useAppStore.getState\(\).resetUnlock\(\)/);
  });

  it('desktop manual return and errors retain the display and mount identity', () => {
    const home = source('components/screens/home/HomeScreen.tsx');
    expect(home).toContain('unlockRequestLifetime.current === lifetime');
    expect(home).toContain('useAppStore.getState().unlock === requestedState');
    expect(home).toContain('if (isCurrent()) applyUnlockSnapshot(snapshot)');
    expect(home).toMatch(/catch \(err\) \{\s*if \(!isCurrent\(\)\) return;/);
  });

  it('mobile return/cleanup and hydration retain admission after stop', () => {
    const home = source('mobile/home/MobileHomeScreen.tsx');
    expect(home).toContain('useAppStore.getState().unlock === requestedUnlockState');
    expect(home.match(/if \(ownsUnlockDisplay\(\)\)/g)).toHaveLength(2);
    expect(home).toContain('requestId && requested && ownsUnlockDisplay()');
    const wiring = source('mobile/app-wiring.ts');
    expect(wiring).toContain('unlockFeedRevision !== unlockHydrationRevision');
    expect(wiring).toMatch(/onStopped\(\(\) => \{\s*unlockFeedRevision \+= 1;\s*store\(\).resetUnlock\(\)/);
  });
});
