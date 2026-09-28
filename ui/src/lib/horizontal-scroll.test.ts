import { describe, it, expect } from 'vitest';
import { horizontalPageTarget } from './horizontal-scroll';

describe('horizontal item paging (tabs and table columns)', () => {
  const tabs = Array.from({ length: 10 }, (_, i) => ({ start: i * 100, end: (i + 1) * 100 }));

  it('moves a viewport with one whole tab of overlap in either direction', () => {
    expect(horizontalPageTarget(0, 400, 1000, tabs, 1)).toBe(300);
    expect(horizontalPageTarget(300, 400, 1000, tabs, -1)).toBe(0);
  });

  it('uses actual variable tab widths and ignores clipped tabs after a trackpad scroll', () => {
    const varied = [{ start: 0, end: 120 }, { start: 123, end: 290 }, { start: 293, end: 390 }, { start: 393, end: 700 }];
    expect(horizontalPageTarget(40, 400, 1000, varied, 1)).toBe(293);
    expect(horizontalPageTarget(140, 400, 1000, varied, -1)).toBe(0);
  });

  it('clamps the final partial page and never scrolls outside the content', () => {
    expect(horizontalPageTarget(500, 400, 1000, tabs, 1)).toBe(600);
    expect(horizontalPageTarget(600, 400, 1000, tabs, 1)).toBe(600);
    expect(horizontalPageTarget(0, 400, 1000, tabs, -1)).toBe(0);
    expect(horizontalPageTarget(0, 1200, 1000, tabs, 1)).toBe(0);
    expect(horizontalPageTarget(0, 400, 0, [], 1)).toBe(0);
  });

  it('makes progress through a tab wider than the viewport without skipping its middle', () => {
    const wide = [{ start: 0, end: 1200 }];
    expect(horizontalPageTarget(0, 400, 1200, wide, 1)).toBe(400);
    expect(horizontalPageTarget(400, 400, 1200, wide, 1)).toBe(800);
    expect(horizontalPageTarget(800, 400, 1200, wide, -1)).toBe(400);
    expect(horizontalPageTarget(0, 100, 1000, tabs, 1)).toBe(100);
  });

  it('pages variable table columns, retaining a column before clamping to the final edge', () => {
    let offset = 0;
    const columns = [28, 54, 125, 110, 110, 110, 120, 72, 62, 129].map((width) => {
      const column = { start: offset, end: offset + width };
      offset += width;
      return column;
    });
    expect(horizontalPageTarget(0, 398, offset, columns, 1)).toBe(207);
    expect(horizontalPageTarget(207, 398, offset, columns, -1)).toBe(0);
    expect(horizontalPageTarget(0, 781, offset, columns, 1)).toBe(139);
    expect(horizontalPageTarget(139, 781, offset, columns, -1)).toBe(0);
  });
});
