import { afterEach, describe, expect, it, vi } from 'vitest';
import { computeCselPosition } from './csel-logic';
import { getControlBoundary, type OverlayBox } from '@/lib/overlay-position';

const vp = { width: 1000, height: 800 };
const card = { left: 180, top: 80, width: 700, height: 580 };
const trigger = { left: 760, top: 220, bottom: 254, width: 100 };
function contained(p: ReturnType<typeof computeCselPosition>, box: OverlayBox) {
  expect(p.left).toBeGreaterThanOrEqual(box.left);
  expect(p.left + p.width).toBeLessThanOrEqual(box.left + box.width);
  expect(p.top).toBeGreaterThanOrEqual(box.top);
  expect(p.top + p.maxHeight).toBeLessThanOrEqual(box.top + box.height);
}

describe('shared select content boundaries', () => {
  it('uses actual menu width at the card right edge, not the narrower trigger or window', () => {
    const p = computeCselPosition(trigger, 100, vp, 300, card);
    expect(p.left).toBe(572);
    expect(p.width).toBe(300);
    contained(p, card);
  });
  it('flips long lists above the trigger and constrains their scroll height', () => {
    const p = computeCselPosition({ ...trigger, top: 430, bottom: 464 }, 500, vp, 300, card);
    expect(p.flipped).toBe(true);
    expect(p.maxHeight).toBe(337);
    contained(p, card);
  });
  it('shrinks below rather than flipping into less available space', () => {
    const p = computeCselPosition(trigger, 500, vp, 300, card);
    expect(p.flipped).toBe(false);
    expect(p.maxHeight).toBe(393);
    contained(p, card);
  });
  it('bounds minimum width on narrow cards and intersects a partially offscreen card', () => {
    const narrow = { left: 900, top: 80, width: 240, height: 580 };
    const p = computeCselPosition({ ...trigger, left: 910, width: 200 }, 100, vp, 320, narrow);
    expect(p.width).toBe(84);
    contained(p, { ...narrow, width: 100 });
  });
  it('keeps shallow cards selectable by shifting over the trigger inside the card', () => {
    const shallow = { left: 180, top: 200, width: 700, height: 74 };
    const p = computeCselPosition(trigger, 300, vp, 300, shallow);
    expect(p.maxHeight).toBe(58);
    contained(p, shallow);
  });
  it('does not place an unreachable sliver for tiny or empty visible intersections', () => {
    for (const boundary of [
      { left: 900, top: 80, width: 15, height: 580 },
      { left: 180, top: 790, width: 700, height: 580 },
      { left: 1000, top: 80, width: 240, height: 580 },
    ]) {
      const p = computeCselPosition(trigger, 300, vp, 300, boundary);
      expect(p.usable).toBe(false);
      expect(p.width).toBe(0);
      expect(p.maxHeight).toBe(0);
      expect(p.left).toBe(boundary.left);
    }
  });
  it('falls back to viewport and aligns the RTL inline start before clamping', () => {
    const p = computeCselPosition({ ...trigger, left: 400 }, 100, vp, 300, undefined, true);
    expect(p.left).toBe(200);
    contained(p, { left: 0, top: 0, ...vp });
  });
});

afterEach(() => vi.unstubAllGlobals());
it('resolves the nearest semantic content boundary, includes padding, and intersects dialog/viewport', () => {
  const element = (left: number, right: number) => ({ getBoundingClientRect: () => ({ left, right, top: 50, bottom: 700 }) });
  const nearest = element(100, 950);
  const dialog = element(150, 900);
  const body = element(200, 800);
  vi.stubGlobal('getComputedStyle', () => ({ borderLeftWidth: '1px', borderRightWidth: '1px', borderTopWidth: '1px', borderBottomWidth: '1px', paddingLeft: '19px', paddingRight: '19px', paddingTop: '9px', paddingBottom: '9px' }));
  const closest = vi.fn((selector: string) => selector === 'dialog' ? dialog : selector === '.dlg-body' ? body : nearest);
  expect(getControlBoundary({ closest } as unknown as HTMLElement, { left: 0, top: 0, ...vp })).toEqual({ left: 220, top: 60, width: 560, height: 630 });
  expect(closest).toHaveBeenCalledWith('[data-overlay-boundary], .card, .dlg-body, dialog');
  expect(getControlBoundary({ closest: () => null } as unknown as HTMLElement, { left: 0, top: 0, ...vp })).toEqual({ left: 0, top: 0, ...vp });
});

it('portalled select escapes a shallow ordinary card vertically, not horizontally', () => {
  vi.stubGlobal('getComputedStyle', () => ({}));
  const card = { getBoundingClientRect: () => ({ left: 100, right: 900, top: 500, bottom: 620 }),
    matches: (selector: string) => selector === '.card' };
  const closest = (selector: string) => selector.includes('[data-overlay-boundary]') ? card : null;
  const boundary = getControlBoundary({ closest } as unknown as HTMLElement, { left: 0, top: 0, ...vp }, false);
  expect(boundary).toEqual({ left: 100, top: 0, width: 800, height: 800 });
  for (const rtl of [false, true]) {
    const p = computeCselPosition({ left: 650, top: 570, bottom: 610, width: 230 }, 300, vp, 320, boundary, rtl);
    expect(p.maxHeight).toBe(300); expect(p.flipped).toBe(true); contained(p, boundary);
  }
});
it('select keeps explicit boundary and dialog containment despite card escape', () => {
  vi.stubGlobal('getComputedStyle', () => ({}));
  const card = { getBoundingClientRect: () => ({ left: 100, right: 900, top: 500, bottom: 620 }), matches: () => true };
  const closest = (selector: string) => selector.includes('[data-overlay-boundary]') ? card : null;
  expect(getControlBoundary({ closest } as unknown as HTMLElement, { left: 0, top: 0, ...vp }, false))
    .toEqual({ left: 100, top: 500, width: 800, height: 120 });
});

it('ordinary card escape still intersects a distinct outer explicit overlay boundary', () => {
  vi.stubGlobal('getComputedStyle', () => ({}));
  const card = { getBoundingClientRect: () => ({ left: 100, right: 900, top: 500, bottom: 620 }), matches: (s: string) => s === '.card' };
  const explicit = { getBoundingClientRect: () => ({ left: 50, right: 950, top: 250, bottom: 700 }), matches: () => false };
  const closest = (s: string) => s === '[data-overlay-boundary]' ? explicit : s.includes('.card') ? card : null;
  expect(getControlBoundary({ closest } as unknown as HTMLElement, { left: 0, top: 0, ...vp }, false))
    .toEqual({ left: 100, top: 250, width: 800, height: 450 });
});
