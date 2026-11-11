import { describe, expect, it } from 'vitest';
import { cycleSortState, type SortState } from './sort-cycle';

type Key = 'host' | 'rate';

describe('连接表排序三态', () => {
  it('同列按默认、升序、降序、默认循环', () => {
    const asc = cycleSortState<Key>(null, 'host');
    const desc = cycleSortState(asc, 'host');
    const restored = cycleSortState(desc, 'host');

    expect(asc).toEqual({ key: 'host', dir: 1 });
    expect(desc).toEqual({ key: 'host', dir: -1 });
    expect(restored).toBeNull();
  });

  it('换列总是从升序开始', () => {
    const current: SortState<Key> = { key: 'host', dir: -1 };
    expect(cycleSortState(current, 'rate')).toEqual({ key: 'rate', dir: 1 });
  });
});
