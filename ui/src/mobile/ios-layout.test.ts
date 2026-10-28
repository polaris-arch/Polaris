import { describe, expect, it, vi } from 'vitest';
import { avoidReserved, intersectsReserved, iosRegionsInCss, readIosViewport, refreshNativeViewportCss, type LayoutRect, type ReservedRegion } from './ios-layout';

const bounds: LayoutRect = { x: 0, y: 0, width: 800, height: 600 };
const region = (over: Partial<ReservedRegion> = {}): ReservedRegion => ({ kind: 'division', x: 400, y: 0, width: 0, height: 600, ...over });

describe('UIKit points and reserved geometry', () => {
  it('converts points once, preserving zero division and visual viewport offsets', () => {
    const value = readIosViewport({ width: 400, height: 800, verticalBarSide: 'left', reservedRegions: [region({ x: 200, height: 800 })] })!;
    expect(iosRegionsInCss(value, 800, { x: 35, y: 80, width: 400, height: 600, scale: 2 })).toEqual([
      { kind: 'division', x: 235, y: 80, width: 0, height: 800 },
    ]);
    expect(value.verticalBarSide).toBe('left');
  });

  it('rotation uses the new native width without reusing the previous point scale', () => {
    const value = readIosViewport({ width: 800, height: 400, reservedRegions: [region({ kind: 'occlusion', x: 300, y: 200, width: 20, height: 30 })] })!;
    expect(iosRegionsInCss(value, 800, { ...bounds, scale: 1 })).toEqual([
      { kind: 'occlusion', x: 300, y: 200, width: 20, height: 30 },
    ]);
  });

  it('accepts only actual finite region shapes, retaining a horizontal zero-thickness division', () => {
    const value = readIosViewport({ width: 800, height: 600, reservedRegions: [
      region({ x: 0, y: 300, width: 800, height: 0 }),
      region({ kind: 'occlusion', width: 0 }), region({ x: NaN }), region({ height: -1 }),
      { kind: 'guessedHinge', x: 10, y: 10, width: 10, height: 10 },
    ] })!;
    expect(value.reservedRegions).toEqual([region({ x: 0, y: 300, width: 800, height: 0 })]);
    expect(readIosViewport({ width: 0, height: 600, reservedRegions: [] })).toBeNull();
    expect(readIosViewport({ width: 800, height: 600 })).toBeNull();
  });

  it('retains raw physical safe edges and keyboard height for later viewport resampling', () => {
    expect(readIosViewport({ width: 874, height: 402, reservedRegions: [],
      top: 0, right: 62, bottom: 20, left: 62, keyboardVisibleBottom: 110 })).toEqual({
      width: 874, height: 402, verticalBarSide: 'unspecified', reservedRegions: [],
      top: 0, right: 62, bottom: 20, left: 62, keyboardVisibleBottom: 110,
    });
  });

  it('does not invent missing raw edges or accept invalid physical lengths', () => {
    expect(readIosViewport({ width: 402, height: 874, reservedRegions: [],
      top: NaN, right: Infinity, bottom: -1, keyboardVisibleBottom: '110' })).toEqual({
      width: 402, height: 874, verticalBarSide: 'unspecified', reservedRegions: [],
    });
    const setProperty = vi.fn();
    const value = { width: Number.MIN_VALUE, height: 402, top: 0, left: 62,
      keyboardVisibleBottom: 110, reservedRegions: [] };
    vi.stubGlobal('document', { documentElement: { clientWidth: 874,
      dataset: { mobileOs: 'ios', iosLayoutSource: 'uikit-viewport' }, style: { setProperty } } });
    vi.stubGlobal('window', { __polarisIosViewport: { sequence: 5, value } });
    try {
      expect(refreshNativeViewportCss()).toBeNull();
      expect(setProperty).not.toHaveBeenCalled();
      value.width = 437;
      value.height = Number.MAX_VALUE;
      expect(refreshNativeViewportCss()).toBeNull();
      expect(setProperty).not.toHaveBeenCalled();
      value.height = 402;
      value.top = Number.MAX_VALUE;
      expect(refreshNativeViewportCss()).toEqual({ width: 874, height: 804 });
      expect(setProperty).not.toHaveBeenCalledWith('--safe-t', expect.anything());
      expect(setProperty).toHaveBeenCalledWith('--safe-l', '124px');
      expect(setProperty).toHaveBeenCalledWith('--ios-keyboard-visible-bottom', '220px');
    } finally { vi.unstubAllGlobals(); }
  });

  it('a division is crossed by a spanning control; touching its edge is allowed', () => {
    expect(intersectsReserved({ x: 350, y: 20, width: 100, height: 48 }, region())).toBe(true);
    expect(intersectsReserved({ x: 300, y: 20, width: 100, height: 48 }, region())).toBe(false);
    expect(intersectsReserved({ x: 400, y: 20, width: 100, height: 48 }, region())).toBe(false);
  });

  it('preserves a valid original component position before considering larger empty regions', () => {
    const target = { x: 50, y: 60, width: 150, height: 80 };
    expect(avoidReserved(target, bounds, [region()])?.frame).toBe(target);
  });

  it('moves a colliding control to the nearest full-size position in its own bounds', () => {
    const target = { x: 370, y: 30, width: 100, height: 48 };
    expect(avoidReserved(target, bounds, [region()])?.frame).toEqual({ ...target, x: 400 });
  });

  it('keeps a panel on one side of a horizontal division without resizing its owner', () => {
    const division = region({ x: 0, y: 300, width: 800, height: 0 });
    const target = { x: 0, y: 200, width: 800, height: 400 };
    const result = avoidReserved(target, bounds, [division])!;
    expect(result.frame.height).toBe(300);
    expect(intersectsReserved(result.frame, division)).toBe(false);
    expect(bounds).toEqual({ x: 0, y: 0, width: 800, height: 600 });
  });

  it('satisfies every region together, including intersecting division and occlusion frames', () => {
    const regions = [region(), region({ kind: 'occlusion', x: 0, y: 300, width: 400, height: 300 })];
    const result = avoidReserved({ x: 0, y: 250, width: 800, height: 350 }, bounds, regions)!;
    expect(regions.some((r) => intersectsReserved(result.frame, r))).toBe(false);
    expect(result.frame.width).toBeGreaterThanOrEqual(48);
    expect(result.frame.height).toBeGreaterThanOrEqual(48);
  });

  it('cannot move rail controls into another column of text', () => {
    const rail = { x: 720, y: 0, width: 80, height: 600 };
    const block = region({ kind: 'occlusion', x: 720, y: 0, width: 80, height: 600 });
    expect(avoidReserved(rail, rail, [block])).toBeNull();
  });

  it('reports no candidate when all space is occluded or below the touch target minimum', () => {
    expect(avoidReserved({ x: 0, y: 0, width: 80, height: 48 }, bounds, [region({ kind: 'occlusion', x: 0, width: 800 })])).toBeNull();
    expect(avoidReserved({ x: 0, y: 0, width: 80, height: 48 }, { x: 0, y: 0, width: 20, height: 20 }, [])).toBeNull();
  });
});
