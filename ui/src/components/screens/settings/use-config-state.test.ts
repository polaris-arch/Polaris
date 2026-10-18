import { describe, expect, it } from 'vitest';
import type { UserConfig } from '@/contracts/types';
import { acceptConfig, beginDirect, commitDirect, configEditState, directKeys, displayConfig, failDirect } from './use-config-state';

const disk = { mixedPort: 7890, logLevel: 'info', desktopNotifications: true, servers: [] } as unknown as UserConfig;
describe('instance-local direct intention over an authoritative baseline', () => {
  it('keeps raw seed and zero-overlay publication references, including a null cold seed', () => {
    expect(displayConfig(configEditState(null))).toBeNull();
    expect(displayConfig(configEditState(disk))).toBe(disk);
    const next = { ...disk, mixedPort: 9000 };
    expect(displayConfig(acceptConfig(configEditState(disk), next, 1, 1))).toBe(next);
  });

  it('A and B can edit the same key independently while sharing the exact baseline', () => {
    const a = beginDirect(configEditState(disk), 1, { mixedPort: 8000 });
    const b = beginDirect(configEditState(disk), 1, { mixedPort: 9000 });
    expect(a.baseline).toBe(b.baseline);
    expect(displayConfig(a)?.mixedPort).toBe(8000); expect(displayConfig(b)?.mixedPort).toBe(9000);
    expect(disk.mixedPort).toBe(7890);
  });

  it('new external fields update beneath pending direct keys', () => {
    const state = beginDirect(configEditState(disk), 1, { mixedPort: 8000 });
    const external = { ...disk, mixedPort: 9000, logLevel: 'debug' } as UserConfig;
    const next = acceptConfig(state, external, 2, 2);
    expect(displayConfig(next)).toMatchObject({ mixedPort: 8000, logLevel: 'debug' });
    expect(next.baseline).toBe(external);
  });

  it.each([[true, true], [true, false], [false, true], [false, false]])('P1 success=%s/P2 success=%s retires only the corresponding operations', (one, two) => {
    let state = beginDirect(beginDirect(configEditState(disk), 1, { mixedPort: 8000 }), 2, { logLevel: 'debug' });
    state = one ? commitDirect(state, 1, { ...disk, mixedPort: 8000 }, 5) : failDirect(state, 1);
    expect(displayConfig(state)?.logLevel).toBe('debug');
    state = two ? commitDirect(state, 2, { ...disk, logLevel: 'debug' } as UserConfig, 6) : failDirect(state, 2);
    expect(displayConfig(state)?.mixedPort).toBe(one ? 8000 : 7890);
    expect(displayConfig(state)?.logLevel).toBe(two ? 'debug' : 'info');
  });

  it('late P1 success and late P1 failure never overwrite P2 same-key intention', () => {
    const state = beginDirect(beginDirect(configEditState(disk), 1, { mixedPort: 8000 }), 2, { mixedPort: 9000 });
    expect(displayConfig(commitDirect(state, 1, { ...disk, mixedPort: 8000 }, 4))?.mixedPort).toBe(9000);
    expect(displayConfig(failDirect(state, 1))?.mixedPort).toBe(9000);
  });

  it('a stale full write reply contributes only its direct keys, then fresh disk truth retires it', () => {
    let state = beginDirect(configEditState(disk), 1, { mixedPort: 8000 });
    const external = { ...disk, mixedPort: 9100, logLevel: 'debug' } as UserConfig;
    state = acceptConfig(state, external, 2, 4);
    state = commitDirect(state, 1, { ...disk, mixedPort: 8000 }, 5);
    expect(displayConfig(state)).toMatchObject({ mixedPort: 8000, logLevel: 'debug' });
    state = acceptConfig(state, external, 3, 4);
    expect(displayConfig(state)?.mixedPort).toBe(8000); // pre-barrier GET cannot retire it
    state = acceptConfig(state, external, 4, 5);
    expect(displayConfig(state)).toBe(external); expect(state.operations).toEqual([]);
  });

  it('old failure rolls back only that op, preserving external state and newer committed values', () => {
    let state = beginDirect(beginDirect(configEditState(disk), 1, { mixedPort: 8000 }), 2, { logLevel: 'debug' });
    state = commitDirect(state, 2, { ...disk, logLevel: 'debug' } as UserConfig, 8);
    state = acceptConfig(state, { ...disk, desktopNotifications: false }, 2, 3);
    state = failDirect(state, 1);
    expect(displayConfig(state)).toMatchObject({ mixedPort: 7890, logLevel: 'debug', desktopNotifications: false });
  });

  it('long read outages bound committed history per key, preserve pending order and largest barrier', () => {
    let state = configEditState(disk);
    for (let id = 1; id <= 300; id++) {
      state = beginDirect(state, id, { mixedPort: 8000 + id });
      state = commitDirect(state, id, { ...disk, mixedPort: 8000 + id }, id);
    }
    expect(state.operations).toHaveLength(1); expect(displayConfig(state)?.mixedPort).toBe(8300);
    state = beginDirect(state, 301, { mixedPort: 9000 });
    state = acceptConfig(state, disk, 2, 300);
    expect(state.operations).toHaveLength(1); expect(displayConfig(state)?.mixedPort).toBe(9000);
    expect(directKeys(state)).toEqual(new Set(['mixedPort']));
  });

  it('compaction cannot put an older committed key above a later pending operation', () => {
    let state = beginDirect(beginDirect(configEditState(disk), 1, { mixedPort: 8000 }), 2, { mixedPort: 9000 });
    state = commitDirect(state, 1, { ...disk, mixedPort: 8000 }, 10);
    state = beginDirect(state, 3, { logLevel: 'debug' });
    state = commitDirect(state, 3, { ...disk, logLevel: 'debug' } as UserConfig, 11);
    expect(displayConfig(state)).toMatchObject({ mixedPort: 9000, logLevel: 'debug' });
  });

  it('rejects older publication serials without erasing newer baseline or intent', () => {
    const state = acceptConfig(configEditState(disk), { ...disk, mixedPort: 9000 }, 10, 8);
    expect(acceptConfig(state, disk, 9, 20)).toBe(state);
  });
});
